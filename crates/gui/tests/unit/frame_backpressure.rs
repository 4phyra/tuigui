//! The client's frame path against a link slower than the client can draw: run the real
//! session loop against the scripted windowing and a throttled transport, and check that
//! everything the window hands to the link arrives at the other end.
//!
//! The only parts of the path that are not real are the ones that cannot exist in a test:
//! the wayland side is the scripted `testing_windowing`, and the socket is a token-bucket
//! link instead of a kernel one. Everything between — the servant loop, the encode, the
//! `Message::Frame` on a `Connection`, and the read side — is the shipping code.
//!
//! A slow enough link means the client can draw faster than it can send, which is the
//! situation under test: the frames that do leave must arrive, and the count of frames that
//! left must equal the count that arrived.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use wire::message::Message;
use wire::transport::{Connection, Transport};

use crate::windowing::testing;

/// The test's link: bytes pass at `RATE` per second, with `BURST` of slack.
///
/// The counterpart of the wire crate's throttle fixture; kept separate because test fixtures
/// are not shared across crates, and the one thing a fixture must not be is configurable into
/// accidentally passing.
#[derive(Clone)]
struct Throttled {
    inner: Arc<ThrottledInner>,
}

struct ThrottledInner {
    bucket: Mutex<TokenBucket>,
    tx: mpsc::Sender<Message>,
    rx: tokio::sync::Mutex<mpsc::Receiver<Message>>,
}

struct TokenBucket {
    tokens: f64,
    rate: f64,
    capacity: f64,
    updated: Instant,
}

impl Throttled {
    fn pair(rate: u64, burst: u64) -> (Self, Self) {
        let make = || {
            let (tx, rx) = mpsc::channel(8);
            (
                tx,
                rx,
                TokenBucket {
                    tokens: burst as f64,
                    rate: rate as f64,
                    capacity: burst as f64,
                    updated: Instant::now(),
                },
            )
        };
        let (tx_a, rx_a, bucket_a) = make();
        let (tx_b, rx_b, bucket_b) = make();
        // Each end writes into the other's channel; each paces its own writes.
        (
            Self {
                inner: Arc::new(ThrottledInner {
                    bucket: Mutex::new(bucket_a),
                    tx: tx_a,
                    rx: tokio::sync::Mutex::new(rx_b),
                }),
            },
            Self {
                inner: Arc::new(ThrottledInner {
                    bucket: Mutex::new(bucket_b),
                    tx: tx_b,
                    rx: tokio::sync::Mutex::new(rx_a),
                }),
            },
        )
    }

    async fn acquire(&self, amount: u64) {
        let amount = amount as f64;
        loop {
            let wait = {
                let mut bucket = self.inner.bucket.lock().expect("the bucket is only locked here");
                let now = Instant::now();
                bucket.tokens = (bucket.tokens + now.duration_since(bucket.updated).as_secs_f64() * bucket.rate)
                    .min(bucket.capacity);
                bucket.updated = now;
                if bucket.tokens >= amount.min(bucket.capacity) {
                    // A single message larger than the burst is still sent; it drives the
                    // bucket negative and future refills pay it off. Never waits on an
                    // amount the ceiling can never hold.
                    bucket.tokens -= amount;
                    return;
                }
                Duration::from_secs_f64((amount - bucket.tokens) / bucket.rate)
            };
            tokio::time::sleep(wait).await;
        }
    }
}

#[async_trait::async_trait]
impl Transport for Throttled {
    fn name(&self) -> &'static str {
        "throttled"
    }

    async fn new() -> std::io::Result<Self> {
        Err(std::io::Error::other("a throttled link is made with Throttle::pair, not discovered"))
    }

    fn address(&self) -> String {
        // Never announced — links from this throttle are handed over directly, never scanned for.
        "throttled".to_owned()
    }

    fn from_magic(_name: &str, _address: &str) -> Option<Self> {
        None
    }

    async fn connect(&self) -> std::io::Result<Connection> {
        Ok(Connection::new(self.clone()))
    }

    async fn accept(&self) -> std::io::Result<Connection> {
        Ok(Connection::new(self.clone()))
    }

    async fn send(&self, message: &Message) -> std::io::Result<()> {
        let cost = match message {
            Message::Frame { bytes, .. } => 16 + bytes.as_ref().map_or(0, |b| b.len() as u64),
            _ => 16,
        };
        self.acquire(cost).await;
        self.inner
            .tx
            .send(message.clone())
            .await
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "the reading half is gone"))
    }

    async fn recv(&self) -> std::io::Result<Option<Message>> {
        Ok(self.inner.rx.lock().await.recv().await)
    }
}

const LINK_RATE: u64 = 64 * 1024;
const LINK_BURST: u64 = 4 * 1024;

/// The window draws at its own pace, the link moves a fraction of that, and still every byte
/// the client puts on the link is what the server side reads — and when the test says stop,
/// the session ends cleanly with everything drained.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_over_a_slow_link_loses_nothing() {
    testing::reset();
    crate::stats::reset();

    let (client_end, server_end) = Throttled::pair(LINK_RATE, LINK_BURST);
    let (transport_tx, transport_rx) = tokio::sync::oneshot::channel();
    transport_tx
        .send(Box::new(client_end) as Box<dyn Transport>)
        .map_err(|_| ())
        .expect("nothing has been dropped yet");
    let (root_tx, mut root_rx) = tokio::sync::oneshot::channel();
    // Held for the whole test: the client ends its session when the compositor drops its
    // handle on the window, so dropping this would end it before a frame is drawn.
    let mut root_keep = None;

    let client = tokio::spawn(crate::client::run(
        root_tx,
        crate::windowing::Options {
            resolution: (320, 200),
            default_window_size: (160, 100),
            dpi: 1.0,
            use_x11: false,
        },
        codecs::CodecId::Jpeg,
        crate::DebugOptions::default(),
        std::path::PathBuf::from("test-windowing"),
        transport_rx,
    ));

    let connection = Connection::new(server_end);
    let mut frames = 0u64;
    let mut bytes = 0u64;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let stop_at = tokio::time::Instant::now() + Duration::from_secs(3);
    let mut stopped = false;

    loop {
        let message = tokio::time::timeout_at(deadline, connection.recv())
            .await
            .expect("the session stalls and never comes back");
        match message {
            Ok(Some(Message::Frame { bytes: Some(payload), .. })) => {
                frames += 1;
                bytes += payload.len() as u64;
            }
            Ok(Some(Message::OpenWindow { surface, .. })) => {
                // The session will not place a frame until the surface is registered.
                connection
                    .send(&Message::WindowCreated { surface })
                    .await
                    .expect("the client end is still there");
            }
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => break,
        }
        if root_keep.is_none() {
            match root_rx.try_recv() {
                Ok(root) => root_keep = Some(root),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    panic!("the client dropped the root before placing it")
                }
            }
        }
        if !stopped && tokio::time::Instant::now() >= stop_at {
            testing::stop();
            stopped = true;
        }
    }

    let result = client.await.expect("the client task does not panic");
    eprintln!("client returned: {result:?}");
    assert!(result.is_ok(), "the session ends cleanly: {result:?}");
    assert!(frames >= 5, "the session moved something: {frames} frames");
    assert_eq!(
        frames,
        testing::frames_sent(),
        "every frame the client handed to the link arrived"
    );
    assert_eq!(bytes, crate::stats::uploaded(), "every byte the client sent arrived");
}
