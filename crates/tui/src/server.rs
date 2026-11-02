//! The image server: a client tells it where it is, and it dials.
//!
//! The server does **not** listen. The client owns the channel and announces it; this side's
//! only job is to notice the announcement and dial back.
//!
//! ```text
//!   pty output ──▶ scan ──▶ a transport ──▶ connect ──▶ Ping/Pong ──▶ frames
//! ```
//!
//! # One loop, two things to wait on
//!
//! The pty's output, where a client announces itself, and the clients already attached,
//! whose messages arrive whenever they do. Neither can wait on the other, so they are one
//! `select!`.
//!
//! The connect is in the arm that found the announcement, because that is where the
//! transport is: it was just built from the text a client wrote, and it is the only thing
//! that knows how to reach it. [`Transport::connect`] is what dials *and* greets, so what
//! comes back is a peer that has answered rather than a socket that opened.

use std::io;

use std::sync::Arc;

use easytgp::{Controller, Format};
use wire::message::SurfaceId;

use crate::scanner::Scanner;
use crate::session::Session;
use crate::DebugOptions;

/// How long to wait between attempts when dialing a named address.
///
/// Long enough not to be a busy loop on a client that is not up yet, and short
/// enough that a session starting does not feel like it is waiting. The retry
/// exists because the client's own start and the server's can be in either order.
/// established: the handshake happens inside [`Transport::connect`], so a connection only
/// exists once a peer has answered the greeting, and there is no half-attached client to be
/// told apart from a whole one.
pub struct Server<F: Format> {
    /// The diagnostic switches, passed to every session it attaches.
    debug: std::sync::Arc<DebugOptions>,
    /// Creates image requests, and owns the reply router behind them.
    ///
    /// On the server and not in a session because there is one terminal: the reply router reads
    /// the one keyboard descriptor and the image namespace is the terminal's, so both are per
    /// process and both outlive any client. It also owns where escapes are written, which is the
    /// screen this server shares with the filter.
    controller: Arc<Controller>,
    /// Every client attached to this session.
    ///
    /// Owned by the loop and nowhere else: reading a link needs it, so a link in a map beside the
    /// loop reading it would be a link in two places.
    sessions: Vec<Session<F>>,
    /// What the command drew, as the terminal should see it.
    ///
    /// Owned, because the rewriting carries state between chunks — half a character, half a
    /// sequence — and carrying it is the whole point of it being one object.
    filter: crate::filter::Filter,
    /// The terminal the filter rewrites against.
    ///
    /// `CellMap::map` asks a terminal for the colour in effect at each cell, so it must be fed the
    /// same bytes the rewrite walks or the two disagree about what is on screen. Nobody else reads
    /// it: the scan keeps its own, because it reads the stream before the rewrite.
    terminal: vt100::Parser,
    /// What is on the screen, as the command drew it.
    scanner: Scanner,
}

impl<F: Format> Server<F> {
    /// Serve a pty's output, with the reply router this side's own escapes need.
    ///
    /// The grid comes from this terminal and nowhere else: an image is placed at a cell, so the
    /// number of cells has to be the one the terminal actually has. A terminal that will not report
    /// one is a failure rather than a guess — a session sized 24x80 on a real screen is a picture
    /// placed at the wrong cell, which looks like a bug in the placement rather than in this call.
    pub fn new(
        debug: std::sync::Arc<DebugOptions>,
        controller: Arc<Controller>,
    ) -> io::Result<Self> {
        // The same size the scan uses, read once: both are fed this terminal's own bytes, and two
        // sizes would be two answers to what fits on the screen.
        let ws = rustix::termios::tcgetwinsize(std::io::stdin()).map_err(io::Error::from)?;
        let rows = std::num::NonZeroU16::new(ws.ws_row)
            .ok_or_else(|| io::Error::other("this terminal reports no rows"))?;
        let cols = std::num::NonZeroU16::new(ws.ws_col)
            .ok_or_else(|| io::Error::other("this terminal reports no columns"))?;
        Ok(Self {
            debug,
            controller,
            sessions: Vec::new(),
            filter: crate::filter::Filter::default(),
            terminal: vt100::Parser::new(rows, cols, 0),
            scanner: Scanner::new(rows, cols),
        })
    }

