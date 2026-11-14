//! The second channel: a QUIC link between two endpoints, each named by a key.
//!
//! ```text
//!   one side: host() ──▶ writes "p2p : <ticket>" into its output ──▶ the other reads it
//!   the other: from_magic("p2p", ticket) ──▶ connect()
//! ```
//!
//! The shape is [`crate::unix`]'s, because the seam is the same: a transport is one
//! value that carries its own link, `connect` and `accept` each end in a greeting, and
//! the announced text is the only thing that crosses between the two sides. What differs
//! is what an address *is*. A unix socket is a path — a fact about one machine, which is
//! why that transport cannot leave it. A `p2p` address is a public key plus the ways to
//! reach it, which is a fact that survives being written down and read somewhere else,
//! so this is the transport a session uses when the two ends are not one machine.
//!
//! # An endpoint is a node, not a peer
//!
//! Dialing over QUIC means *being* a node: your own key, your own sockets, your own
//! handshake. So the endpoint a [`P2p`] holds is its own, never the other side's, and
//! both ends of a session have one whether they dial or accept. That is also why it is
//! built here and cached: it owns the UDP socket and the relay connection, so an
//! endpoint per attempt would be a socket per frame's worth of work, and one dropped at
//! the end of a call would take the link down with it.
//!
//! # The ticket is not the key
//!
//! [`EndpointAddr`] is the key *and* the addresses — a relay URL, any addresses already
//! known — because a key alone says who to talk to and nothing about how. Its text form
//! is a ticket, and that is what an announcement carries: base32 behind a kind prefix,
//! which is a plain printable string, so the scan finds it the same way it finds a path.

use std::io;
use std::sync::Arc;

use futures_util::{SinkExt as _, StreamExt as _};
use iroh::endpoint::presets;
use iroh::{Endpoint, EndpointAddr};
use iroh_tickets::endpoint::EndpointTicket;
use tokio::sync::Mutex;
use tokio_util::codec::{FramedRead, FramedWrite};

use crate::message::Message;
use crate::transport::{framed_read, framed_write, Connection, MessageCodec, Transport};

/// Stable key for this transport, as it appears in an announcement.
pub const NAME: &str = "p2p";

/// The application protocol both ends agree on.
///
/// QUIC refuses a connection whose ALPN the two sides do not share, so this is what keeps
/// an iroh endpoint of someone else's speaking to ours: a session is named, not merely
/// addressed. Versioned because a change to [`Message`] is a change to the conversation,
/// and two builds that disagree should fail to connect rather than misread each other.
const ALPN: &[u8] = b"tuigui/1";

/// A session over QUIC, at an address a public key names.
///
/// Cloneable, and the clone is the same session: the endpoint and the link are behind
/// `Arc`s, so a [`Connection`] can hold one handle while the caller keeps another.
///
/// Two of the fields are filled later rather than at construction, and both for the same
/// reason: the work they stand for is `async` and this type is built by a synchronous
/// [`Transport::from_magic`]. See [`P2p::host`] and [`Transport::connect`].
#[derive(Clone)]
pub struct P2p {
    /// Who a dial reaches, when this transport is the one dialing.
    ///
    /// Absent when hosting, and not because it is "not known yet": a host has no peer until
    /// one dials in, and that peer comes out of the accept. Keeping an empty slot here would
    /// be a field whose emptiness means something other than emptiness.
    peer: Option<EndpointAddr>,
    /// This side's own endpoint, once bound.
    ///
    /// Held, not borrowed: it owns the socket the link runs on, so it has to outlive every
    /// connection made through it. A tokio mutex because it is bound under an await.
    endpoint: Arc<Mutex<Option<Endpoint>>>,
    /// The link, once there is one.
    ///
    /// Swapped in once by [`P2p::adopt`] and read by every send and receive, so a read or a write
    /// waits for nothing but its own direction.
    link: Arc<arc_swap::ArcSwapOption<Link>>,
}

