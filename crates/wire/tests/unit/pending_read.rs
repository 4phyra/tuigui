//! What every transport must do, written once and run by each of them.
//!
//! A transport is a channel plus a handshake, and the claims about it are about the trait rather
//! than about any one channel. Each transport's own file is then only the reach: which type it
//! hands to these functions, and nothing about what is expected of it.
//!
//! Reading blocks — the trait says so, because a peer has to send something — while writing must
//! not wait for it. A session does both at once: one side sits in `recv` while the windows it
//! serves write frames down the same link. A transport that takes one lock for both deadlocks the
//! moment both are in use, and a deadlocked link is not a slow pane but a frozen one: no frame, no
//! keystroke, no close request. That is the second claim here, and it is why this file exists.

use crate::message::Message;
use crate::transport::{Connection, Transport};
use std::time::Duration;

/// How long a step may take.
///
/// Generous on purpose: the claims are that each step completes at all, so a loaded machine — a
/// CI box starting a QUIC endpoint, say — must not be what makes them fail. Bounded all the same,
/// because the failure these tests are for is a link that waits for itself: an unbounded wait
/// would turn that into a suite that never ends.
const PATIENCE: Duration = Duration::from_secs(5);

/// Two ends of one established link, made the way a session makes one.
///
/// `host` is a transport already listening at its own address, which the caller made: this side
/// announces itself with `to_magic` and the dialer is whatever `from_magic` reads that back as. So
/// this is the path a real session uses, not a shortcut past it.
pub async fn linked<T: Transport + 'static>(host: T) -> (Connection, Connection) {
    let connected = async {
        let magic = host.to_magic().expect("a hosted transport announces itself");
        let (name, address) = magic
            .split_once(crate::transport::ANNOUNCE_SEPARATOR)
            .expect("an announcement is a name, a separator and an address");
        let dialer = T::from_magic(name, address).expect("our own announcement reads back");

        tokio::join!(host.accept(), dialer.connect())
    };
    let (accepted, dialed) = tokio::time::timeout(PATIENCE, connected)
        .await
        .expect("the handshake completes: a link that waits for itself would hang here");
    (
        accepted.expect("the accepting side establishes"),
        dialed.expect("the dialing side establishes"),
    )
}

/// Both ends of a link are established, and each can be read by the other.
pub async fn the_handshake_completes<T: Transport + 'static>(host: T) {
    let (accepted, dialed) = linked(host).await;

    let sent = async {
        dialed
            .send(&Message::Ping("hi".into()))
            .await
            .expect("send");
        accepted.recv().await.expect("recv")
    };
    assert_eq!(
        tokio::time::timeout(PATIENCE, sent)
            .await
            .expect("a message crosses the link"),
        Some(Message::Ping("hi".into())),
        "a message sent on an established link is what the other end reads"
    );
}

/// A read with nothing to read must not stop a write on the same link.
///
/// A session does both at once: one side sits in `recv` while the windows it serves write frames
/// down the same link. That is one `Connection`, cloned — so a transport that takes one lock for
/// both directions deadlocks the moment both are in use, and the peer at the other end never sees
/// either. The two ends here are named for their roles in the test, not by which half of the
/// channel they are.
pub async fn a_pending_read_does_not_block_a_send<T: Transport + 'static>(host: T) {
    let (ours, theirs) = linked(host).await;

    // A read with nothing arriving, which is the state a session is in most of the time. On a
    // clone of the connection that is about to write, because that is the pair that deadlocks.
    let writing = ours.clone();
    let mut waiting = std::pin::pin!(ours.recv());
    let first = futures_util::poll!(&mut waiting);
    assert!(
        first.is_pending(),
        "nothing has been sent yet, so the read waits: {first:?}"
    );

    let sent = tokio::time::timeout(PATIENCE, writing.send(&Message::Pong)).await;
    let sent =
        sent.expect("a write waited on a read that was waiting on it: the link is deadlocked");
    sent.expect("the write completes");

    // And the peer reads what was written, which is the other half of the same claim: the two
    // directions are one stream, not two.
    let heard = tokio::time::timeout(PATIENCE, theirs.recv()).await;
    assert_eq!(
        heard.expect("the peer reads").expect("the peer"),
        Some(Message::Pong),
        "a message written while a read was pending is what the peer reads"
    );

    // The pending read is still a read: what the peer sends now is what it returns.
    theirs
        .send(&Message::Ping("back".into()))
        .await
        .expect("send");
    let read = tokio::time::timeout(PATIENCE, waiting).await;
    assert_eq!(
        read.expect("the pending read completes").expect("the read"),
        Some(Message::Ping("back".into())),
        "the read that waited through the write is still the next message"
    );
}
