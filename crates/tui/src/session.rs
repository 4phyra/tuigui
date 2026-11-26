//! One attached client, and what it says.
//!
//! A [`Session`] is a link plus the state of the thing at the other end: the
//! client's grid, the controller that creates images, and whether one has been
//! placed. All of it is made once and updated by what arrives, which is why it is
//! a struct rather than a set of arguments.
//!
//! ```text
//!   a Frame ──▶ handle_msg ──▶ decode, transmit, place
//! ```
//!
//! Each frame replaces the image and re-places it, because a transmit under an
//! existing id frees that image's placements in the terminal.

use std::collections::HashMap;
use std::io;
use std::sync::Arc;

use codecs::CodecId;
use easytgp::{Controller, Format, ImageRequest};
use futures_retry::FutureRetry;
use tokio::sync::mpsc;
use wire::message::{FramePosition, Message, SurfaceId};
use wire::transport::Connection;

use crate::error::{ErrorInner, Result};
use crate::proxy::{Anchors, Position};
use crate::video_stream::VideoStream;
use crate::DebugOptions;

// The unit tests drive the session from the outside and need what it builds frames from.
#[cfg(test)]
pub(crate) use crate::video_stream::stacking;

/// The image id a window's frames are sent under.
///
/// Named by the window's own surface, not by a constant: two windows are two
/// pictures, and one shared id would make the second replace the first. Derived
/// rather than allocated so both ends agree without a negotiation, and so it is
/// stable across frames — a replacement under the same id is what makes a frame an
/// update rather than a new image.
#[tracing::instrument(skip_all)]
pub fn image_id(surface: &SurfaceId) -> u32 {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    surface.hash(&mut hasher);
    // A small id on purpose. The protocol allows a wider range, but a terminal does
    // not have to accept one: Ghostty keeps its own ids in the low thousands and
    // ignores a transmit above them, with no reply that says so — the picture simply
    // never appears. Two to 9999 leaves room for several windows and stays inside the
    // range every implementation keeps.
    (hasher.finish() % 9998) as u32 + 2
}

/// How often the windows are checked against the cells the scan found.
///
/// A frame is what carries a picture's placement, and a window whose pane is not on screen sends
/// no frames. The scan already knows the cells are gone; this is the clock that notices. 16ms is
/// one video frame's worth of latency, which is as fast as a human switch needs.
const RECONCILE: std::time::Duration = std::time::Duration::from_millis(16);
/// The frame that is painted when [`DebugOptions::blue_pixel`] is set.
///
/// Small and solid, because its whole purpose is to be unmistakable: anything else
/// on screen is not this, and this cannot be confused with a decoded frame.
const BLUE: (u32, u32) = (64, 64);

/// One window this session is drawing.
///
/// Generic over the medium, which is fixed when the session is built: a caller that knows its
/// terminal cannot map a shared memory object picks [`easytgp::Png`], and one that knows it can
/// picks [`easytgp::Shm`]. Neither pays for the other's branch, and the choice is a type rather
/// than a flag.
///
/// One of these per window, not per session: two windows are two pictures, and a decoder that
/// followed one stream cannot follow the other.
///
/// The window does not carry its own surface: the session keeps these in a map *keyed* by it, so
/// the identity exists once. A copy here would be a second answer to "which cells is this window",
/// and a frame naming the key would still have to search for the copy that matched.
pub struct Window<F: Format> {
    /// What the picture is doing, and the operations pending on it.
    pub picture: VideoStream<F>,
    /// The codec this window's frames are in, which is what its decoder was built from.
    pub codec: CodecId,
    /// The codec's decoder, kept across frames.
    ///
    /// Not an `Option`: a window's frames are in one codec for its whole life, so the decoder is
    /// made when the window is opened. A stream that goes bad means the decoder is *replaced*
    /// from [`Window::codec`], not that there is none.
    pub decoder: Box<dyn codecs::Decoder>,
    /// How fast this pane places this window's pictures, averaged.
    ///
    /// An EMA (alpha 1/8) of `place_frame`'s own await, seeded by the first measurement, and
    /// reported to the client as [`Message::Pace`] so it can space the next picture by it. It
    /// belongs to the window because it is that window's stream: a big picture costs more to
    /// place than a small one. `None` until one has been placed.
    pub pace: Option<std::time::Duration>,
}

