//! One window's picture, and the operations that move it.
//!
//! A picture does one of three things: it is placed where the cells are, it
//! is taken off the screen because its cells went away, or it is put back when
//! they come back. Those are [`VideoStream::update`], [`VideoStream::hide`] and
//! [`VideoStream::show`], and the frame a picture is under travels with it —
//! without it, putting a picture back means asking the program for a frame it
//! has no reason to send.
//!
//! The pixels are never owned here: they live in the image on the terminal's
//! side, and what is kept in memory is the means to place it again. A decoded
//! frame arrives from outside, travels straight to the terminal, and what
//! survives is the acknowledgement, not the picture.

use easytgp::{DynImage, Format, ImageRequest, PlacementRequest};
use tokio_util::time::FutureExt;
use wire::message::FramePosition;

use crate::error::Result;
use crate::proxy::Position;

/// How long to wait for a terminal to acknowledge an image transmit.
///
/// A terminal that never answers is a terminal that will never draw the frame, so
/// waiting is pointless past the point where a working one would have replied.
/// Generous rather than tight: an acknowledgement is one short escape, and the cost
/// of being wrong is failing a session on a terminal that was merely slow.
///
/// `pub(crate)` because the error's `From<Elapsed>` turns a timeout into this duration's variant.
pub(crate) const TRANSMIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

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
pub(crate) fn stacking(z: u32) -> i32 {
    // The band is the whole lower half of the range, so a rank large enough to saturate is not one
    // the gui can produce: the furthest it could go is the front of the band, which is where
    // saturating puts it.
    const FLOOR: i32 = i32::MIN;
    const CEILING: i32 = i32::MIN / 2 - 1;
    let rank = i32::try_from(z).unwrap_or(i32::MAX);
    FLOOR.saturating_add(rank).min(CEILING)
}

/// The placement everything about a picture hangs off, paired with its image.
///
/// Distinct from [`crate::session::image_id`] because the two are different facts: the image is
/// the pixels, the placement is the box they are fitted into.
pub const PLACEMENT_ID: u32 = 98;

/// The placement a frame asks for, and the one a hidden picture is put back with.
///
/// One shape for both, because they are the same picture coming back to the same box: `at` is
/// where the scan found the cells this time around, and everything else is the frame's own
/// rank and box.
fn placement_request(at: Position, frame: FramePosition) -> PlacementRequest {
    let FramePosition { z, cols, rows } = frame;
    PlacementRequest {
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
        // Behind the text *and* behind the cell backgrounds, so a window's picture cannot
        // cover another window's content. Ordered among windows by `z`, which is what
        // `stacking` is for.
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
    }
}

/// One window's picture: what state it is in, and the operations that move it.
///
/// Generic over the medium, which is fixed when the session is built: a caller that knows its
/// terminal cannot map a shared memory object picks [`easytgp::Png`], and one that knows it can
/// picks [`easytgp::Shm`]. Neither pays for the other's branch, and the choice is a type rather
/// than a flag.
///
/// `Shown` and `Hidden` carry the frame the picture was placed under — where it sits and how
/// big it is — so a picture can be put back without a frame arriving or the program being
/// asked for one. A window that is not on screen sends no frames at all.
///
/// An `Option` could not say the difference that matters. A window that has not drawn has
/// no picture anywhere; a window whose cells went away has one — the pixels are still here,
/// the terminal was told to drop it — and coming back has to put it up again. Folded into
/// `None`, the second was treated as the first, and the returning pane stayed blank until
/// the program was given a reason to draw.
pub enum VideoStream<F: Format> {
    /// Nothing drawn yet, so there is nothing to place: every window starts here.
    Nothing,
    /// On the terminal, under the window's own id, under the frame it was placed with.
    ///
    /// Kept across frames rather than rebuilt, because it owns the unacknowledged transmits
    /// and placements: a frame adds to it and `sync` is what waits for those to land.
    Shown(DynImage<F>, FramePosition),
    /// Taken off the terminal, with the request that can put it back and the frame it was
    /// placed under.
    ///
    /// The request is what came back from [`DynImage::delete`]: the pixels survive the
    /// delete, so putting the picture up again costs one command where re-encoding a frame
    /// costs a memcpy of every pixel.
    Hidden(ImageRequest<F>, FramePosition),
}

