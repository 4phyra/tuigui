use super::*;

/// Bind, accept, and connect against each other, and get a link.
///
/// The handshake is the part that cannot be tested by inspection: both ends wait for the
/// other, so a wrong order is a hang rather than a failure, and a hang under a timeout is
/// the only way to see it.
#[tokio::test]
async fn the_handshake_completes_from_both_sides() {
    let dir = std::env::temp_dir().join(format!("wire-unix-{}", std::process::id()));
    let path = dir.join("s.sock");
    let _ = std::fs::remove_dir_all(&dir);

    let bound = UnixSocket::new(&path);
    bound.bind().await.expect("bind");

    // The announcer: it knows the path, so it is the address a peer arrives at.
    let dialer = UnixSocket::new(&path);

    let accept = bound.accept();
    let connect = dialer.connect();
    let (accepted, dialed) = tokio::join!(accept, connect);

    let accepted = accepted.expect("the accepting side establishes");
    let dialed = dialed.expect("the dialing side establishes");

    // Both ends are links, so both can send and be read at the other end.
    dialed
        .send(&Message::Ping("hi".into()))
        .await
        .expect("send");
    assert_eq!(
        accepted.recv().await.expect("recv"),
        Some(Message::Ping("hi".into()))
    );

    let _ = std::fs::remove_dir_all(&dir);
}