impl<F: Format> Window<F> {
    /// Fold one placement's cost into the pace, and answer what it is now.
    ///
    /// Alpha 1/8: a step in service time is followed in about eight pictures, which is quicker
    /// than a user notices and slow enough not to chase frame-to-frame noise.
    #[tracing::instrument(skip_all)]
    fn observe_pace(&mut self, took: std::time::Duration) -> std::time::Duration {
        let pace = match self.pace {
            None => took,
            Some(pace) => pace.mul_f64(7.0 / 8.0) + took.mul_f64(1.0 / 8.0),
        };
        self.pace = Some(pace);
        pace
    }
}

impl<F: Format> Drop for Window<F> {
    /// The picture goes with the window.
    ///
    /// The terminal owns an image once it is transmitted, so dropping this frees nothing on the
    /// screen: the last frame stays where it was for as long as the terminal keeps it. Saying so
    /// is the only thing that removes it, and this is the one place that always runs — a window
    /// closed, a session that ends, a link that dies.
    #[tracing::instrument(skip_all)]
    fn drop(&mut self) {
        if let VideoStream::Shown(image, _) =
            std::mem::replace(&mut self.picture, VideoStream::Nothing)
        {
            image.delete();
        }
    }
}

/// One client the server has attached.
///
/// Holds its link and what belongs to the link rather than to a window, and decides
/// what to do with whatever arrives. The decision is here rather than in the
/// server's loop so the loop stays one `select!` over several clients instead of a
/// loop that also has to know what a frame means.
pub struct Session<F: Format> {
    /// The link. Every session has one, so it is not optional.
    pub connection: Connection,
    /// The diagnostic switches this session was started with.
    pub debug: std::sync::Arc<DebugOptions>,
    /// Creates image requests, and owns the reply router behind them.
    pub controller: Arc<Controller>,
    /// Every window this session is drawing, keyed by the cells it draws into.
    ///
    /// A map rather than a list, because the surface *is* how a frame names its window: every
    /// lookup was a search for the entry whose copy of the key matched the one in hand. A map
    /// answers that directly, and the identity exists once instead of once per lookup.
    ///
    /// The stacking order is not this map's business: `z` travels with each frame, so it is read
    /// from the frame it applies to rather than from a position in here. Empty until a window is
    /// opened, which is also the reason a frame can arrive with nowhere to go.
    pub windows: HashMap<SurfaceId, Window<F>>,
}

impl<F: Format> Session<F> {
    #[tracing::instrument(skip_all)]
    pub fn new(
        connection: Connection,
        debug: std::sync::Arc<DebugOptions>,
        controller: Arc<Controller>,
    ) -> Self {
        Self {
            connection,
            debug,
            controller,
            windows: HashMap::new(),
        }
    }

    /// Read messages until the link ends, which is the client leaving.
    ///
    /// One `await` per message and no return in between: the session owns itself for the whole
    /// conversation, so the server holds this as one future among several and never has to hand
    /// a session back. A `FuturesUnordered` makes sure a chatty client cannot starve a quiet one,
    /// and `?` on the read is the end of the session either way — a link that failed has nothing
    /// left to read.
    #[tracing::instrument(skip_all)]
    pub async fn run(mut self, anchors: Anchors) -> Result<()> {
        // Read by a task of its own, so each message is stamped when the socket handed it over
        // rather than when this session got around to it.
        let mut link = timestamped_connection(self.connection.clone());
        loop {
            tokio::select! {
                arrival = link.recv() => {
                    let Some((arrived, message)) = arrival else { return Ok(()) };
                    if let Err(e) = self.handle_inner(message, arrived, &anchors).await {
                        log::error!("{e}");
                        // Reported rather than returned: the link is the only channel a client listens on,
                        // and a failure it is not told about is a pane that stops changing for no stated
                        // reason. Retried because this is the last thing the peer is ever told — a report
                        // that is itself lost leaves that same silent pane.
                        let report = Message::Error(shared_utils::report::drawn(
                            miette::Report::new(e).wrap_err("the tui server failed:"),
                        ));
                        FutureRetry::new(
                            || self.connection.send(&report),
                            crate::error::ReportFailure,
                        )
                        .await
                        .unwrap_or_else(|(e, attempts)| {
                            // Not a failure of this side: a report that cannot be sent is a client that
                            // is gone, and there is nothing left to tell. Logged so the reason is findable
                            // afterwards, and the session simply ends.
                            tracing::error!(
                                "could not report a failure to the client after {attempts} attempts: {e}"
                            );
                            ((), attempts)
                        });
                    }
                }
                // A picture is placed against cells the pane drew, and a window that is off screen
                // sends no frames to notice them leaving — so this is the clock that notices. The
                // sleep is made fresh each turn: it fires 16 ms after the link has gone quiet,
                // and a frame arriving first takes the arm out of the running — frames hide
                // windows on the same rule, on their own path.
                // TODO: be triggered by the proxy (the scan that finds the cells) rather than
                // by this timer — the sleep is a stand-in until the proxy can say when a cell
                // went away or came back.
                _ = tokio::time::sleep(RECONCILE) => self.reconcile(&anchors).await,
            }
            // One frame per turn, marked once the turn's work is done: a message handled
            // or a reconcile tick, with the wait that preceded it outside the interval.
            crate::frame_mark();
        }
    }

