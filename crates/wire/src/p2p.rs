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
use crate::transport::{framed_read, framed_write, BoxFuture, Connection, MessageCodec, Transport};

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
    link: Arc<Mutex<Option<Link>>>,
}

/// The connected half: the two directions of one bidirectional QUIC stream.
///
/// No split here, unlike a unix socket: QUIC hands out the two directions separately, so
/// a read and a write are already different objects and neither holds up the other.
struct Link {
    read: FramedRead<iroh::endpoint::RecvStream, MessageCodec>,
    write: FramedWrite<iroh::endpoint::SendStream, MessageCodec>,
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
            link: Arc::new(Mutex::new(None)),
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
        *self.link.lock().await = Some(Link {
            read: framed_read(recv),
            write: framed_write(send),
        });
    }

    /// What this transport calls itself, for the greeting.
    ///
    /// The key, because that is what identifies the endpoint at the other end: an address
    /// is how to reach it, and two addresses can name one endpoint.
    fn label(addr: &EndpointAddr) -> String {
        EndpointTicket::new(addr.clone()).to_string()
    }
}

impl Transport for P2p {
    fn name(&self) -> &'static str {
        NAME
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
            link: Arc::new(Mutex::new(None)),
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
    fn connect(&self) -> BoxFuture<'_, io::Result<Connection>> {
        Box::pin(async move {
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
        })
    }

    /// Wait for a peer to dial, answer its greeting, and return the link.
    ///
    /// The mirror of [`Transport::connect`], and the same order reversed: the stream is
    /// accepted only once the peer has written on it, so the greeting arrives with the
    /// news that there is a stream to answer on.
    #[tracing::instrument(name = "msg.accept", skip_all)]
    fn accept(&self) -> BoxFuture<'_, io::Result<Connection>> {
        Box::pin(async move {
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
        })
    }

    #[tracing::instrument(name = "msg.send", skip_all)]
    fn send<'a>(&'a self, message: &'a Message) -> BoxFuture<'a, io::Result<()>> {
        Box::pin(async move {
            let mut link = self.link.lock().await;
            let link = link
                .as_mut()
                .ok_or_else(|| io::Error::other("this transport has no link yet"))?;
            link.write.send(message.clone()).await
        })
    }

    /// Read one message, waiting for a whole one.
    ///
    /// Waiting is the point: a peer has to send a message, so a read that answered the
    /// instant it was asked would be asking before the peer had a chance to write.
    /// `None` means the peer finished the stream, which is the normal end of a session.
    #[tracing::instrument(name = "msg.recv", skip_all)]
    fn recv(&self) -> BoxFuture<'_, io::Result<Option<Message>>> {
        Box::pin(async move {
            let mut guard = self.link.lock().await;
            let link = guard
                .as_mut()
                .ok_or_else(|| io::Error::other("this transport has no link yet"))?;
            match link.read.next().await {
                Some(Ok(message)) => Ok(Some(message)),
                Some(Err(e)) => Err(e),
                None => Ok(None),
            }
        })
    }

    /// Read one message, but do not wait for one.
    ///
    /// Polled straight through to the codec, which is where a half-arrived message is kept:
    /// nothing is created here, so having nothing to read leaves what has been read where it
    /// was. That is what makes this different from cancelling [`Transport::recv`].
    ///
    /// `None` for a quiet link. A busy lock is reported as quiet rather than waited on:
    /// there is one reader per connection, so a lock this caller cannot take is a message
    /// another reader is in the middle of, not one about to be lost.
    fn recv_next(&self) -> io::Result<Option<Message>> {
        let mut guard = match self.link.try_lock() {
            Ok(guard) => guard,
            Err(_) => return Ok(None),
        };
        let link = guard
            .as_mut()
            .ok_or_else(|| io::Error::other("this transport has no link yet"))?;
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        match link.read.poll_next_unpin(&mut context) {
            std::task::Poll::Ready(Some(r)) => r.map(Some),
            std::task::Poll::Ready(None) => Ok(None),
            std::task::Poll::Pending => Ok(None),
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    /// The announcement a host writes parses back into the peer it names.
    ///
    /// The round trip is the whole contract between the two sides: one writes text into a
    /// pane, the other reads it out of a grid, and nothing else passes between them. A
    /// ticket that did not survive that is a session nothing can dial.
    #[tokio::test]
    async fn an_announcement_names_the_endpoint_it_came_from() {
        let host = P2p::host().await.expect("an endpoint binds");
        let magic = host.to_magic().expect("a bound endpoint has an address");
        let (name, address) = magic
            .split_once(crate::transport::ANNOUNCE_SEPARATOR)
            .expect("an announcement is a name, a separator and an address");
        assert_eq!(name, NAME);

        let dialer = P2p::from_magic(name, address).expect("our own announcement parses");
        let dialer_addr = dialer.peer.expect("a parsed transport names its peer");
        let host_addr = host.address().expect("a bound endpoint names itself");
        assert_eq!(
            EndpointTicket::new(dialer_addr).to_string(),
            host_addr,
            "the ticket that was written is the ticket that was read"
        );
    }

    /// A name that is not ours is not an error, it is somebody else's announcement.
    #[test]
    fn another_transport_is_not_parsed_as_this_one() {
        assert!(P2p::from_magic(crate::unix::NAME, "/run/user/1000/tuigui.sock").is_none());
    }

    /// Garbage behind our name is refused rather than guessed at.
    #[test]
    fn a_ticket_that_is_not_a_ticket_is_refused() {
        assert!(P2p::from_magic(NAME, "not-a-ticket").is_none());
        assert!(P2p::from_magic(NAME, "").is_none());
    }
}
