//! A producer that outruns its link: whether the link holds.
//!
//! The client's frame path is depth one: a frame is built and handed to
//! [`Connection::send`], and the send does not complete until the link accepts the bytes. A
//! producer faster than the link can therefore only be held back, never buffered ahead without
//! bound — the await is the throttle. A link that drained slower than the frames are made
//! must lose nothing, must not reorder, and must slow the producer down rather than let it
//! run ahead: this file puts a link that drains at a fixed byte rate between the two ends and
//! checks exactly those three.
//!
//! The throttle here is deliberately unlike the real transports, which push bytes as fast as
//! the kernel accepts them and lean on the socket buffer for elasticity. Here the rate and
//! the burst are numbers, so "the link cannot keep up" is a fixture, not the mood of the
//! machine — and the waits it forces are the behavior under test, not an accident of load.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use wire::message::{Message, SurfaceId, FramePosition};
use wire::transport::{Connection, Transport};

/// A link that drains at `rate` bytes per second, with at most `burst` bytes of slack.
///
/// The two ends are made as one by [`Throttle::pair`], the way a socket pair is. `send` waits
/// for enough of the budget to cover the message before it moves it, so a producer that
/// outruns the rate is simply slower — the frame is never copied into an unbounded queue or
/// dropped on the floor.
struct Throttled {
    /// Shared with nothing else: each end paces itself. `std::Mutex` because the critical
    /// section is arithmetic and never held across an await.
    bucket: Arc<Mutex<TokenBucket>>,
    /// The bytes this side writes into the link. Capacity is a message count, not a byte
    /// count: what must hold is that the paired receiver is bounded, so a stalled reader is a
    /// stalled writer, not a growing allocation.
    tx: mpsc::Sender<Message>,
    rx: tokio::sync::Mutex<mpsc::Receiver<Message>>,
}

struct TokenBucket {
    /// Bytes that may be sent right now.
    tokens: f64,
    /// Bytes per second the bucket refills at.
    rate: f64,
    /// The most the bucket ever holds — the burst the link forgives.
    capacity: f64,
    /// When the bucket was last topped up.
    updated: Instant,
}

impl TokenBucket {
    fn new(rate: u64, burst: u64) -> Self {
        Self {
            tokens: burst as f64,
            rate: rate as f64,
            capacity: burst as f64,
            updated: Instant::now(),
        }
    }

}

impl Throttled {
    /// Wait until `amount` bytes may be sent, then charge them.
    async fn acquire(&self, amount: u64) {
        let amount = amount as f64;
        loop {
            let wait = {
                let mut bucket = self.bucket.lock().expect("the bucket is only locked here");
                let now = Instant::now();
                let refilled = now.duration_since(bucket.updated).as_secs_f64() * bucket.rate;
                bucket.tokens = (bucket.tokens + refilled).min(bucket.capacity);
                bucket.updated = now;
                if bucket.tokens >= amount.min(bucket.capacity) {
                    // A single message larger than the burst is still sent; it drives
                    // the bucket negative and future refills pay it off.
                    bucket.tokens -= amount;
                    return;
                }
                Duration::from_secs_f64((amount - bucket.tokens) / bucket.rate)
            };
            tokio::time::sleep(wait).await;
        }
    }
}

impl Throttled {
    /// The two ends of one link, each draining at `rate` bytes per second with `burst` bytes
    /// of slack. Ends are independent: what one side writes is what the other reads, at the
    /// writer's pace.
    fn pair(rate: u64, burst: u64) -> (Self, Self) {
        let make = || (
            mpsc::channel(8),
            TokenBucket::new(rate, burst),
        );
        let ((tx_a, rx_a), bucket_a) = make();
        let ((tx_b, rx_b), bucket_b) = make();
        (
            Self {
                bucket: Arc::new(Mutex::new(bucket_a)),
                tx: tx_a,
                rx: tokio::sync::Mutex::new(rx_b),
            },
            Self {
                bucket: Arc::new(Mutex::new(bucket_b)),
                tx: tx_b,
                rx: tokio::sync::Mutex::new(rx_a),
            },
        )
    }
}

