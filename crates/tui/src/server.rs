//! The image server: a client tells it where it is, and it dials.
//!
//! The server does **not** listen. The client owns the channel and announces it; this side's
//! only job is to notice the announcement and dial back.
//!
//! ```text
//!   a barcode ──▶ dial ──▶ Ping/Pong ──▶ frames
//! ```
//!
//! # One loop, two things to wait on
//!
//! A client announcing itself, and the clients already attached. Neither can wait on the other,
//! so they are one `select!`: announcements arrive on a channel the proxy sends into, and every
//! attached client is a future in a `FuturesUnordered`. Whichever finishes first is awaited
//! directly, so no session is polled in turn behind another one's wait.
//!
//! A session's future is its whole conversation — it loops on its own reads — so this loop never
//! re-arms one. What comes out of the set is the end of a session, not one to put back.

use std::io;
use std::sync::Arc;

use easytgp::{Controller, Format};
use wire::transport::Transport;

use crate::session::Session;
use crate::DebugOptions;

/// The image server: every attached client, and the dial for the next one.
///
/// The medium is not a parameter of the server, because no client exists when it is built: a
/// session is made inside the dial that reached its peer, and the medium is chosen by whoever
/// started the server — so it is `run`'s type parameter rather than the struct's.
pub struct Server {
    /// The diagnostic switches, passed to every session it attaches.
    debug: std::sync::Arc<DebugOptions>,
    /// Creates image requests, and owns the reply router behind them.
    ///
    /// On the server and not in a session because there is one terminal: the reply router reads
    /// the one keyboard descriptor and the image namespace is the terminal's, so both are per
    /// process and both outlive any client.
    controller: Arc<Controller>,
    /// The surfaces a window is drawing into, each with the cell it was found at.
    ///
    /// Shared with the proxy, which is the side that reads the command's bytes: it scans them and
    /// replaces this map, and the rewrite blanks the cells in it. A session reads it to place a
    /// frame, so nothing about a position travels on the wire — a multiplexer that moved the text
    /// moved the window, and the next scan says so.
    anchors: crate::proxy::Anchors,
}

impl Server {
    /// A server over the clients that announce themselves to the proxy.
    ///
    /// The grid does not come from here: the scan that finds where a window is belongs to the
    /// proxy, which is the half holding the command's bytes.
    pub fn new(
        debug: std::sync::Arc<DebugOptions>,
        controller: Arc<Controller>,
        anchors: crate::proxy::Anchors,
    ) -> Self {
        Self {
            debug,
            controller,
            anchors,
        }
    }

    /// Dial whoever announces itself, and serve every client until the command's output ends.
    pub async fn run<F: Format + Send + Sync>(
        self,
        mut announcements: tokio::sync::mpsc::Receiver<Box<dyn Transport>>,
    ) -> io::Result<()> {
        // Every client is its own task, so one waiting on a terminal's acknowledgement cannot
        // delay another's frames. `JoinSet` rather than a set of futures: a spawned task is
        // scheduled by the runtime instead of being polled in turn here.
        let mut attached = tokio::task::JoinSet::new();

        loop {
            tokio::select! {
                announced = announcements.recv() => match announced {
                    // The dial is spawned, not awaited: reaching a peer over a relay is seconds of
                    // another machine's time, and the attached clients have to keep being read
                    // while it happens.
                    Some(transport) => {
                        let (debug, controller) = (self.debug.clone(), self.controller.clone());
                        let anchors = self.anchors.clone();
                        attached.spawn(async move {
                            let connection = match transport.connect().await {
                                Ok(connection) => connection,
                                // Reported rather than fatal: the announcement is still on the
                                // screen, so the next chunk offers the same client again, and
                                // failing to reach one that is still starting is the ordinary case.
                                Err(e) => {
                                    tracing::warn!("could not reach an announced client: {e}");
                                    return;
                                }
                            };
                            let session = Session::<F>::new(connection, debug, controller);
                            // A link that failed is nothing this loop can recover from, and a pane
                            // that keeps drawing over it hides the failure.
                            if let Err(e) = session.run(anchors).await {
                                tracing::error!("a client's session ended: {e}");
                            }
                        });
                    },
                    // Nothing can announce itself again. Every attached client is still worth
                    // reading, so the loop waits on those alone.
                    None => {
                        while attached.join_next().await.is_some() {}
                        return Ok(());
                    }
                },
                // A session ended. Collected so a panic in one is not silently dropped: the
                // runtime reports it here rather than taking the whole server down.
                Some(joined) = attached.join_next(), if !attached.is_empty() => {
                    if let Err(e) = joined {
                        tracing::error!("a client's task ended: {e}");
                    }
                }
            }
        }
    }
}
