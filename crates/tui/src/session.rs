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
use easytgp::{Controller, DynImage, Format, PlacementRequest};
use wire::message::{Message, SurfaceId};
use wire::transport::Connection;

use crate::error::{Error, Result};
use crate::scanner::Position;
use crate::DebugOptions;

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
pub struct Window<F: Format> {
    /// The image the frames are sent under, and the pending operations on it.
    ///
    /// Kept across frames rather than rebuilt, because it owns the unacknowledged transmits and
    /// placements: a frame adds to it and `sync` is what waits for those to land. `None` until the
    /// first frame, since an image cannot be created before there are pixels for it.
    pub image: Option<DynImage<F>>,
    /// The codec this window's frames are in, which is what its decoder was built from.
    pub codec: CodecId,
    /// The codec's decoder, kept across frames.
    ///
    /// Not an `Option`: a window's frames are in one codec for its whole life, so the decoder is
    /// made when the window is opened. A stream that goes bad means the decoder is *replaced*
    /// from [`Window::codec`], not that there is none.
    pub decoder: Box<dyn codecs::Decoder>,
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

    pub async fn handle(
        &mut self,
        positions: &HashMap<SurfaceId, Position>,
        sink: &mut (impl std::io::Write + ?Sized),
    ) -> Result<()> {
        while let Some(message) = self.connection.recv_next()? {
            if let Err(e) = self.handle_inner(message, positions, sink).await {
                tracing::error!("{e}");
                // Reported rather than returned: the link is the only channel a client listens on,
                // and a failure it is not told about is a pane that stops changing for no stated
                // reason. Retried because this is the last thing the peer is ever told — a report
                // that is itself lost leaves that same silent pane.
                let report = Message::Error(e.to_string());
                futures_retry::FutureRetry::new(
                    || self.connection.send(&report),
                    crate::error::ReportFailure,
                )
                .await
                .unwrap_or_else(|(e, attempts)| {
                    panic!(
                        "could not report a failure to the client after {attempts} attempts: {e}"
                    )
                });
            }
        }
        Ok(())
    }

