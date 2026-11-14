//! The unix socket transport: one path, both sides of a session.
//!
//! ```text
//!   one side: bind(path) ──▶ writes "unix : path" into its output ──▶ the other reads it
//!   the other: from_magic("unix", path) ──▶ connect()
//! ```
//!
//! The side that binds chooses the path, because nothing else can: a peer cannot write into
//! its filesystem, and a path provisioned from outside is a path two clients would share. It
//! picks under `XDG_RUNTIME_DIR` — private to the user, cleared on logout — and says so in
//! the output the other half is already reading.
//!
//! # Both halves are here
//!
//! [`UnixSocket`] dials *and* accepts. Either side of a session may take either role, and the
//! two roles are less different than they look: both end in a greeting and an answer, and
//! only the order differs — the one that dialed greets, and the one that was dialed answers.
//! A type that could only do one of them would be a type the other side cannot use, and the
//! server and the client are the same program's two ends rather than two kinds of thing.

use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use futures_util::{SinkExt as _, StreamExt as _};
use tokio::io::ReadHalf;
use tokio::sync::Mutex;
use tokio_util::codec::{FramedRead, FramedWrite};

use crate::message::Message;
use crate::transport::{framed_read, framed_write, Connection, MessageCodec, Transport};

/// Stable key for this transport, as it appears in an announcement.
pub const NAME: &str = "unix";

/// A unix socket at a known path.
///
/// Cloneable, and the clone is the same socket: the listener and the link are behind `Arc`s,
/// so a [`Connection`] can hold one handle while the caller keeps another for the next
/// accept.
#[derive(Clone)]
pub struct UnixSocket {
    path: PathBuf,
    /// The bound listener, once bound.
    ///
    /// `None` until [`UnixSocket::bind`], and kept afterwards so a second call is not a
    /// rebind. An `Arc` because a listener is shared with the accept that uses it and cannot
    /// be cloned itself: `tokio`'s `UnixListener` has no `try_clone`, unlike its std
    /// counterpart.
    listener: Arc<Mutex<Option<Arc<tokio::net::UnixListener>>>>,
    /// The link, once there is one.
    ///
    /// Swapped in once by [`UnixSocket::adopt`] and read by every send and receive, so a read or a
    /// write waits for nothing but its own direction: the two halves have their own locks.
    link: Arc<arc_swap::ArcSwapOption<Link>>,
}

/// The connected half: the stream, split so a read never holds up a write.
///
/// The two halves are one socket behind two `Arc`s, and each carries its own
/// half of the codec: a read that returns fewer bytes than a message holds is
/// the codec library's problem, not this file's.
///
/// A lock per direction, which is the point rather than a detail: a session reads and writes at
/// the same time, and one lock for both means a read waiting for the peer holds off the very
/// write the peer is waiting for.
struct Link {
    read: Mutex<FramedRead<ReadHalf<tokio::net::UnixStream>, MessageCodec>>,
    write: Mutex<FramedWrite<tokio::io::WriteHalf<tokio::net::UnixStream>, MessageCodec>>,
}

impl UnixSocket {
    /// A socket at `path`, listening.
    ///
    /// Binding is part of making one: an address nothing listens at is not an address a peer can
    /// dial, and every use of this is followed by `bind` or was a caller that forgot to.
    pub async fn at(path: impl Into<PathBuf>) -> io::Result<Self> {
        let socket = Self {
            path: path.into(),
            listener: Arc::new(Mutex::new(None)),
            link: Arc::new(arc_swap::ArcSwapOption::empty()),
        };
        socket.bind().await?;
        Ok(socket)
    }

    /// A socket at `path` that is not listening, for a caller that will bind it itself.
    ///
    /// Only for the paths `Transport::new` walks when it finds the standard address taken: it has
    /// to try a bind and see, so it cannot use [`UnixSocket::at`].
    fn unbound(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            listener: Arc::new(Mutex::new(None)),
            link: Arc::new(arc_swap::ArcSwapOption::empty()),
        }
    }

    /// The paths a fresh socket may listen at, in the order to try them.
    ///
    /// `$XDG_RUNTIME_DIR/tuigui.sock` is the address both halves of a session write down, which is
    /// what lets them find each other without a scan; the socket is announced as well, so a peer
    /// reaches it by the announcement rather than by guessing.
    ///
    /// The names after it are for a process that already has a live socket at the standard one —
    /// a second session, or a test runner running tests side by side. Each is announced, so a peer
    /// still dials the address it was given.
    fn socket_paths() -> impl Iterator<Item = PathBuf> {
        (0..16).map(|n| {
            let name = if n == 0 {
                "tuigui.sock".to_owned()
            } else {
                format!("tuigui-{n}.sock")
            };
            match std::env::var_os("XDG_RUNTIME_DIR") {
                Some(dir) => std::path::Path::new(&dir).join(name),
                // No runtime dir — a container, a test runner — so a directory of this process's
                // own, which keeps a name here from reaching another user's session.
                None => std::env::temp_dir().join(format!("tuigui-{}", std::process::id())).join(name),
            }
        })
    }

    /// The path this socket is at.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Bind the socket, so that the address this announces is one that answers.
    ///
    /// Separate from [`Transport::connect`] and [`Transport::accept`] because it has to
    /// happen *before* either: the address is written into an announcement, and a peer that
    /// dials the address of a socket nobody has bound dials nothing.
    ///
    /// A second call is not a rebind, which is what lets a caller be careless about it.
    pub async fn bind(&self) -> io::Result<()> {
        let mut listener = self.listener.lock().await;
        if listener.is_none() {
            // A stale file from a previous run would make the bind fail and look like a
            // permissions problem. Removing it is what `sshd` does with
            // `StreamLocalBindUnlink`.
            match std::fs::remove_file(&self.path) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(io::Error::new(
                        e.kind(),
                        format!("removing stale {}: {e}", self.path.display()),
                    ))
                }
            }
            if let Some(dir) = self.path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            *listener = Some(Arc::new(tokio::net::UnixListener::bind(&self.path)?));
        }
        Ok(())
    }

    /// Adopt a stream as this socket's link.
    ///
    /// Shared by dialing and accepting, which differ only in where the stream came from.
    async fn adopt(&self, stream: tokio::net::UnixStream) {
        let (read, write) = tokio::io::split(stream);
        self.link.store(Some(Arc::new(Link {
            read: Mutex::new(framed_read(read)),
            write: Mutex::new(framed_write(write)),
        })));
    }
}