impl<F: Format> VideoStream<F> {
    /// Take the picture off the terminal, keeping what can put it up again.
    ///
    /// The whole of the move is here because only the enum can do it: `delete` consumes the
    /// image, and the image lives inside the variant, so it has to be taken out before it can
    /// be given away. Nothing to do for the other two — a hidden picture is already off the
    /// screen and one that never drew has nothing to take off.
    pub fn hide(&mut self) {
        let taken = std::mem::replace(self, VideoStream::Nothing);
        *self = match taken {
            VideoStream::Shown(image, placed) => VideoStream::Hidden(image.delete(), placed),
            other => other,
        };
    }

    /// Whether the picture is off the terminal right now.
    pub(crate) fn is_hidden(&self) -> bool {
        matches!(self, VideoStream::Hidden(_, _))
    }

    /// Remember the frame the next placement is under.
    ///
    /// `Nothing` has nowhere to hang it and nothing to remember: the first frame that carries
    /// pixels sets the picture's frame itself, and one that does not has nothing to place.
    pub(crate) fn set_frame_position(&mut self, frame: FramePosition) {
        match self {
            VideoStream::Shown(_, slot) | VideoStream::Hidden(_, slot) => *slot = frame,
            VideoStream::Nothing => {}
        }
    }

    /// Put the picture up again at `at`, and wait for the terminal to take it.
    ///
    /// Where it goes is the frame it was last placed under — this does not ask the program for
    /// a new one — rebased onto `at`, so a pane whose cells came back anywhere shows its picture
    /// where they are now.
    ///
    /// Both forms keep their pixels, so neither needs the client to draw anything: a shown
    /// image re-sends what it holds, and a hidden one travels back up from the request the
    /// delete handed over. `Nothing` is a window between opening and its first frame, which
    /// has no picture to raise.
    ///
    /// A transmit, never a placement. A placement names an image the terminal may have
    /// dropped, and it answers `ENOENT` when it has — a failure with no recovery in it,
    /// since a placement carries no pixels.
    pub async fn show(&mut self, image_id: u32, at: Position) -> Result<()> {
        let placed = match std::mem::replace(self, VideoStream::Nothing) {
            VideoStream::Nothing => None,
            VideoStream::Shown(image, frame) => {
                Some((image.retransplace(placement_request(at, frame))?.await?, frame))
            }
            VideoStream::Hidden(request, frame) => Some((
                request
                    .temp_remove_this_api_maybe_resend_shm()?
                    .transplace(image_id, placement_request(at, frame))
                    .await?,
                frame,
            )),
        };
        let Some((image, frame)) = placed else {
            return Ok(());
        };
        self.settle(image, frame).await
    }

    /// Wait for the placement to land, then keep the image under the frame it went up with.
    ///
    /// The placement only counts once the terminal has it: until then nothing is on
    /// screen, and a frame that was not placed is a frame the user never sees. Timed: a
    /// terminal that never acknowledges never draws, and waiting past the point a working
    /// one would have replied is waiting for nothing.
    async fn settle(&mut self, mut image: DynImage<F>, frame: FramePosition) -> Result<()> {
        // `sync` returns nothing; the image is what is stored, and it is still held here.
        image.sync().timeout(TRANSMIT_TIMEOUT).await??;
        *self = VideoStream::Shown(image, frame);
        Ok(())
    }

    /// Transmit a freshly decoded frame under the window's image id, and place it.
    ///
    /// This is the common case, once per frame with pixels in it. One command, not two:
    /// transmitting and placing separately leaves the image on screen with nothing drawing it
    /// in between — the terminal frees an image's placements when its data is replaced — and
    /// that gap is a round trip wide, which is a visible flicker on every frame.
    ///
    /// Timed, because this is the one await in the program that a terminal can leave
    /// hanging: the acknowledgement is a reply a terminal may not send, and a session that
    /// stalls here shows an empty pane forever with no clue why.
    pub async fn update(
        &mut self,
        image_id: u32,
        at: Position,
        frame: FramePosition,
        request: ImageRequest<F>,
    ) -> Result<()> {
        let image = request
            .transplace(image_id, placement_request(at, frame))
            .timeout(TRANSMIT_TIMEOUT)
            .await??;
        self.settle(image, frame).await
    }
}