/// The number of bytes `Message::Frame` costs to move, as the pacing charge.
///
/// The payload dominates by orders of magnitude, so the framing overhead is a flat allowance
/// rather than a serialization: the test asserts on order and content, not on accounting, and
/// the rate is chosen to make the pacing unmissable at any sane overhead.
fn frame_cost(frame: &Message) -> u64 {
    match frame {
        Message::Frame { bytes, .. } => {
            16 + bytes.as_ref().map_or(0, |b| u64::try_from(b.len()).unwrap_or(u64::MAX))
        }
        _ => 16,
    }
}

#[async_trait::async_trait]
impl Transport for Throttled {
    fn name(&self) -> &'static str {
        "throttled"
    }

    async fn new() -> std::io::Result<Self> {
        Err(std::io::Error::other(
            "a throttled link is made with Throttle::pair, not discovered",
        ))
    }

    fn address(&self) -> String {
        // Never announced — links from `Throttle::pair` are passed directly, never scanned for.
        "throttled".to_owned()
    }

    fn from_magic(_name: &str, _address: &str) -> Option<Self>
    where
        Self: Sized,
    {
        None
    }

    async fn connect(&self) -> std::io::Result<Connection> {
        Err(std::io::Error::other(
            "a throttled link has nothing to dial; pair its two ends directly",
        ))
    }

    async fn accept(&self) -> std::io::Result<Connection> {
        Err(std::io::Error::other(
            "a throttled link has nothing to accept; pair its two ends directly",
        ))
    }

    async fn send(&self, message: &Message) -> std::io::Result<()> {
        let cost = frame_cost(message);
        // The wait happens before anything is queued, which is the whole point: the caller
        // cannot get ahead of the link no matter how fast it calls this.
        self.acquire(cost).await;
        self.tx
            .send(message.clone())
            .await
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "the reading half is gone"))
    }

    async fn recv(&self) -> std::io::Result<Option<Message>> {
        Ok(self.rx.lock().await.recv().await)
    }
}

fn surface() -> SurfaceId {
    SurfaceId {
        symbol: 'x',
        fg: (200, 10, 10),
        bg: (10, 10, 200),
    }
}

/// One frame whose bytes say which frame it is, so a payload that arrives in the wrong order
/// or damaged is caught by comparing against what was sent.
fn frame(index: u32, payload: usize) -> Message {
    let mut bytes = vec![(index % 251) as u8; payload];
    bytes[..4].copy_from_slice(&index.to_le_bytes());
    Message::Frame {
        surface: surface(),
        position: FramePosition {
            z: 0,
            cols: 80,
            rows: 24,
        },
        bytes: Some(bytes),
    }
}

fn payload(message: &Message) -> Option<&[u8]> {
    match message {
        Message::Frame { bytes, .. } => bytes.as_deref(),
        _ => None,
    }
}

/// A frame per call, faster than the link can move them: everything sent is received, in the
/// order it was sent. The lossless half of "the bottleneck is handled correctly".
#[tokio::test]
async fn every_frame_arrives_in_order() {
    let (a, b) = Throttled::pair(64 * 1024 * 1024, 4 * 1024 * 1024);
    let sender = Connection::new(a);
    let receiver = Connection::new(b);

    let frames: Vec<Message> = (0..60).map(|i| frame(i, 16 * 1024)).collect();
    let expected = frames.clone();

    let producer = tokio::spawn(async move {
        for message in &frames {
            sender.send(message).await.expect("the link stays up");
        }
        // `sender` drops here, which is what ends the reader's loop.
    });
    let consumer = tokio::spawn(async move {
        let mut got = Vec::new();
        while let Some(message) = receiver.recv().await.expect("the link stays readable") {
            got.push(message);
        }
        got
    });

    producer.await.expect("the producer task does not panic");
    let got = consumer.await.expect("the consumer task does not panic");

    assert_eq!(got, expected, "every frame arrives, in the order it was sent");
    let sent_bytes: usize = expected.iter().map(|f| payload(f).map_or(0, <[_]>::len)).sum();
    let got_bytes: usize = got.iter().map(|f| payload(f).map_or(0, <[_]>::len)).sum();
    assert_eq!(sent_bytes, got_bytes, "every payload byte arrives");
}

