//! The wayland session, wearing the windowing vocabulary.
//!
//! The session runs in `wayland`: surfaces, commits, the seat. This is the layer
//! over it: the session's windows as [`AgnosticWindow`] values, each with its
//! children, and — because this is the side with a link — the stream each one's
//! picture travels in.
//!
//! The shape is the demo session's, deliberately: a server holding its root, a
//! window holding its own state and its children. A window is minted once and
//! kept — the compositor hands the same window back every frame, and with it the
//! place the compositor last put it.
//!
//! # What a window owns that the compositor does not
//!
//! The surface id, the codec and the encoder. The id is what the pane's cells are
//! painted with so the far side can find the picture; the codec is what that
//! picture is compressed with; and the encoder is the pictures already sent, which
//! is why it is per window — a window is a stream.

use std::sync::{Mutex, Weak};

use compositor::Error;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use wayland::WaylandServer as Native;
use tokio::sync::broadcast;
use wire::message::{Message, SurfaceId};
use wire::transport::Connection;

use super::{Compositable, WindowingServer};

/// What a session is started with, as the backend this machine has answers it.
pub use wayland::Options;

/// The wayland session as a windowing server.
pub struct WaylandServer {
    /// The session.
    inner: Native,
    /// The codec every window of this session streams with: a property of the
    /// stream, and every window of one session shares it.
    codec: codecs::CodecId,
    /// The program's own output, held until someone takes it.
    ///
    /// Not drained here: what a program prints belongs to whoever shows it, and this side only
    /// started it. Kept rather than dropped for the same reason any pipe must be read — a writer
    /// blocks once it fills — and taken once, because a pipe has one reader.
    stdout: Option<std::io::PipeReader>,
}

/// One window of the session: the session's window, the windows its program
/// opened after it, and the stream its picture travels in.
pub struct WaylandWindow {
    /// The window in the session.
    ///
    /// `Weak`, because the session owns its windows and this does not: a wrapper that kept the
    /// `Arc` would be a second owner, and would have to be pruned when the real one died. Holding
    /// the weak means a dead window is one that fails to upgrade, which is the truth and needs no
    /// bookkeeping.
    inner: Weak<wayland::Window>,
    /// What this window's program has opened since the last read.
    ///
    /// Subscribed when the wrapper is made, so a child admitted while it was being built is still
    /// reported: a subscription starts at the moment it is made, and one made later would miss it.
    ///
    /// Draining one advances its own cursor, which is why it is behind a lock: the trait hands the
    /// window to its reader as `&self`, because that reader waits on this and feeds the window in
    /// the same `select!` — an `&mut` wait would conflict with the arms that type into it.
    new_children: tokio::sync::Mutex<broadcast::Receiver<Weak<wayland::Window>>>,
    new_popups: tokio::sync::Mutex<broadcast::Receiver<Weak<wayland::Window>>>,
    /// The cells this window's picture goes into, which is what the far side scans
    /// the pane for.
    ///
    /// Minted with the window and kept: the id names the image on the far side, so
    /// a new one per frame would be a new image per frame.
    surface: SurfaceId,
    /// What this picture is encoded with: the session's, for every window of it.
    codec: codecs::CodecId,
    /// The encoder those frames go through, and the pictures it has already sent.
    ///
    /// Built on the first frame, because an encoder is made for a size and the size
    /// is only known once the window has drawn.
    ///
    /// `Mutex` rather than `&mut`: a frame is taken through `&self`, because the
    /// window is behind an `Arc` that the pane holds for drawing.
    encoder: Mutex<Option<Box<dyn codecs::Encoder>>>,
}

impl WaylandWindow {
    /// One window of the session, with nothing known about where it sits yet.
    ///
    /// Announced here rather than at the first frame: the pane paints this window's cells the
    /// moment the window exists, and the far side blanks them only for a surface it has been
    /// told about. Announcing at the first capture leaves the first painted cells on screen,
    /// and nothing repaints them — the picture ends up behind cells still carrying the anchor.
    ///
    /// `None` for a window the session has already let go, which is not a failure: the caller
    /// asked about a handle it was given and the answer is that there is nothing behind it.
    fn new(inner: Weak<wayland::Window>, codec: codecs::CodecId) -> Option<Self> {
        let window = inner.upgrade()?;
        // Subscribed here, not at the first read: a subscription starts at the moment it is made,
        // so a child admitted while this wrapper is being built would otherwise be one nobody ever
        // hears about.
        let new_children = window.subscribe_new_children();
        let new_popups = window.subscribe_new_popups();
        Some(Self {
            inner,
            new_children: tokio::sync::Mutex::new(new_children),
            new_popups: tokio::sync::Mutex::new(new_popups),
            surface: SurfaceId::random(),
            codec,
            encoder: Mutex::new(None),
        })
    }
}

impl Drop for WaylandWindow {
    /// Nothing to tell the far side, and nothing to release.
    ///
    /// The picture is named by a surface id that travels with every frame, and the far side frees
    /// it when the frames stop; a window that is over is one whose `Weak` no longer upgrades.
    fn drop(&mut self) {}
}

