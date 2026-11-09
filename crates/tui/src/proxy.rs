//! The command's side of a session: its output, on its way to the screen.
//!
//! ```text
//!   pty output ──▶ filter ──▶ this terminal   (a window's cells blanked)
//!              └─▶ scanner ─▶ where the windows are, and who announced itself
//! ```
//!
//! Two readers of the same bytes, because they ask different questions of them: the filter wants
//! the stream as it will be drawn, and the scan wants the screen as the command drew it. One is
//! the terminal's answer, the other is the command's, and a window is placed where the command put
//! it rather than where the rewrite puts it.

pub mod filter;
pub mod scanner;

use std::collections::HashMap;
use std::sync::Arc;

use arc_swap::ArcSwap;
use futures_util::StreamExt as _;
use tokio::sync::mpsc;
use wire::message::SurfaceId;
use wire::transport::Transport;

pub use filter::Filter;
pub use scanner::{Position, Scanner};

/// Every window on the screen: which surfaces exist, and where each was last found.
///
/// Shared rather than passed because the two halves run apart: the rewrite reads it for every chunk
/// of the command's output, a session reads it to place a frame, and neither can wait for the
/// other. An `ArcSwap` makes the handover a pointer swap on one side and a load on the other.
///
/// `None` is a surface whose cells are not on the screen: it has not drawn yet, or has scrolled
/// away, or is covered. That is not the same as a surface that does not exist, which is why the
/// value is an `Option` rather than the key being absent.
pub type Anchors = Arc<ArcSwap<HashMap<SurfaceId, Option<Position>>>>;

/// Forward the command's output until it ends, which is the command exiting.
///
/// `slave_stdout` is the pty master's read side, as the chunks it produces. Everything read from
/// it is rewritten onto this terminal with the anchors' cells blanked, and fed to the scan, which
/// is what turns a client's drawing into the channel it announces.
///
/// An announcement is sent as it is found. The dial is the far end's time to answer, and waiting
/// for it here would freeze the pane for as long as reaching the peer took.
pub async fn run(
    mut slave_stdout: impl futures_util::Stream<Item = std::io::Result<Vec<u8>>> + Unpin,
    anchors: Anchors,
    new_barcodes: mpsc::Sender<Box<dyn Transport>>,
) -> std::io::Result<()> {
    // Each holds its own model of the screen: the filter the stream as it will be drawn, the scan
    // the screen as the command drew it. One is the terminal's answer and the other is the
    // command's, and a window is placed where the command put it rather than where the rewrite does.
    let mut filter = Filter::new()?;
    let mut scanner = Scanner::new()?;
    let mut sink = std::io::stdout();

    // Held across turns rather than made per chunk: a resize that arrived while the last chunk was
    // being rewritten belongs to this future, and one made afresh each time would be a subscriber
    // registered after the change it was waiting for.
    let mut resized = Box::pin(shared_utils::resize::resized());

    loop {
        tokio::select! {
            next = slave_stdout.next() => {
                // `transpose` keeps the two answers apart: a finished stream ends the session, and a
                // failed read ends it with the reason it failed.
                let Some(next) = next.transpose()? else {
                    return Ok(());
                };

                // Read before the rewrite: a window opened by a message handled in between belongs
                // to the next chunk, and blanking cells this chunk never drew would be blanking the
                // wrong ones.
                let known: Vec<SurfaceId> = anchors.load().keys().copied().collect();

                filter
                    .feed(next.as_slice(), &mut sink, &known)
                    .await?;
                scanner.feed(&next);

                // Where each known window is now, merged into what the server published: this side
                // knows the cells, the server knows which windows exist. A surface the scan cannot
                // find keeps no position — it has scrolled away or been covered, and there is
                // nothing to place.
                let found = scanner.scan_positions(&known);
                anchors.rcu(|current| {
                    let mut next = HashMap::clone(current);
                    for surface in &known {
                        next.insert(*surface, found.get(surface).copied());
                    }
                    next
                });

                // A client that cannot be dialed says so by drawing its address, and the scan is
                // what turns that into a channel.
                if let Some(announced) = scanner.scan_barcode() {
                    new_barcodes
                        .send(announced)
                        .await
                        .map_err(|_| std::io::Error::other("the server is no longer listening"))?;
                }
            }
            // The terminal's own grid changed. Both models of the screen follow it, because the
            // command is told its new size by the pty and draws for that one: a parser left at the
            // old bounds has columns the rewrite cannot blank and the scan cannot search.
            size = &mut resized => {
                let ws = size?;
                let (Some(rows), Some(cols)) = (
                    std::num::NonZeroU16::new(ws.ws_row),
                    std::num::NonZeroU16::new(ws.ws_col),
                ) else {
                    return Err(std::io::Error::other("the terminal reports no cells after a resize"));
                };
                filter.resize(rows, cols);
                scanner.resize(rows, cols);
                resized = Box::pin(shared_utils::resize::resized());
            }
        }
    }
}