    /// One message, and what it means.
    async fn handle_inner(
        &mut self,
        message: Message,
        positions: &HashMap<SurfaceId, Position>,
        sink: &mut (impl std::io::Write + ?Sized),
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
                match positions.get(&surface) {
                    Some(at) => {
                        self.paint(bytes.as_deref(), surface, *at, z, (cols, rows), sink)
                            .await?
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
                        if let Some(image) = self
                            .windows
                            .get_mut(&surface)
                            .and_then(|window| window.image.take())
                        {
                            image.delete();
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
                        image: None,
                        decoder: codec.to_decoder()?,
                    },
                );
                log::info!(
                    "opened a window on {surface:?} in {codec:?} ({} now open)",
                    self.windows.len()
                );
            }
            Message::CloseWindow { surface } => {
                match self.windows.remove(&surface) {
                    // The picture is deleted with the window. The terminal owns an image once it
                    // is transmitted, so dropping the window frees nothing on screen: the last
                    // frame would stay where it was for the rest of the session. `delete` consumes
                    // the image because its id goes with it, which is why the window comes out of
                    // the map first.
                    Some(window) => {
                        if let Some(image) = window.image {
                            image.delete();
                        }
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
            other @ Message::CompositorRequest(_) => {
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
    #[tracing::instrument(name = "frame", skip(self, stream, sink))]
    async fn paint(
        &mut self,
        stream: Option<&[u8]>,
        surface: SurfaceId,
        at: Position,
        z: u32,
        (cols, rows): (u32, u32),
        sink: &mut (impl std::io::Write + ?Sized),
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
            ..Default::default()
        };

        // The picture is drawn from the cursor, so the cursor goes to the window's own
        // top-left cell first. This is the position — the protocol has no key for it,
        // which is why it is a cursor move and not a field on the placement.
        //
        // Both paths below place a picture, so both need it, and it goes first: a move that
        // landed between two halves of an escape would put the picture in the wrong place.
        sink.write_all(crossterm::cursor::MoveTo(at.0, at.1).to_string().as_bytes())
            .map_err(Error::Screen)?;

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
                    Ok(Some(frame)) => image::RgbImage::from_raw(
                        frame.width,
                        frame.height,
                        frame.rgb,
                    )
                    .ok_or(Error::FrameSize {
                        width: frame.width,
                        height: frame.height,
                    })?,
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

            let request = F::request(&self.controller, pixels).map_err(Error::Stage)?;
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
            state.image = Some(image);
        } else {
            // A window that has not drawn has no picture to move, which is not a failure: it is
            // the state every window is in between opening and its first frame.
            let Some(image) = state.image.as_mut() else {
                return Ok(());
            };
            image.place(placement);
            tokio::time::timeout(TRANSMIT_TIMEOUT, image.sync())
                .await
                .map_err(|_| Error::TransmitTimeout(TRANSMIT_TIMEOUT))??;
        }

        Ok(())
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    use wire::transport::Transport as _;
    use wire::unix::UnixSocket;

    /// The window the tests paint into: its surface is its identity, and that is what a
    /// frame names.
    const SURFACE: SurfaceId = SurfaceId {
        symbol: '❋',
        fg: (1, 2, 3),
        bg: (4, 5, 6),
    };

    /// A terminal that collects what it is sent and answers the liveness probes.
    ///
    /// The library's reply path is a real conversation: a transfer asks the
    /// terminal for an acknowledgement and waits for it, so a test that never
    /// answers would hang rather than fail. This answers the way a terminal does —
    /// echoing the marker it was asked about — which is what makes the awaited
    /// transmit complete.
    struct Terminal {
        /// Everything written to the terminal, escapes and all.
        written: Arc<Mutex<Vec<u8>>>,
        /// Where the acknowledgement goes: the input side of the library.
        keys: Mutex<std::io::PipeWriter>,
    }

    impl Write for Terminal {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.written.lock().unwrap().extend_from_slice(buf);

            // Anything the library waits on is answered by its `i=`: a liveness probe, a
            // transplace, a placement. The answer is the id it named, back with an `OK`, which
            // is the same conversation a real terminal has. Answering anything that names an id
            // is what makes this a terminal rather than a stub per command.
            let text = String::from_utf8_lossy(buf);
            for part in text.split('\x1b').filter(|p| p.starts_with("_G")) {
                let Some(marker) = part
                    .split(',')
                    .filter(|cd| cd.starts_with("i=") || cd.contains(",i="))
                    .find_map(|cd| cd.get(cd.find("i=")? + 2..))
                    .map(|v| v.trim_end_matches('\u{5c}'))
                    .filter(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
                else {
                    continue;
                };
                let ack = format!("\x1b_Gi={marker};OK\x1b\\");
                self.keys.lock().unwrap().write_all(ack.as_bytes())?;
            }
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// A codec frame of one flat colour, which is all the painter looks at.
    ///
    /// Encoded through the codec itself rather than anything simpler: a test that
    /// feeds the decoder bytes another codec produced would pass by accident.
    fn codec_frame(width: u32, height: u32) -> Vec<u8> {
        let mut encoder = codecs::h264::Encoder::new(width, height).expect("an encoder");
        let bgra = vec![20u8, 20, 20, 255].repeat((width * height) as usize);
        let stream = codecs::Encoder::encode(&mut encoder, &bgra, width, height, false)
            .expect("encode a frame");
        assert!(
            !stream.is_empty(),
            "the first frame of a stream carries its picture"
        );
        stream
    }

    /// A connection to throw away, so a session can be built.
    ///
    /// The link is not what these tests are about, but a session cannot exist
    /// without one: it is the thing a session is attached to.
    async fn connection(dir: &std::path::Path) -> Connection {
        let _ = std::fs::remove_dir_all(dir);
        let path = dir.join("s.sock");
        let bound = UnixSocket::new(&path);
        bound.bind().await.expect("bind");
        let dialer = UnixSocket::new(&path);
        let (accepted, _dialed) = tokio::join!(bound.accept(), dialer.connect());
        accepted.expect("accept")
    }

    /// The screen the session draws on: the same buffer the library's escapes land in.
    ///
    /// Nothing is answered here. A probe is the library's own liveness check, and the thread that
    /// drains its escapes is what answers those; this only appends what a session writes itself,
    /// which is the cursor move a real placement needs.
    struct Screen(Arc<Mutex<Vec<u8>>>);

    impl Write for Screen {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Where the one window is: the cells the scan would have found it on.
    ///
    /// Written by hand because a client draws those cells: a real session gets this from a scan of
    /// the screen, and a test has no screen to scan.
    fn where_it_is() -> HashMap<SurfaceId, Position> {
        HashMap::from([(SURFACE, (3, 5))])
    }

    /// Put everything the escape pipe has onto `terminal`, which is what a step does.
    ///
    /// Every frame is transmitted under one id, and placed again.
    ///
    /// The escape destination is global to the process and can only be set once, so both halves are
    /// one test: two would race for it, and whichever lost would observe an empty terminal rather
    /// than a wrong painting. The behaviour under test is a sequence anyway — first frame, then
    /// next.
    ///
    /// The re-placement is the point rather than a detail. A transmit under an id the terminal
    /// already has **frees that image's placements**, so a session that places only its first frame
    /// shows one picture and then a blank pane. What is asserted here is that the second frame
    /// carries its own placement, under the same id.
    #[tokio::test]
    async fn every_frame_reuses_one_id_and_places_itself_again() {
        let written = Arc::new(Mutex::new(Vec::new()));

        // The wiring the program has: the library's escapes go through the sink installed here,
        // and that sink is also the terminal — it records the bytes and answers what it must, so
        // a paint can complete its wait for an acknowledgement.
        //
        // Both halves live in one sink because the destination is process-global: a test that
        // split recording from answering would have to route through a pipe this side would then
        // have to drain while the paint is still waiting.
        let (answers, replies) = std::io::pipe().expect("a pipe for the terminal's answers");
        let (controller, input) =
            easytgp::Controller::init_with_pipe(answers).expect("the controller's pipes");
        // Dropped: nothing here is typed, and a held writer would keep the router's forward alive.
        drop(input);

        let sink = Terminal {
            written: written.clone(),
            keys: Mutex::new(replies),
        };
        assert!(
            easytgp::set_sink(Box::new(sink)),
            "this test installs the only sink in the process"
        );

        let dir = std::env::temp_dir().join(format!("tui-session-{}", std::process::id()));
        let mut session = Session::<easytgp::Shm>::new(
            connection(&dir).await,
            Arc::new(DebugOptions::default()),
            controller,
        );
        // The sink is what the library writes through, so the session only has to hand it
        // somewhere to put its own writes. It draws nothing itself, which is why a sink of its own
        // is the same buffer.
        let mut terminal = Screen(written.clone());

        // The window, which brings its codec with it. A frame names only its surface, so a session
        // that has not been told about that window has nowhere to paint it.
        session
            .handle_inner(
                Message::OpenWindow {
                    surface: SURFACE,
                    codec: CodecId::H264,
                },
                &where_it_is(),
                &mut terminal,
            )
            .await
            .expect("the window is opened");

        session
            .handle_inner(
                Message::Frame {
                    surface: SURFACE,
                    z: 0,
                    cols: 8,
                    rows: 8,
                    bytes: Some(codec_frame(8, 8)),
                },
                &where_it_is(),
                &mut terminal,
            )
            .await
            .expect("the first frame is painted");
        // The screen's thread writes the escapes into the same buffer, so this is where the paint
        // landed rather than a second thing that had to be asked for.
        let first = String::from_utf8(written.lock().unwrap().clone()).expect("escapes are text");
        // One command, `a=T`, that both replaces the pixels and makes the placement. The two used
        // to be separate, and the terminal frees an image's placements when its data is replaced —
        // so between them the picture was on screen with nothing drawing it, for as long as a round
        // trip takes. That gap is the flicker.
        assert!(first.contains("a=T"), "one transplace command: {first:?}");
        // The image is named by the window's own surface, so two windows are two pictures rather
        // than one replacing the other.
        let image = image_id(&SURFACE);
        assert!(first.contains(&format!("i={image}")), "{first:?}");
        assert!(first.contains("t=s"), "{first:?}");
        // The transmitted data's size, and the cell box it is fitted into. Both travel on one
        // command and are different keys.
        assert!(first.contains("s=8"), "{first:?}");
        assert!(first.contains("v=8"), "{first:?}");
        // The picture is placed where the cursor was moved to, which is the window's own top-left
        // cell: the scan reports (3, 5), and the cursor move is what puts it there. Not `x`/`y` —
        // those crop the source, which is why the position is a cursor move and not a key.
        assert!(first.contains("\u{1b}[6;4H"), "the cursor is moved: {first:?}");
        // Under the text *and* under the cells' own backgrounds, which the protocol puts strictly
        // below `INT32_MIN / 2` — a window's box is full of background colour, so a picture at or
        // above that boundary would be covered by the cells it exists to sit behind. Rank 0 is the
        // furthest back, so it is the floor of the range and not the boundary.
        assert!(first.contains(&format!("z={}", stacking(0))), "{first:?}");
        assert!(
            stacking(0) < i32::MIN / 2,
            "under the backgrounds: {}",
            stacking(0)
        );
        assert!(first.contains("p=98"), "{first:?}");
        // Without this the terminal moves the cursor by the placement's box once per frame, which
        // walks the picture down the screen.
        assert!(first.contains("C=1"), "{first:?}");

        let after_first = first.len();
        session
            .handle_inner(
                Message::Frame {
                    surface: SURFACE,
                    z: 3,
                    cols: 8,
                    rows: 8,
                    bytes: Some(codec_frame(8, 8)),
                },
                &where_it_is(),
                &mut terminal,
            )
            .await
            .expect("the second frame is painted");

        let all = written.lock().unwrap().clone();
        let second = String::from_utf8(all[after_first..].to_vec()).expect("escapes are text");
        // The same id, so the client's cells keep naming the image being updated, and its own
        // placement, under the same placement id — a repeat transmit replaces the placement that
        // was there.
        assert!(
            second.contains("a=T"),
            "a later frame transplaces: {second:?}"
        );
        assert!(
            second.contains(&format!("i={}", image_id(&SURFACE))),
            "the same id: {second:?}"
        );
        assert!(
            second.contains("p=98"),
            "under the same placement id: {second:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
