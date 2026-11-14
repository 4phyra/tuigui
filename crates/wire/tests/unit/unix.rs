use super::*;

/// The claims every transport owes, shared with the other channel's tests.
#[path = "pending_read.rs"]
mod pending_read;

#[tokio::test]
async fn the_handshake_completes_from_both_sides() {
    pending_read::the_handshake_completes(UnixSocket::new().await.expect("listen")).await;
}

#[tokio::test]
async fn a_pending_read_does_not_block_a_send() {
    pending_read::a_pending_read_does_not_block_a_send(UnixSocket::new().await.expect("listen"))
        .await;
}
