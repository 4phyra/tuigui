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
use easytgp::{Controller, DynImage, Format, ImageRequest, PlacementRequest};
use wire::message::{Message, SurfaceId};
use wire::transport::Connection;

use crate::error::{Error, Result};
use crate::proxy::Position;
use crate::DebugOptions;

/// This file, compiled in, so a label can quote the line it points at.
///
/// miette renders a snippet from whatever `SourceCode` the error carries; it does not read
/// files. Compiling the text in is what makes the quoted line the line that failed rather
/// than whatever is on disk when the report is printed.
pub(crate) const SESSION_SOURCE: &str = include_str!("session.rs");

/// The image id a window's frames are sent under.
///
/// Named by the window's own surface, not by a constant: two windows are two
/// pictures, and one shared id would make the second replace the first. Derived
/// rather than allocated so both ends agree without a negotiation, and so it is
/// stable across frames — a replacement under the same id is what makes a frame an
/// update rather than a new image.
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

/// The z index a window's picture is drawn at.
///
/// Strictly below `INT32_MIN / 2`, which is the band `docs/kitty-graphics-protocol.rst` says is
/// drawn under cells with non-default backgrounds as well as under the text. A window's box is
/// full of non-default backgrounds — the client draws them to mark where the picture goes — so
/// anything at or above that boundary would be covered by the very cells it is placed behind.
///
/// The order comes from the gui, which is the side with a window manager: it sends a rank per
/// frame, `0` being furthest away and larger being closer to the user. That rank is shifted into
/// the band, so the order is the gui's and only the position is this side's business.
fn stacking(z: u32) -> i32 {
    // The band is the whole lower half of the range, so a rank large enough to saturate is not one
    // the gui can produce: the furthest it could go is the front of the band, which is where
    // saturating puts it.
    const FLOOR: i32 = i32::MIN;
    const CEILING: i32 = i32::MIN / 2 - 1;
    let rank = i32::try_from(z).unwrap_or(i32::MAX);
    FLOOR.saturating_add(rank).min(CEILING)
}

///
/// Distinct from [`IMAGE_ID`] because the two are different facts: the image is
/// the pixels, the placement is the box they are fitted into.
pub const PLACEMENT_ID: u32 = 98;

/// How long to wait for a terminal to acknowledge an image transmit.
///
/// A terminal that never answers is a terminal that will never draw the frame, so
/// waiting is pointless past the point where a working one would have replied.
/// Generous rather than tight: an acknowledgement is one short escape, and the cost
/// of being wrong is failing a session on a terminal that was merely slow.
const TRANSMIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

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
/// What a window's picture is doing.
///
/// An `Option` could not say the difference that matters. A window that has not drawn has
/// no picture anywhere; a window whose cells went away has one — the pixels are still here,
/// the terminal was told to drop it — and coming back has to put it up again. Folded into
/// `None`, the second was treated as the first, and the returning pane stayed blank until
/// the program was given a reason to draw.
pub enum Picture<F: Format> {
    /// Nothing drawn yet, so there is nothing to place: every window starts here.
    Nothing,
    /// On the terminal, under the window's own id.
    ///
    /// Kept across frames rather than rebuilt, because it owns the unacknowledged transmits
    /// and placements: a frame adds to it and `sync` is what waits for those to land.
    Shown(DynImage<F>),
    /// Taken off the terminal, with the request that can put it back.
    ///
    /// The request is what came back from [`DynImage::delete`]: the pixels survive the
    /// delete, so putting the picture up again costs one command where re-encoding a frame
    /// costs a memcpy of every pixel.
    Hidden(ImageRequest<F>),
}

impl<F: Format> Picture<F> {
    /// Take the picture off the terminal, keeping what can put it up again.
    ///
    /// The whole of the move is here because only the enum can do it: `delete` consumes the
    /// image, and the image lives inside the variant, so it has to be taken out before it can
    /// be given away. Nothing to do for the other two — a hidden picture is already off the
    /// screen and one that never drew has nothing to take off.
    pub fn hide(&mut self) {
        let taken = std::mem::replace(self, Picture::Nothing);
        *self = match taken {
            Picture::Shown(image) => Picture::Hidden(image.delete()),
            other => other,
        };
    }

    /// Put the picture up again, under `placement`, and wait for the terminal to take it.
    ///
    /// Both forms keep their pixels, so neither needs the client to draw anything: a shown
    /// image re-sends what it holds, and a hidden one travels back up from the request the
    /// delete handed over. `Nothing` is a window between opening and its first frame, which
    /// has no picture to raise.
    ///
    /// A transmit, never a placement. A placement names an image the terminal may have
    /// dropped, and it answers `ENOENT` when it has — a failure with no recovery in it,
    /// since a placement carries no pixels.
    pub async fn show(&mut self, image_id: u32, placement: PlacementRequest) -> Result<()> {
        use Picture::*;

        let taken = std::mem::replace(self, Nothing);
        let mut image = match taken {
            Nothing => return Ok(()),
            Shown(image) => image.retransplace(placement)?.await?,
            Hidden(request) => {
                request
                    .temp_remove_this_api_maybe_resend_shm()?
                    .transplace(image_id, placement)
                    .await?
            }
        };
        tokio::time::timeout(TRANSMIT_TIMEOUT, image.sync())
            .await
            .map_err(|_| Error::TransmitTimeout(TRANSMIT_TIMEOUT))??;
        *self = Picture::Shown(image);
        Ok(())
    }
}