    /// Take every window's picture down whose cells the scan cannot find, and put back the
    /// ones that are hidden but visible again.
    ///
    /// A window missing from the table, or in it without a position, is not on screen: its
    /// picture goes. A window that went away and came back gets its picture re-placed at the
    /// cells the scan now finds, without a frame from its program — a window that is not
    /// drawing sends none, and waiting for one would leave the returning pane blank forever.
    ///
    /// Both questions are the same lookup, so they are answered in one pass.
    #[tracing::instrument(skip_all)]
    async fn reconcile(&mut self, anchors: &Anchors) {
        let snapshot = anchors.load();
        for (surface, window) in &mut self.windows {
            match snapshot.get(surface) {
                Some(Some(at)) => {
                    if window.picture.is_hidden() {
                        if let Err(e) = window.picture.show(image_id(surface), *at).await {
                            tracing::error!("could not put {surface:?}'s picture back: {e}");
                        }
                    }
                }
                // Not in the table, or in it without a position: either way, not on screen.
                _ => window.picture.hide(),
            }
        }
    }

    /// One message, and what it means.
    #[tracing::instrument(skip_all)]
    async fn handle_inner(
        &mut self,
        message: Message,
        arrived: std::time::Instant,
        anchors: &Arc<arc_swap::ArcSwap<HashMap<SurfaceId, Option<Position>>>>,
    ) -> Result<()> {
        match message {
            Message::Frame {
                surface,
                position,
                bytes,
            } => {
                // Where the window is, looked up rather than tracked: nothing about position
                // travels on the wire, because the cells carrying the surface are its box — so a
                // multiplexer that moved the text moved the window without anyone saying so.
                //
                // The stacking order does travel, because nothing on the screen says which window
                // is in front of another: that is the window manager's answer, and it arrives with
                // the frame it applies to.
                match anchors
                    .load()
                    .get(&surface)
                    .ok_or(io::Error::other(format!("no anchor for {surface:?}")))?
                {
                    Some(at) => {
                        // Timed from the moment the socket handed this frame over, so the lag
                        // below — this pane's own cost, standing in for a TUI slower than the
                        // producer — and the wait in the reader's queue are both inside the
                        // number: what the client is told is how long a picture took to reach
                        // the screen, not how long one call took.
                        if let Some(ms) = self.debug.frame_lag_ms {
                            tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
                        }
                        self.place_frame(surface, *at, position, bytes.as_deref())
                            .await?;
                        // Only a frame that placed a picture has a time to report: a move places
                        // nothing to encode.
                        if bytes.is_some() {
                            if let Some(window) = self.windows.get_mut(&surface) {
                                let pace = window.observe_pace(arrived.elapsed());
                                // Clamped to what the field can carry: a pace longer than there
                                // are microseconds of it is not a number to report.
                                let micros = pace.as_micros().min(u32::MAX.into()) as u32;
                                self.connection
                                    .send(&Message::Pace { surface, micros })
                                    .await?;
                            }
                        }
                    }
                    // A window with no cells is not placed. It has scrolled away, or been covered
                    // entirely, or has not drawn yet, and all three mean the same thing to a
                    // placement: there is nothing on screen for it to go behind.
                    //
                    // Its picture is deleted on the way out. The terminal owns an image once it is
                    // transmitted, so the cells going away frees nothing: the picture stays where
                    // it was while the text that placed it has moved on. Only the image goes — the
                    // window stays, because cells that scrolled away can come back, and its next
                    // frame transmits a fresh picture under the same id.
                    None => {
                        if let Some(window) = self.windows.get_mut(&surface) {
                            window.picture.hide();
                        }
                    }
                }
            }
            Message::Error(text) => {
                tracing::warn!("a client reported: {text}");
            }
            // The handshake's own messages, which were answered when the link was established and
            // mean nothing on their own.
            Message::Ping(label) => {
                tracing::info!("a client greeted us as {label}");
            }
            Message::Pong => (),
            Message::OpenWindow { surface, codec } => {
                self.windows.insert(
                    surface,
                    Window {
                        codec,
                        picture: VideoStream::Nothing,
                        decoder: codec.to_decoder()?,
                        pace: None,
                    },
                );
                // Announced before anything is drawn: the cells carrying this surface are what the
                // rewrite blanks, and where they are is what the next scan looks for. No position
                // yet, which is the answer for a window that has not painted those cells.
                anchors.rcu(|current| {
                    let mut next = HashMap::clone(current);
                    next.insert(surface, None);
                    next
                });
                // The client waits for this before drawing its first cell. The two sides read
                // different streams, so without it a chunk of the pty carrying this window's
                // anchors can be rewritten — or rather, not rewritten — before this registration
                // exists, and cells nobody can blank are cells a user sees.
                self.connection
                    .send(&Message::WindowCreated { surface })
                    .await?;
                log::info!(
                    "opened a window on {surface:?} in {codec:?} ({} now open)",
                    self.windows.len()
                );
            }
            Message::CloseWindow { surface } => {
                match self.windows.remove(&surface) {
                    // Dropped, and dropping a window is what deletes its picture: see
                    // [`Window`]'s own `Drop`. Nothing is said here because the window is the one
                    // that knows the image's id, and it is going with it.
                    Some(window) => {
                        drop(window);
                        anchors.rcu(|current| {
                            let mut next = HashMap::clone(current);
                            next.remove(&surface);
                            next
                        });
                        log::info!(
                            "closed the window on {surface:?} ({} left)",
                            self.windows.len()
                        );
                    }
                    None => {
                        return Err(
                            io::Error::other(format!("no window draws into {surface:?}")).into(),
                        )
                    }
                }
            }
            // Not handled yet. Nothing here is a failure — the client sends these in the normal
            // course of a session — so they are reported rather than obeyed or turned into an error
            // the client would see as the end.
            other @ (Message::CompositorRequest(_)
            | Message::WindowCreated { .. }
            | Message::Pace { .. }) => {
                tracing::warn!("tui: {other:?} is not handled yet");
            }
        }

        Ok(())
    }