/// A link draining at a fixed rate paces the producer: it cannot hand over frames faster
/// than the bytes go. This is the other half of "the bottleneck is handled correctly" — the
/// producer is not merely slower than it could be, it is held by the link itself, which is
/// what a fast client meeting a slow terminal needs.
#[tokio::test]
async fn the_link_paces_the_producer() {
    // 1 MiB/s, one frame of slack. Eight 64 KiB frames must take at least the time the
    // link needs for the seven beyond the burst — if `send` were not waiting on the link,
    // the producer would run to completion in microseconds.
    const RATE: u64 = 1024 * 1024;
    const PAYLOAD: usize = 64 * 1024;
    const FRAMES: u32 = 8;

    let (a, b) = Throttled::pair(RATE, PAYLOAD as u64 + 32);
    let sender = Connection::new(a);
    let receiver = Connection::new(b);

    let consumer = tokio::spawn(async move {
        let mut got = Vec::new();
        while let Some(message) = receiver.recv().await.expect("the link stays readable") {
            got.push(message);
        }
        got
    });

    let started = Instant::now();
    for i in 0..FRAMES {
        sender.send(&frame(i, PAYLOAD)).await.expect("the link stays up");
    }
    let elapsed = started.elapsed();
    drop(sender);

    // One frame's worth of burst is free; the other FRAMES - 1 frames are rate-limited.
    let floor = Duration::from_secs_f64((f64::from(FRAMES - 1)) * PAYLOAD as f64 / RATE as f64);
    assert!(
        elapsed >= floor,
        "the producer returned in {elapsed:?}, before the link can have carried {FRAMES} frames: \
         the send did not wait for the link",
    );

    let got = consumer.await.expect("the consumer task does not panic");
    assert_eq!(got.len(), FRAMES as usize, "nothing is lost even when the producer outruns the link");
}

/// Two producers sharing one link — the shape of two windows served on one connection —
/// keep their own order intact, and everything they send arrives.
#[tokio::test]
async fn two_producers_keep_their_own_order() {
    let (a, b) = Throttled::pair(8 * 1024 * 1024, 64 * 1024);
    let sender = Connection::new(a);
    let link = Arc::new(sender);
    let receiver = Connection::new(b);

    let producer = |z: u32| {
        let link = Arc::clone(&link);
        tokio::spawn(async move {
            for i in 0..24u32 {
                let mut message = frame(i, 8 * 1024);
                if let Message::Frame {
                    position: FramePosition { z: ref mut slot, .. },
                    ..
                } = message
                {
                    *slot = z;
                }
                link.send(&message).await.expect("the link stays up");
            }
        })
    };
    let (first, second) = (producer(1), producer(2));

    let consumer = tokio::spawn(async move {
        let mut got = Vec::new();
        while let Some(message) = receiver.recv().await.expect("the link stays readable") {
            got.push(message);
        }
        got
    });

    first.await.expect("the first producer does not panic");
    second.await.expect("the second producer does not panic");
    drop(link);
    let got = consumer.await.expect("the consumer task does not panic");

    assert_eq!(got.len(), 48, "both producers' frames all arrive");
    for z in [1, 2] {
        let indices: Vec<u32> = got
            .iter()
            .filter_map(|message| match message {
                Message::Frame {
                    position,
                    bytes,
                    ..
                } if position.z == z => bytes.as_ref().map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])),
                _ => None,
            })
            .collect();
        let mut expected: Vec<u32> = (0..24).collect();
        expected.sort_unstable();
        assert_eq!(indices, expected, "producer {z}'s frames arrive in the order it sent them");
    }
}