pub struct Window<F: Format> {
    /// What the picture is doing, and the operations pending on it.
    pub picture: Picture<F>,
    /// The codec this window's frames are in, which is what its decoder was built from.
    pub codec: CodecId,
    /// The codec's decoder, kept across frames.
    ///
    /// Not an `Option`: a window's frames are in one codec for its whole life, so the decoder is
    /// made when the window is opened. A stream that goes bad means the decoder is *replaced*
    /// from [`Window::codec`], not that there is none.
    pub decoder: Box<dyn codecs::Decoder>,
}

impl<F: Format> Drop for Window<F> {
    /// The picture goes with the window.
    ///
    /// The terminal owns an image once it is transmitted, so dropping this frees nothing on the
    /// screen: the last frame stays where it was for as long as the terminal keeps it. Saying so
    /// is the only thing that removes it, and this is the one place that always runs — a window
    /// closed, a session that ends, a link that dies.
    fn drop(&mut self) {
        if let Picture::Shown(image) = std::mem::replace(&mut self.picture, Picture::Nothing) {
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
    pub async fn run(
        mut self,
        anchors: std::sync::Arc<arc_swap::ArcSwap<HashMap<SurfaceId, Option<Position>>>>,
    ) -> Result<()> {
        while let Some(message) = self.connection.recv().await? {
            if let Err(e) = self.handle_inner(message, &anchors).await {
                log::error!("{e}");
                // Reported rather than returned: the link is the only channel a client listens on,
                // and a failure it is not told about is a pane that stops changing for no stated
                // reason. Retried because this is the last thing the peer is ever told — a report
                // that is itself lost leaves that same silent pane.
                let report = Message::Error(shared_utils::report::drawn(miette::Report::new(e)));
                futures_retry::FutureRetry::new(
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
        Ok(())
    }

    /// One message, and what it means.
    async fn handle_inner(
        &mut self,
        message: Message,
        anchors: &Arc<arc_swap::ArcSwap<HashMap<SurfaceId, Option<Position>>>>,
    ) -> Result<()> {
        match message {
            Message::Frame {
                surface,
                z,
                cols,
                rows,
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
                        if !self.debug.no_graphics {
                            self.paint(bytes.as_deref(), surface, *at, z, (cols, rows))
                                .await?
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
                        picture: Picture::Nothing,
                        decoder: codec.to_decoder()?,
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
            other @ (Message::CompositorRequest(_) | Message::WindowCreated { .. }) => {
                tracing::warn!("tui: {other:?} is not handled yet");
            }
        }

        Ok(())
    }

    /// Paint one frame into this pane.
    /// The frame that arrives is a stretch of the codec's stream, and the terminal does
    /// not accept that, so it is decoded here and rebuilt as a request in this session's
    /// medium. Which medium that is is the type parameter, not a runtime choice:
    /// [`Format::request`] is what turns pixels into a request for it.
    ///
    /// The frame is painted into the window its surface names: the surface
    /// identifies the cells, and the frame is a stretch of *that* window's stream, so
    /// the decoder it continues is that window's. The pid is the report's own — the
    /// client that sent it owns the window.
    ///
    /// One span per frame, so the profiler's timeline is divided at the same
    /// boundary the pane's is. The awaits are inside it, which is why this is
    /// `#[instrument]` rather than a held guard: `Span::enter`'s guard must not
    /// outlive an `.await`, and the attribute enters and exits per poll instead.
    /// The two waits it contains — the terminal's acknowledgement of the transmit, and
    /// of the placement — are the two numbers this exists to see.
    #[tracing::instrument(name = "frame", skip(self, stream))]
    async fn paint(
        &mut self,
        stream: Option<&[u8]>,
        surface: SurfaceId,
        at: Position,
        z: u32,
        (cols, rows): (u32, u32),
    ) -> Result<()> {
        // A frame for a window this side has not been told about is not a failure: the
        // announcement and the picture are two messages, and a window that is already drawing
        // can be announced after the drawer's first commit has been encoded. The frame is
        // dropped and the next one — which arrives after the announcement — is painted, so
        // nothing is lost but one turn.
        let Some(state) = self.windows.get_mut(&surface) else {
            tracing::debug!("a frame for a window this session does not know: {surface:?}");
            return Ok(());
        };

        let placement = PlacementRequest {
            // The box the picture is scaled into, in cells, as the side that owns the window laid
            // it out. `c`/`r` are the keys that scale: the terminal fits the image into that many
            // cells and computes the other dimension from the picture's own aspect ratio, so the
            // window keeps its shape at whatever size the pane gives it.
            //
            // Not `w`/`h`, which look like a size and are not: they name a *source* rectangle to
            // crop the image by, so the pixels sent there would cut the picture down and leave it
            // drawn at its own resolution inside a box of a different one.
            cols: Some(cols),
            rows: Some(rows),
            // Behind the text *and* behind the cell backgrounds, so a window's
            // picture cannot cover another window's content. Ordered among windows by
            // `z`, which is what `stacking` is for.
            z: Some(stacking(z)),
            placement_id: Some(PLACEMENT_ID),
            // A real placement draws at the cursor, and nothing here is virtual: the
            // cells are the window's own area, so the picture goes where the cursor is
            // put.
            virtual_placement: false,
            // Without this the terminal moves the cursor by the placement's own box
            // after placing, once per frame: on a stream that is a scroll per frame.
            fix_cursor: true,
            // The picture is drawn from the cursor rather than from a key, so the cell it goes to
            // *is* the placement: the protocol has no field for a screen position, which is why
            // this is a cursor move. It rides the same write as the escape, and the writer puts
            // the cursor back afterwards, so nothing lands between the move and the picture it
            // positions and nothing is left moved.
            at: Some((at.1, at.0)),
            ..Default::default()
        };

        let image_id = image_id(&surface);

        // No pixels: the window moved or changed rank while its program drew nothing, so the
        // picture the terminal already holds is what belongs in the new box. Placing it again is
        // the whole of the work, under the same image and placement id, which the protocol
        // replaces rather than duplicates.
        //
        // `?` on the image is the first frame of a window that has not drawn: there is no
        // picture to move, and the frame carrying pixels is what puts one up.
        if let Some(data) = stream {
            let pixels = if self.debug.blue_pixel {
                // The frame is ignored on purpose, so nothing in the wire path, the codec
                // or the decode is exercised: what appears on screen is attributable to
                // the encode, the transmit and the placement alone.
                image::RgbImage::from_pixel(BLUE.0, BLUE.1, image::Rgb([0, 0, 255]))
            } else {
                match state.decoder.decode(data) {
                    Ok(Some(frame)) => {
                        image::RgbImage::from_raw(frame.width, frame.height, frame.rgb).ok_or(
                            Error::FrameSize {
                                width: frame.width,
                                height: frame.height,
                            },
                        )?
                    }
                    // Headers, or a frame the codec held back: nothing to show yet,
                    // and nothing to ask for — the next frame the client sends is what
                    // the decoder held, and a static window sends again when it moves.
                    Ok(None) => return Ok(()),
                    // The decoder's state is what failed: replace it, so the next frame starts
                    // the stream over from the codec the window was opened with. The client is
                    // not asked for a keyframe because a client cannot be asked: every message
                    // it reads has to be one this side can answer for, and a request it cannot
                    // answer is a stall.
                    Err(e) => {
                        state.decoder = state.codec.to_decoder()?;
                        tracing::error!("the codec failed: {e}");
                        return Ok(());
                    }
                }
            };

            // Where this is, taken before the call rather than inside the error: `line!()`
            // evaluates to the line it is written on, and written in the struct literal below
            // it would point at the literal instead of at the call that failed.
            let site = line!() as usize + 1;
            let request = F::request(&self.controller, pixels).map_err(|source| Error::Stage {
                source,
                surface,
                site: shared_utils::report::line_span(SESSION_SOURCE, site),
                source_code: miette::NamedSource::new("crates/tui/src/session.rs", SESSION_SOURCE),
            })?;
            // One command, not two. Transmitting and placing separately leaves the image on
            // screen with nothing drawing it in between — the terminal frees an image's
            // placements when its data is replaced — and that gap is a round trip wide, which is
            // a visible flicker on every frame.
            //
            // Timed, because this is the one await in the program that a terminal can leave
            // hanging: the acknowledgement is a reply a terminal may not send, and a session that
            // stalls here shows an empty pane forever with no clue why.
            let mut image =
                tokio::time::timeout(TRANSMIT_TIMEOUT, request.transplace(image_id, placement))
                    .await
                    .map_err(|_| Error::TransmitTimeout(TRANSMIT_TIMEOUT))?
                    .map_err(Error::Graphics)?;

            // Waited on: until the placement lands nothing is on screen, so a frame that was
            // not placed is a frame the user never sees. Timed like the transmit above, and for
            // the same reason.
            tokio::time::timeout(TRANSMIT_TIMEOUT, image.sync())
                .await
                .map_err(|_| Error::TransmitTimeout(TRANSMIT_TIMEOUT))??;
            // `sync` returns nothing; the image is what is stored, and it is still held here.
            state.picture = Picture::Shown(image);
        } else {
            // The window moved or changed rank while its program drew nothing. Both forms
            // still hold the pixels, so the picture goes back up without the client being
            // asked for anything; a window that never drew has nothing to raise.
            state.picture.show(image_id, placement).await?;
        }

        Ok(())
    }
}

#[cfg(test)]
#[path = "../tests/unit/session.rs"]
mod tests;