#[async_trait::async_trait]
impl Transport for UnixSocket {
    fn name(&self) -> &'static str {
        NAME
    }

    /// The socket both halves of a local session agree on, listening.
    ///
    /// Bound here because `to_magic` has to name an address a peer can actually dial: an unbound
    /// socket announces a path nothing answers at. The standard path is tried first, that being
    /// the address two halves of a session find each other by; a process that already has a live
    /// socket there takes the next free name instead, and announces it.
    async fn new() -> io::Result<Self> {
        let mut taken = None;
        for path in Self::socket_paths() {
            let socket = Self::unbound(path);
            match socket.bind().await {
                Ok(()) => return Ok(socket),
                // A listener that is still there. Any other failure is one another name will not
                // fix, so it is the caller's to hear.
                Err(e) if e.kind() == io::ErrorKind::AddrInUse => taken = Some(e),
                Err(e) => return Err(e),
            }
        }
        Err(taken.expect("a path was offered"))
    }

    fn address(&self) -> Option<String> {
        Some(self.path.to_string_lossy().into_owned())
    }

    /// A dialer: the socket at the announced path, with nothing listening here.
    fn from_magic(name: &str, address: &str) -> Option<Self> {
        (name == NAME).then(|| Self::unbound(address))
    }

    /// Dial the path this socket holds, greet, and wait for the answer.
    ///
    /// The greeting is here and not in the caller because the two halves of the handshake
    /// only make sense together: the side that dialed is the side that knows the link is new,
    /// so it greets, and a caller that forgot to would leave the other end waiting forever.
    #[tracing::instrument(name = "msg.connect", skip_all)]
    async fn connect(&self) -> io::Result<Connection> {
        let stream = tokio::net::UnixStream::connect(&self.path).await?;
        self.adopt(stream).await;

        self.send(&Message::Ping(self.label())).await?;
        match self.recv().await? {
            Some(Message::Pong) => Ok(Connection::new(self.clone())),
            Some(other) => Err(io::Error::other(format!(
                "a peer answered the greeting with {other:?}"
            ))),
            None => Err(io::Error::other("a peer never answered the greeting")),
        }
    }

    /// Wait for a peer to dial, answer its greeting, and return the link.
    ///
    /// The mirror of [`Transport::connect`]. The order is fixed here rather than left to a
    /// caller: answering before the greeting arrives would be answering nothing, and reading
    /// after it would be reading a greeting already consumed.
    #[tracing::instrument(name = "msg.accept", skip_all)]
    async fn accept(&self) -> io::Result<Connection> {
        let listener = {
            let guard = self.listener.lock().await;
            guard
                .as_ref()
                .ok_or_else(|| io::Error::other("this socket is not bound"))?
                .clone()
        };
        let (stream, _) = listener.accept().await?;
        self.adopt(stream).await;

        match self.recv().await? {
            // The greeting. Answering it is what completes the handshake, and until it is
            // answered the other end is still waiting.
            Some(Message::Ping(_)) => {
                self.send(&Message::Pong).await?;
                Ok(Connection::new(self.clone()))
            }
            Some(other) => Err(io::Error::other(format!(
                "a peer's first message was {other:?}, not a greeting"
            ))),
            None => Err(io::Error::other("a peer connected but never greeted")),
        }
    }

    #[tracing::instrument(name = "msg.send", skip_all)]
    async fn send(&self, message: &Message) -> io::Result<()> {
        let link = self
            .link
            .load_full()
            .ok_or_else(|| io::Error::other("this socket has no link yet"))?;
        let sent = link.write.lock().await.send(message.clone()).await;
        sent
    }

    /// Read one message, waiting for a whole one.
    ///
    /// A read that returns the instant it is asked would ask before the peer has had a chance
    /// to write, and every handshake would fail on a peer that was merely not instant. So this
    /// waits, and `None` means the peer closed rather than "nothing yet" — an `Err` here is
    /// a peer that closed *mid-message*, which the codec reports rather than calling clean.
    ///
    /// The read's own lock is held across the await, which is what keeps two readers from
    /// interleaving on one stream. It is not the write's lock: a session reads and writes at the
    /// same time, and one lock for both is a link that deadlocks itself.
    #[tracing::instrument(name = "msg.recv", skip_all)]
    async fn recv(&self) -> io::Result<Option<Message>> {
        let link = self
            .link
            .load_full()
            .ok_or_else(|| io::Error::other("this socket has no link yet"))?;
        let heard = match link.read.lock().await.next().await {
            Some(Ok(message)) => Ok(Some(message)),
            Some(Err(e)) => Err(e),
            None => Ok(None),
        };
        heard
    }
}

impl UnixSocket {
    /// What this transport calls itself, for the greeting.
    ///
    /// The path, because that is what the other end would need to say which socket this is.
    /// The label is logs and nothing else: the name and address are what identify a
    /// transport.
    fn label(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }
}

#[cfg(test)]
#[path = "../tests/unit/unix.rs"]
mod tests;