/// The connected half: the two directions of one bidirectional QUIC stream.
///
/// No split is needed here, unlike a unix socket: QUIC hands out the two directions separately,
/// so a read and a write are already different objects. A lock each, so that holding one for a
/// wait — which a read does — never holds off the other.
struct Link {
    read: Mutex<FramedRead<iroh::endpoint::RecvStream, MessageCodec>>,
    write: Mutex<FramedWrite<iroh::endpoint::SendStream, MessageCodec>>,
}

impl P2p {
    /// Bind an endpoint a peer can reach, and be ready to be dialed.
    ///
    /// Separate from [`Transport::accept`] because the address has to be announced
    /// *before* anyone dials it: [`Transport::to_magic`] answers from the endpoint this
    /// binds, so a transport that had not bound yet would have nothing to advertise.
    ///
    /// The preset is `N0`, which is a relay and a DNS lookup service: a relay so the two
    /// ends can reach each other through NAT at all, and the lookup so an [`EndpointAddr`]
    /// can be resolved from its key alone once the peer has published. Hole punching
    /// replaces the relay with a direct path as soon as one works; that is the library's
    /// business and not this file's.
    pub async fn host() -> io::Result<Self> {
        let endpoint = Endpoint::builder(presets::N0)
            .alpns(vec![ALPN.to_vec()])
            .bind()
            .await
            .map_err(|e| io::Error::other(format!("could not bind an endpoint: {e}")))?;
        Ok(Self {
            peer: None,
            // Bound already, which is the whole point of this constructor: what `to_magic`
            // reads and what `accept` waits on are the same endpoint.
            endpoint: Arc::new(Mutex::new(Some(endpoint))),
            link: Arc::new(arc_swap::ArcSwapOption::empty()),
        })
    }

    /// The endpoint, binding one if there is none yet.
    ///
    /// The dialer's half of [`P2p::host`]: it has no endpoint when it is built from an
    /// announcement, and needs one before it can connect. Bound lazily rather than eagerly
    /// because the only thing that can bind it is a future, and this is where there is one.
    ///
    /// Bound once and kept. A second call is not a rebind, which is what lets both the
    /// handshake and a retry reach it without agreeing on who went first.
    async fn endpoint(&self) -> io::Result<Endpoint> {
        let mut endpoint = self.endpoint.lock().await;
        if endpoint.is_none() {
            *endpoint = Some(
                Endpoint::builder(presets::N0)
                    .alpns(vec![ALPN.to_vec()])
                    .bind()
                    .await
                    .map_err(|e| io::Error::other(format!("could not bind an endpoint: {e}")))?,
            );
        }
        endpoint
            .as_ref()
            .map(Endpoint::clone)
            .ok_or_else(|| io::Error::other("an endpoint was just bound"))
    }

    /// Adopt a stream pair as this transport's link.
    ///
    /// Shared by dialing and accepting, which differ only in who opened the stream.
    async fn adopt(&self, (send, recv): (iroh::endpoint::SendStream, iroh::endpoint::RecvStream)) {
        self.link.store(Some(Arc::new(Link {
            read: Mutex::new(framed_read(recv)),
            write: Mutex::new(framed_write(send)),
        })));
    }

    /// What this transport calls itself, for the greeting.
    ///
    /// The key, because that is what identifies the endpoint at the other end: an address
    /// is how to reach it, and two addresses can name one endpoint.
    fn label(addr: &EndpointAddr) -> String {
        EndpointTicket::new(addr.clone()).to_string()
    }
}

