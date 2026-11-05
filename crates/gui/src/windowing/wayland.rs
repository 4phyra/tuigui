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

use std::sync::{Arc, Mutex, Weak};

use compositor::{Compositable, Error, WindowingServer};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use wayland::WaylandServer as Native;
use tokio::sync::broadcast;
use wire::message::{Message, SurfaceId};
use wire::transport::Connection;

/// The wayland session as a windowing server.
pub struct WaylandServer {
    /// The session.
    inner: Native,
    /// The codec every window of this session streams with: a property of the
    /// stream, and every window of one session shares it.
    codec: codecs::CodecId,
    /// Where the pictures go, until there is anything to send to.
    ///
    /// `None` until the other side dials, and no window exists before then: the dial
    /// is what starts the program, and a window is only ever minted once its program
    /// has drawn something. Every window minted afterwards takes a clone of this.
    maybe_connection: Option<Connection>,
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
    /// A plain field, not a lock: reading drains these, and the compositor reads them through
    /// `&mut` — it walks its stack mutably to place what has arrived.
    new_children: broadcast::Receiver<Weak<wayland::Window>>,
    new_popups: broadcast::Receiver<Weak<wayland::Window>>,
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
    /// Where this window's frames go.
    ///
    /// Not an `Option`: a window exists only because its program connected, and the
    /// dial that let it connect is the same one that handed this session its
    /// connection. A window with nowhere to send could not have been minted.
    connection: Connection,
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
    fn new(
        inner: Weak<wayland::Window>,
        codec: codecs::CodecId,
        connection: Connection,
    ) -> Option<Self> {
        let window = inner.upgrade()?;
        let surface = SurfaceId::random();
        // Reported and carried on with: a window that could not be announced draws its anchor
        // cells with no picture behind them, which is a window that failed rather than one that
        // does not exist.
        if let Err(e) = super::send(&connection, &Message::OpenWindow { surface, codec }) {
            log::warn!("a window could not be announced: {e}");
        }
        // Subscribed here, not at the first read: a subscription starts at the moment it is made,
        // so a child admitted while this wrapper is being built would otherwise be one nobody ever
        // hears about.
        let new_children = window.subscribe_new_children();
        let new_popups = window.subscribe_new_popups();
        Some(Self {
            inner,
            new_children,
            new_popups,
            surface,
            codec,
            encoder: Mutex::new(None),
            connection,
        })
    }
}

impl Drop for WaylandWindow {
    /// Tell the far side this window is over, so it can free the picture it holds.
    ///
    /// Dropping is the one moment a window is known to be gone: its program closed it, its client
    /// exited, or the tree that held it was torn down. Nothing polls for that, and nothing else
    /// names the surface afterwards, so a picture nobody releases stays on the pane for the rest
    /// of the session.
    ///
    /// Best effort and silent: there is no caller to report to, and a link that will not take
    /// this is a link whose pane is already lost.
    fn drop(&mut self) {
        let notice = Message::CloseWindow {
            surface: self.surface,
        };
        if let Err(e) = super::send(&self.connection, &notice) {
            log::warn!("a closed window could not be reported: {e}");
        }
    }
}

impl WaylandServer {
    /// Start the session.
    pub fn new(options: wayland::Options, codec: codecs::CodecId) -> std::io::Result<Self> {
        let inner = Native::new(options).map_err(|e| std::io::Error::other(e.to_string()))?;
        Ok(Self {
            inner,
            codec,
            maybe_connection: None,
        })
    }

    /// Hand the session the connection its pictures leave by.
    ///
    /// The dial is what starts the program, so this arrives before any window exists:
    /// every window minted afterwards takes a clone of it.
    ///
    /// TODO: make it impossible to call `spawn` with no connection. The order is what
    /// this relies on — a program started before the dial draws into a session with
    /// nowhere to send — and nothing in the types says so.
    pub fn set_connection(&mut self, connection: Connection) {
        self.maybe_connection = Some(connection);
    }
}

impl WindowingServer for WaylandServer {
    /// The root window, wrapped afresh on every call.
    ///
    /// Not kept: the session is what holds its windows, so asking is a lookup rather than a slot
    /// this side has to keep in step with it. A root that went away answers `None`, which is the
    /// same thing a missing one does.
    fn root_window(&mut self) -> Option<Box<dyn Compositable>> {
        let tree = self.inner.root_tree()?;
        // No connection means nothing has dialed, and a program that has not been started cannot
        // have a root window: the two arrive together.
        let connection = self.maybe_connection.clone()?;
        let window = WaylandWindow::new(Arc::downgrade(&tree), self.codec, connection)?;
        Some(Box::new(window))
    }

    fn spawn(
        &mut self,
        path: &std::path::Path,
    ) -> std::io::Result<Option<(std::process::ChildStdout, std::process::ChildStderr)>> {
        let (stdout, stderr) = self.inner.spawn(path)?;

        // The program's own words, drained rather than dropped. They are the whole diagnosis when
        // a client exits instead of drawing: "cannot open display", a missing driver, a path that
        // is not there — none of which this side can see any other way. A pipe nobody reads also
        // blocks its writer once it fills, which would turn a chatty program into a stalled one.
        for (which, pipe) in [
            ("out", Box::new(stdout) as Box<dyn std::io::Read + Send>),
            ("err", Box::new(stderr) as Box<dyn std::io::Read + Send>),
        ] {
            std::thread::spawn(move || {
                use std::io::BufRead as _;
                for line in std::io::BufReader::new(pipe).lines().map_while(Result::ok) {
                    log::info!("the program's {which}: {line}");
                }
            });
        }

        Ok(None)
    }