impl WaylandServer {
    /// Start the session.
    ///
    /// The wait for the window the program draws is the caller's, taken from the session's own
    /// channel by [`Self::recv_root`]: only a turn notices a client has committed, so the news can
    /// only come from the session, and the wait can only live where it is awaited.
    pub fn new(options: wayland::Options, codec: codecs::CodecId) -> std::io::Result<Self> {
        let inner = Native::new(options).map_err(|e| std::io::Error::other(e.to_string()))?;
        Ok(Self {
            inner,
            codec,
            stdout: None,
        })
    }

    /// The wait for the window the program drew, owning what it waits on.
    ///
    /// Owned rather than borrowed, because the caller has to keep turning the session while it
    /// waits: a future holding the session would make the very turn that answers it impossible.
    pub fn take_root(
        &mut self,
    ) -> std::io::Result<impl std::future::Future<Output = std::io::Result<Box<dyn Compositable>>>>
    {
        let root = self
            .inner
            .take_root_receiver()
            .ok_or_else(|| std::io::Error::other("the root window is handed over once"))?;
        let codec = self.codec;
        Ok(async move {
            let root = root
                .await
                .map_err(|_| std::io::Error::other("the session ended before it drew a window"))?;
            WaylandWindow::new(root, codec)
                .map(|window| Box::new(window) as Box<dyn Compositable>)
                .ok_or_else(|| std::io::Error::other("the root window is already gone"))
        })
    }
}

impl WindowingServer for WaylandServer {
    async fn recv_root(&mut self) -> std::io::Result<Box<dyn Compositable>> {
        self.take_root()?.await
    }

    fn spawn(&mut self, path: &std::path::Path) -> std::io::Result<Option<std::io::PipeReader>> {
        // The program's own words, kept until someone takes them: what a program says belongs to
        // whoever shows it, and this side only started it. Both of its streams arrive here, in
        // the order it wrote them, because the session gives the child one pipe for both.
        self.stdout = Some(self.inner.spawn(path)?);
        Ok(None)
    }

    fn take_stdout(&mut self) -> Option<std::io::PipeReader> {
        self.stdout.take()
    }

    #[tracing::instrument(name = "compositor.advance", skip_all)]
    fn advance(&mut self, timeout: std::time::Duration) {
        self.inner.step(timeout);
    }

    fn is_running(&self) -> bool {
        self.inner.is_running()
    }

    /// The session is resized: the output changes mode, and the root window is
    /// measured against the new size on its next read.
    ///
    /// The caller states the size the pane is every frame, so this is where the
    /// comparing is done: a session already that size changes nothing, which is
    /// what keeps an untouched terminal from a configure per frame.
    fn set_size(&mut self, size: (u32, u32)) {
        if self.inner.size() != size {
            self.inner.set_size(size);
        }
    }
}

// The macro, because the trait is written with it: an `async fn` in a trait the compositor holds
// as `dyn` is desugared into a boxed future, and an implementation spelled the language's own way
// describes a different signature than the one the vtable points at.
#[async_trait::async_trait]
impl Compositable for WaylandWindow {
    fn key_input(&self, event: termina::event::KeyEvent) -> std::io::Result<()> {
        let window = self
            .inner
            .upgrade()
            .ok_or_else(|| std::io::Error::other("the window is gone"))?;
        window.queue_key(crate::windowing::event_map::key(&event));
        Ok(())
    }

    fn pointer_input(&self, event: termina::event::MouseEvent) -> std::io::Result<()> {
        let window = self
            .inner
            .upgrade()
            .ok_or_else(|| std::io::Error::other("the window is gone"))?;
        if let Some(event) = crate::windowing::event_map::pointer(
            &event,
            (f64::from(event.column), f64::from(event.row)),
        ) {
            window.queue_pointer(event);
        }
        Ok(())
    }

    /// Take the picture the window drew, encode it, and send it down the link.
    ///
    /// The announcement went out when the window was minted, so the far side already knows the
    /// surface the frame names. What is built here is the encoder, on the first frame that has
    /// pixels: an encoder is made for a size, and the size is only known once the window has
    /// drawn.
    ///
    /// A message goes out every frame, whether or not there are new pixels. The box and the
    /// rank are what place the picture, and they change while the picture does not — a window
    /// being dragged moves without its program drawing anything — so a frame that only repeats
    /// the picture still has to travel. `None` bytes is exactly that: place what you already have
    /// in this box.
    ///
    /// The box the picture goes into travels with it, in cells: the far side puts a
    /// picture on a grid of cells, and this is the side that decided which cells those are.
    #[tracing::instrument(name = "window.frame", skip_all)]
    async fn frame(
        &self,
        connection: &Connection,
        rank: u32,
        cells: (u16, u16),
    ) -> std::io::Result<()> {
        // The picture, if the program drew one. Nothing to take is the ordinary case for a
        // window nobody has touched since the last frame.
        let window = self.inner.upgrade().ok_or(Error::Gone)?;
        let captured = window.capture();
        log::info!(
            "window.frame: captured {}, size {:?}",
            captured.is_some(),
            captured.as_ref().map(|(_, size)| *size)
        );
        let bytes = match captured {
            Some((pixels, size)) => {
                let mut held = self.encoder.lock().unwrap_or_else(|e| e.into_inner());
                let encoder = match held.as_mut() {
                    Some(encoder) => encoder,
                    None => held.insert(
                        self.codec
                            .to_encoder(size)
                            .map_err(|e| Error::Frame(e.to_string()))?,
                    ),
                };
                let encoded = encoder
                    .encode(&pixels, size.0, size.1, false)
                    .map_err(|e| Error::Frame(e.to_string()))?;
                // A codec holding a frame back has nothing for the wire: not a failure, and
                // not pixels either — the picture already on the far side stays in the box.
                (!encoded.is_empty()).then_some(encoded)
            }
            None => None,
        };

        let message = Message::Frame {
            surface: self.surface,
            z: rank,
            cols: u32::from(cells.0),
            rows: u32::from(cells.1),
            bytes,
        };
        connection.send(&message).await
    }

