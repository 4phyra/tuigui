//! The session this client composites, and how its pictures leave.
//!
//! The compositor places windows and hands them events; it does not know what a
//! frame becomes. This is the half that does: the session the program runs in —
//! one per platform — and the link its pictures are written to.
//!
//! The session is a type alias rather than a parameter because this client has
//! exactly one on any given machine. The choice lives here, so nothing above has
//! to be written again for a second one.

#[cfg(target_os = "linux")]
pub mod wayland;

/// The session a program on this machine runs in.
#[cfg(target_os = "linux")]
pub type PlatformWindowing = wayland::WaylandServer;

/// How long one message is given to leave this side.
///
/// A message is a write to another process, which may be busy with the one
/// before it. Bounded so a link that stopped draining is reported rather than
/// waited on: without it the frame stalls with the message in hand and says
/// nothing, which reads as a frozen pane.
pub const SEND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Write one message to the link from a draw, which is synchronous.
///
/// The blocking is what `block_in_place` makes safe: the draw runs on a runtime
/// worker, and that call is the one that says so. Sending here rather than
/// pushing the write onto the runtime keeps messages in the order the frames
/// that produced them were made.
///
/// FIXME: this should go. A draw that blocks on a socket is a draw waiting for
/// another process, and the frame loop should hand its messages over instead of
/// writing them itself.
#[tracing::instrument(name = "windowing.send", skip(link, message))]
pub fn send(
    link: &wire::transport::Connection,
    message: &wire::message::Message,
) -> Result<(), compositor::Error> {
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            tokio::time::timeout(SEND_TIMEOUT, link.send(message))
                .await
                .map_err(|_| {
                    compositor::Error::Frame(format!(
                        "the link took nothing within {SEND_TIMEOUT:?}"
                    ))
                })?
                .map_err(|e| compositor::Error::Frame(e.to_string()))
        })
    })
}
