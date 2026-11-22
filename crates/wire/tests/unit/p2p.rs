use super::*;

/// The claims every transport owes, shared with the other channel's tests.
#[path = "pending_read.rs"]
mod pending_read;

/// The announcement a host writes parses back into the peer it names.
///
/// The round trip is the whole contract between the two sides: one writes text into a
/// pane, the other reads it out of a grid, and nothing else passes between them. A
/// ticket that did not survive that is a session nothing can dial.
#[tokio::test]
async fn an_announcement_names_the_endpoint_it_came_from() {
    let host = <P2p as Transport>::new()
        .await
        .expect("an endpoint binds");
    let magic = host.to_magic();
    let (name, address) = magic
        .split_once(crate::transport::ANNOUNCE_SEPARATOR)
        .expect("an announcement is a name, a separator and an address");
    assert_eq!(name, NAME);

    let dialer = P2p::from_magic(name, address).expect("our own announcement parses");
    let dialer_addr = dialer.peer.expect("a parsed transport names its peer");
    let host_addr = host.address();
    assert_eq!(
        EndpointTicket::new(dialer_addr).to_string(),
        host_addr,
        "the ticket that was written is the ticket that was read"
    );
}

/// The second channel, through the two claims every transport owes.
///
/// A real QUIC link between two endpoints on this machine, so this is the ordering the session
/// depends on rather than a local imitation of it.
#[tokio::test]
async fn the_handshake_completes_from_both_sides() {
    pending_read::the_handshake_completes(<P2p as Transport>::new().await.expect("listen")).await;
}

#[tokio::test]
async fn a_pending_read_does_not_block_a_send() {
    pending_read::a_pending_read_does_not_block_a_send(<P2p as Transport>::new().await.expect("listen"))
        .await;
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