    /// Put a window's frame on the screen.
    ///
    /// `bytes` is the program's frame, already known to have somewhere to go: `at` is where the
    /// scan found the window's cells. `position` is the frame's own rank and box, which travel
    /// with the placement whether or not there is anything to place — a window that moved or
    /// changed rank while its program drew nothing still arrives as a frame, and the picture
    /// the terminal already holds is what belongs in the new box.
    ///
    /// One span per frame, so the profiler's timeline is divided at the same
    /// boundary the pane's is. The awaits are inside it, which is why this is
    /// `#[instrument]` rather than a held guard: `Span::enter`'s guard must not
    /// outlive an `.await`, and the attribute enters and exits per poll instead.
    #[tracing::instrument(name = "frame", skip_all, fields(surface = ?surface))]
    async fn place_frame(
        &mut self,
        surface: SurfaceId,
        at: Position,
        position: FramePosition,
        bytes: Option<&[u8]>,
    ) -> Result<()> {
        let Some(window) = self.windows.get_mut(&surface) else {
            tracing::debug!("a frame for a window this session does not know: {surface:?}");
            return Ok(());
        };
        let request = bytes
            .map(|data| Self::decode(&self.debug, &self.controller, window, surface, data))
            .transpose()?
            .flatten();
        let image_id = image_id(&surface);
        let placed = match request {
            Some(request) => window.picture.update(image_id, at, position, request).await,
            // A frame with nothing to show: the remembered frame moves with it, and the picture
            // the terminal holds is what belongs in the new box.
            None => {
                window.picture.set_frame_position(position);
                window.picture.show(image_id, at).await
            }
        };
        placed
    }