    #[tracing::instrument(name = "compositor.advance", skip_all)]
    fn advance(&mut self, timeout: std::time::Duration) {
        self.inner.step(timeout);
    }

    fn is_ready(&self) -> bool {
        self.inner.root_tree().is_some()
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

impl Compositable for WaylandWindow {
    /// Whether the session still holds the window this stands for.
    ///
    /// The one question a `Weak` asks, and the reason for holding one: a wrapper whose window is
    /// gone is not an error to report, it is the window being closed, and the holder drops it.
    fn is_alive(&self) -> bool {
        self.inner.strong_count() > 0
    }

    fn key_input(&self, event: ui_events::keyboard::KeyboardEvent) -> Result<(), Error> {
        self.inner.upgrade().ok_or(Error::Gone)?.queue_key(event);
        Ok(())
    }

    fn pointer_input(&self, event: ui_events::pointer::PointerEvent) -> Result<(), Error> {
        self.inner
            .upgrade()
            .ok_or(Error::Gone)?
            .queue_pointer(event);
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
    fn frame(&self, rank: u32, cells: (u16, u16)) -> Result<(), Error> {
        // The picture, if the program drew one. Nothing to take is the ordinary case for a
        // window nobody has touched since the last frame.
        let window = self.inner.upgrade().ok_or(Error::Gone)?;
        let captured = window.capture();
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

        super::send(
            &self.connection,
            &Message::Frame {
                surface: self.surface,
                z: rank,
                cols: u32::from(cells.0),
                rows: u32::from(cells.1),
                bytes,
            },
        )?;
        Ok(())
    }

    /// How big this window is, in pixels: what its program last committed, or the size it was
    /// adopted at before it drew.
    fn size_px(&self) -> Result<(u32, u32), Error> {
        Ok(self.inner.upgrade().ok_or(Error::Gone)?.size())
    }

    /// Ask the session's window to draw at a size. The next commit answers.
    ///
    /// Asked unconditionally: whether this is a size the window was already asked for is the
    /// caller's record, not this one's — the window knows what its program last committed at,
    /// which is the answer to the ask rather than the ask itself.
    #[tracing::instrument(name = "window.set_size", skip_all)]
    fn set_size(&self, size: (u32, u32)) -> Result<(), Error> {
        self.inner.upgrade().ok_or(Error::Gone)?.set_size(size);
        Ok(())
    }

    /// Whether this window is a popup, which its own program sized.
    fn is_popup(&self) -> bool {
        self.inner
            .upgrade()
            .is_some_and(|window| window.is_popup())
    }

    /// The windows this one's program has opened since the last read.
    ///
    /// Subscribed when the wrapper was made, so this reports what the window has gained: the
    /// compositor already holds everything from before. The queue carries `Weak`, which goes
    /// straight into the wrapper, and a window that died before its news was read is `None` there
    /// rather than something to check here.
    #[tracing::instrument(name = "window.children", skip_all)]
    fn get_new_children(&mut self) -> Vec<Box<dyn Compositable>> {
        let (codec, connection) = (self.codec, self.connection.clone());
        std::iter::from_fn(|| self.new_children.try_recv().ok())
            .filter_map(|window| WaylandWindow::new(window, codec, connection.clone()))
            .map(|window| Box::new(window) as Box<dyn Compositable>)
            .collect()
    }

    /// The windows this one's program has placed inside it since the last read: menus, tooltips,
    /// dropdowns.
    #[tracing::instrument(name = "window.popups", skip_all)]
    fn get_new_popups(&mut self) -> Vec<Box<dyn Compositable>> {
        let (codec, connection) = (self.codec, self.connection.clone());
        std::iter::from_fn(|| self.new_popups.try_recv().ok())
            .filter_map(|window| WaylandWindow::new(window, codec, connection.clone()))
            .map(|window| Box::new(window) as Box<dyn Compositable>)
            .collect()
    }

    /// Where this window's program asked to sit inside the window that opened it.
    ///
    /// In the parent's pixels, because that is what the client's own surface is measured in. The
    /// conversion into the pane's cells is the trait's, so it happens once for every backend.
    fn offset_px(&self) -> Result<Option<(i32, i32)>, Error> {
        Ok(*self.inner.upgrade().ok_or(Error::Gone)?.offset())
    }

    /// What the program calls its window: the title it set, or its app id when it
    /// set no title, or nothing when it set neither.
    fn title(&self) -> Result<String, Error> {
        Ok(self
            .inner
            .upgrade()
            .ok_or(Error::Gone)?
            .title()
            .unwrap_or_default())
    }

    /// Ask the program to close the window. It is a request: the client is told
    /// and decides, which is why the window may still be there afterwards.
    fn terminate(&self) -> Result<(), Error> {
        self.inner.upgrade().ok_or(Error::Gone)?.terminate();
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