    /// How big this window is, in pixels: what its program last committed, or the size it was
    /// adopted at before it drew.
    fn size_px(&self) -> std::io::Result<(u32, u32)> {
        Ok(self
            .inner
            .upgrade()
            .ok_or_else(|| std::io::Error::other("the window is gone"))?
            .size())
    }

    /// Ask the session's window to draw at a size. The next commit answers.
    ///
    /// Asked unconditionally: whether this is a size the window was already asked for is the
    /// caller's record, not this one's — the window knows what its program last committed at,
    /// which is the answer to the ask rather than the ask itself.
    #[tracing::instrument(name = "window.set_size", skip_all)]
    fn set_size(&self, size: (u32, u32)) -> std::io::Result<()> {
        self.inner
            .upgrade()
            .ok_or_else(|| std::io::Error::other("the window is gone"))?
            .set_size(size);
        Ok(())
    }

    /// Whether this window is a popup, which its own program sized.
    fn is_popup(&self) -> bool {
        self.inner
            .upgrade()
            .is_some_and(|window| window.is_popup())
    }

    /// The signature this window's cells carry, which names it on both sides.
    fn surface(&self) -> SurfaceId {
        self.surface
    }

    /// The codec this window's frames are in, which the far side needs to decode them.
    fn codec(&self) -> codecs::CodecId {
        self.codec
    }

    /// The next window this one's program opens, or `None` once that program is over.
    #[tracing::instrument(name = "window.children", skip_all)]
    async fn recv_new_child(&self) -> Option<Box<dyn Compositable>> {
        // A channel that closed is the session letting go of the window it belongs to, which is
        // its program being over: there is nothing more to open, and nothing wrong to report.
        let window = self.new_children.lock().await.recv().await.ok()?;
        WaylandWindow::new(window, self.codec).map(|window| Box::new(window) as Box<dyn Compositable>)
    }

    /// The next menu this one's program places inside it, or `None` once it is over.
    #[tracing::instrument(name = "window.popups", skip_all)]
    async fn recv_new_popup(&self) -> Option<Box<dyn Compositable>> {
        let window = self.new_popups.lock().await.recv().await.ok()?;
        WaylandWindow::new(window, self.codec).map(|window| Box::new(window) as Box<dyn Compositable>)
    }

    /// Where this window's program asked to sit inside the window that opened it.
    ///
    /// In the parent's pixels, because that is what the client's own surface is measured in. The
    /// conversion into the pane's cells is the trait's, so it happens once for every backend.
    fn offset_px(&self) -> std::io::Result<Option<(i32, i32)>> {
        Ok(*self
            .inner
            .upgrade()
            .ok_or_else(|| std::io::Error::other("the window is gone"))?
            .offset())
    }

    /// What the program calls its window: the title it set, or its app id when it set no title, or
    /// nothing when it set neither.
    fn title(&self) -> std::io::Result<String> {
        Ok(self
            .inner
            .upgrade()
            .ok_or_else(|| std::io::Error::other("the window is gone"))?
            .title()
            .unwrap_or_default())
    }

    /// Ask the program to close the window. It is a request: the client is told and decides, which
    /// is why the window may still be there afterwards.
    fn terminate(&self) -> std::io::Result<()> {
        self.inner
            .upgrade()
            .ok_or_else(|| std::io::Error::other("the window is gone"))?
            .terminate();
        Ok(())
    }
}

/// A window, the way the pane draws it: every cell filled with the window's own surface id,
/// which is the anchor the other side scans for.
impl ratatui::widgets::StatefulWidgetRef for WaylandWindow {
    type State = compositor::widgets::WindowState;

    fn render_ref(&self, area: Rect, buf: &mut Buffer, _state: &mut Self::State) {
        let id = self.surface;
        let style = Style::new()
            .fg(Color::Rgb(id.fg.0, id.fg.1, id.fg.2))
            .bg(Color::Rgb(id.bg.0, id.bg.1, id.bg.2));
        buf.set_style(area, style);
        for row in area.y..area.bottom() {
            for column in area.x..area.right() {
                if let Some(cell) = buf.cell_mut((column, row)) {
                    cell.set_char(id.symbol);
                }
            }
        }
    }
}