    /// How many clients are attached.
    pub fn clients(&self) -> usize {
        self.sessions.len()
    }

    /// Serve the command's output until it ends, which is the command exiting.
    ///
    /// One step per chunk. It ends where the stream does: a pty master reads as closed once the
    /// command and every process holding its slave are gone.
    pub async fn run(
        mut self,
        master_r: &mut (impl tokio::io::AsyncBufRead + Unpin),
        sink_w: &mut impl std::io::Write,
    ) -> io::Result<()> {
        // Where the windows were, as of the last pass that read any output. A pass with nothing to
        // read learns nothing about the screen, so what the last one found is still the answer.
        let mut positions = std::collections::HashMap::new();
        loop {
            match wire::drain::drain_reader_async(master_r).await? {
                // Output to rewrite, a screen to scan, and possibly a client to dial.
                Some(master_bytes) if !master_bytes.is_empty() => {
                    let master_bytes_chunk = master_bytes.as_slice();

                    // Every window's cells, worked out once and used by both the filter and the scan:
                    // they are the same question — which cells belong to a window — asked of the same
                    // list.
                    let surfaces: Vec<SurfaceId> = self
                        .sessions
                        .iter()
                        .flat_map(|session| session.windows.keys().copied())
                        .collect();

                    // What the terminal is written: the same bytes with every window's cells blanked,
                    // so the picture behind them shows through.
                    self.filter
                        .feed(master_bytes_chunk, &mut self.terminal, sink_w, &surfaces)
                        .await?;

                    // And the screen the scan reads, which is the command's own drawing rather than
                    // the rewritten stream: a window's cells are found where it drew them.
                    self.scanner.feed(master_bytes_chunk);
                    positions = self.scanner.scan_positions(&surfaces);

                    // A client that cannot be dialed says so by drawing its address, and the scan is
                    // what turns that into a channel.
                    //
                    // One client at a time, for now. The scan reports the longest run of braille on
                    // the screen, which is one announcement, so there is one address to dial and one
                    // session to make of it. The wait is local rather than stored: a future kept in a
                    // field is a future holding the link's lock across steps, which starved every
                    // read of it.
                    //
                    // TODO: several panes are several announcements on one screen, so this is where
                    // it stops being one. `scan_barcode` has to report every run it parses rather
                    // than the longest, a session has to carry the announcement it came from, and the
                    // dial has to skip the ones already attached — a client that has answered is no
                    // longer listening, so greeting it again is a wait nothing ends.
                    if self.sessions.is_empty() {
                        if let Some(announced) = self.scanner.scan_barcode() {
                            match announced.connect().await {
                                Ok(connection) => self.sessions.push(Session::new(
                                    connection,
                                    self.debug.clone(),
                                    self.controller.clone(),
                                )),
                                Err(e) => {
                                    tracing::warn!("could not reach an announced client: {e}")
                                }
                            }
                        }
                    }
                }
                // Nothing waiting. The screen is untouched, so the scan of it is skipped: walking
                // every cell to rediscover where the windows already are is the most expensive thing
                // in here, and a quiet command would pay it on every pass.
                Some(_) => {}
                // The pty is closed, which is the command exiting.
                None => return Ok(()),
            }

            // Every session's turn, with the positions they share. Read by reference, so the sessions
            // could run at once: a surface belongs to one window, so a lookup cannot be routed twice.
            //
            // A failure ends the session: whatever went wrong inside a client's link is nothing
            // this loop can recover from, and a pane that keeps drawing over a broken link is a
            // pane that hides the failure.
            for session in &mut self.sessions {
                session.handle(&positions, sink_w).await?;
            }

            // Handed back to the runtime at the end of every pass, whatever the pass did. Nothing
            // above can park this loop: the read answers with whatever is waiting instead of waiting
            // for more, and a quiet client's `recv_next` answers too. A loop that only ever returns
            // `Ready` never gives the runtime its thread back, so the `select!` that drives this
            // against the command's own half — the arm that reads `SIGWINCH` — is never polled
            // again, and a resize goes unnoticed while the pane keeps its old shape.
            tokio::task::yield_now().await;
        }
    }
}