#[async_trait::async_trait]
impl Transport for P2p {
    fn name(&self) -> &'static str {
        NAME
    }

    /// A transport hosting at the address this channel uses when nobody named one.
    ///
    /// Bound here because `to_magic` has to name an address a peer can dial, and an endpoint's key
    /// is minted at bind time: until there is one there is nothing to announce.
    async fn new() -> io::Result<Self> {
        Self::host().await
    }

    /// The address to dial, as the text an announcement carries.
    ///
    /// Our own when hosting — read from the bound endpoint, because the key is minted at
    /// bind time and cannot be known before it. `None` when dialing, and that is the
    /// honest answer: a dialer has no address for anyone to reach it at, which is exactly
    /// what makes it unreachable in a scan.
    fn address(&self) -> Option<String> {
        let addr = self.peer.clone().or_else(|| {
            // A bound endpoint answers for itself; one that is not bound has no address,
            // and blocking here to find out would mean parking a thread on a lock that a
            // connect may be holding.
            self.endpoint
                .try_lock()
                .ok()
                .and_then(|guard| guard.as_ref().map(Endpoint::addr))
        })?;
        Some(EndpointTicket::new(addr).to_string())
    }

    /// Parse the ticket an announcement carries.
    ///
    /// Only the address is read here. Binding an endpoint is `async` and this is not, and
    /// the endpoint is this side's own anyway: see [`P2p::endpoint`], which makes it when
    /// something that can await asks. Nothing is lost by the split, because a ticket is
    /// the whole of what the peer said.
    fn from_magic(name: &str, address: &str) -> Option<Self> {
        if name != NAME {
            return None;
        }
        let ticket: EndpointTicket = address.parse().ok()?;
        Some(Self {
            peer: Some(ticket.into()),
            endpoint: Arc::new(Mutex::new(None)),
            link: Arc::new(arc_swap::ArcSwapOption::empty()),
        })
    }

    /// Dial the announced endpoint, greet it, and wait for the answer.
    ///
    /// The stream is opened here and first *written to* before the greeting is expected
    /// back: QUIC does not tell a peer a stream exists until data moves on it, so an
    /// opener that waited to be read would deadlock against an acceptor waiting to be
    /// told. Sending the [`Message::Ping`] is that first move, and it is the handshake's
    /// anyway.
    #[tracing::instrument(name = "msg.connect", skip_all)]
    async fn connect(&self) -> io::Result<Connection> {
        let peer = self
            .peer
            .clone()
            .ok_or_else(|| io::Error::other("this transport was never given an address"))?;
        // Named before the connection takes it: `connect` consumes the address, and the
        // label is a fact about the peer rather than about the connection.
        let label = Self::label(&peer);
        let endpoint = self.endpoint().await?;
        let conn = endpoint
            .connect(peer, ALPN)
            .await
            .map_err(|e| io::Error::other(format!("could not reach the peer: {e}")))?;
        let stream = conn
            .open_bi()
            .await
            .map_err(|e| io::Error::other(format!("could not open a stream: {e}")))?;
        self.adopt(stream).await;

        self.send(&Message::Ping(label)).await?;
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
    /// The mirror of [`Transport::connect`], and the same order reversed: the stream is
    /// accepted only once the peer has written on it, so the greeting arrives with the
    /// news that there is a stream to answer on.
    #[tracing::instrument(name = "msg.accept", skip_all)]
    async fn accept(&self) -> io::Result<Connection> {
        let endpoint = self.endpoint().await?;
        let incoming = endpoint
            .accept()
            .await
            .ok_or_else(|| io::Error::other("this endpoint stopped accepting"))?;
        let conn = incoming
            .await
            .map_err(|e| io::Error::other(format!("a peer did not complete the handshake: {e}")))?;
        let stream = conn
            .accept_bi()
            .await
            .map_err(|e| io::Error::other(format!("could not accept a stream: {e}")))?;
        self.adopt(stream).await;

        match self.recv().await? {
            // The greeting. Answering it is what completes the handshake, and until it
            // is answered the other end is still waiting.
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
            .ok_or_else(|| io::Error::other("this transport has no link yet"))?;
        let sent = link.write.lock().await.send(message.clone()).await;
        sent
    }

    /// Read one message, waiting for a whole one.
    ///
    /// Waiting is the point: a peer has to send a message, so a read that answered the
    /// instant it was asked would be asking before the peer had a chance to write.
    /// `None` means the peer finished the stream, which is the normal end of a session.
    #[tracing::instrument(name = "msg.recv", skip_all)]
    async fn recv(&self) -> io::Result<Option<Message>> {
        let link = self
            .link
            .load_full()
            .ok_or_else(|| io::Error::other("this transport has no link yet"))?;
        let heard = match link.read.lock().await.next().await {
            Some(Ok(message)) => Ok(Some(message)),
            Some(Err(e)) => Err(e),
            None => Ok(None),
        };
        heard
    }
}


#[cfg(test)]
#[path = "../tests/unit/p2p.rs"]
mod tests;
