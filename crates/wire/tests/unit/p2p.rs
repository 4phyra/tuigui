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