    /// Decode one frame's worth of bytes into a request ready to transmit.
    ///
    /// Answers `Ok(None)` for a frame with nothing to place: codec headers, a frame the codec
    /// held back, or one it failed on. A failed codec is replaced from its own id, so the next
    /// frame starts from a clean state; the stored picture is left alone, because a window
    /// whose program stopped drawing keeps whatever it last showed.
    #[tracing::instrument(skip_all)]
    fn decode(
        debug: &DebugOptions,
        controller: &Controller,
        state: &mut Window<F>,
        surface: SurfaceId,
        data: &[u8],
    ) -> Result<Option<ImageRequest<F>>> {
        let pixels = if debug.blue_pixel {
            // The frame is ignored on purpose, so nothing in the wire path, the codec
            // or the decode is exercised: what appears on screen is attributable to
            // the encode, the transmit and the placement alone.
            image::RgbImage::from_pixel(BLUE.0, BLUE.1, image::Rgb([0, 0, 255]))
        } else {
            match state.decoder.decode(data) {
                Ok(Some(frame)) => image::RgbImage::from_raw(frame.width, frame.height, frame.rgb)
                    .ok_or(ErrorInner::FrameSize {
                        width: frame.width,
                        height: frame.height,
                    })?,
                // Headers, or a frame the codec held back: nothing to show yet,
                // and nothing to ask for — the next frame the client sends is what
                // the decoder held, and a static window sends again when it moves.
                Ok(None) => return Ok(None),
                // The client is not asked for a keyframe because a client cannot be asked: every
                // message it reads has to be one this side can answer for, and a request it cannot
                // answer is a stall.
                Err(e) => {
                    state.decoder = state.codec.to_decoder()?;
                    tracing::error!("the codec failed: {e}");
                    return Ok(None);
                }
            }
        };
        match F::request(controller, pixels) {
            Ok(request) => Ok(Some(request)),
            Err(source) => Err(crate::error::Error::at(ErrorInner::Stage {
                source,
                surface,
            })),
        }
    }
}

/// The link, read by a task of its own, with every message stamped by the moment the socket
/// handed it over.
///
/// An arrival has to be stamped when it arrives, and a session spends its turns placing and
/// encoding: reading between those turns would stamp the session's own busy time as the arrival,
/// and a pane that cannot tell its work from the link's cannot report either.
///
/// The link ending is the channel closing, and a failure is the same end with its reason said
/// here: what the session has to say about the link is either a message or nothing.
type TimestampedConnection = mpsc::Receiver<(std::time::Instant, Message)>;

/// Read the link in a task of its own.
///
/// Parked there, the read is woken by the socket and stamps what it was given. Bounded, so a
/// client that runs ahead is held in the socket's own queue rather than in this process's
/// memory. The task ends when the receiver is dropped — the session's own end — because a send
/// to nobody is the answer that it is over.
fn timestamped_connection(connection: Connection) -> TimestampedConnection {
    // Enough for a couple of windows to be two or three pictures ahead, and no more: past that
    // the socket is the better place for a queue, because it is bounded without us.
    const HELD: usize = 8;

    let (sender, messages) = mpsc::channel(HELD);
    tokio::spawn(async move {
        loop {
            let message = match connection.recv().await {
                Ok(Some(message)) => message,
                // The link ended, and the channel closing is how the session hears it.
                Ok(None) => break,
                // Named here rather than carried: the session's end is the same either way, and
                // an end nobody named is a pane that stops changing for no stated reason.
                Err(e) => {
                    tracing::error!("the link failed: {e}");
                    break;
                }
            };
            if sender
                .send((std::time::Instant::now(), message))
                .await
                .is_err()
            {
                break;
            }
        }
    });
    messages
}

#[cfg(test)]
#[path = "../tests/unit/session.rs"]
mod tests;
