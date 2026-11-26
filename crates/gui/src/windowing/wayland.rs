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

use std::sync::Weak;

use compositor::Error;
use tokio::sync::broadcast;
use wayland::WaylandServer as Native;

use super::{Compositable, WindowingServer};

/// What a session is started with, as the backend this machine has answers it.
pub use wayland::Options;

/// The wayland session as a windowing server.
pub struct WaylandServer {
    /// The session.
    inner: Native,
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
    #[cfg_attr(feature = "tracy", tracing::instrument(skip_all))]
    fn new(inner: Weak<wayland::Window>) -> Option<Self> {
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
        })
    }
}

impl Drop for WaylandWindow {
    /// Nothing to tell the far side, and nothing to release.
    ///
    /// The picture is named by a surface id that travels with every frame, and the far side frees
    /// it when the frames stop; a window that is over is one whose `Weak` no longer upgrades.
    #[cfg_attr(feature = "tracy", tracing::instrument(skip_all))]
    fn drop(&mut self) {}
}

impl WaylandServer {
    /// Start the session.
    ///
    /// The wait for the window the program draws is the caller's, taken from the session's own
    /// channel by [`Self::take_root`]: only a turn notices a client has committed, so the news can
    /// only come from the session, and the wait can only live where it is awaited.
    #[cfg_attr(feature = "tracy", tracing::instrument(skip_all))]
    pub fn new(options: wayland::Options) -> std::io::Result<Self> {
        let inner = Native::new(options).map_err(|e| std::io::Error::other(e.to_string()))?;
        Ok(Self {
            inner,
            stdout: None,
        })
    }

    /// The wait for the window the program drew, owning what it waits on.
    ///
    /// Owned rather than borrowed, because the caller has to keep turning the session while it
    /// waits: a future holding the session would make the very turn that answers it impossible.
    #[cfg_attr(feature = "tracy", tracing::instrument(skip_all))]
    pub fn take_root(
        &mut self,
    ) -> std::io::Result<impl std::future::Future<Output = std::io::Result<Box<dyn Compositable>>>>
    {
        let root = self
            .inner
            .take_root_receiver()
            .ok_or_else(|| std::io::Error::other("the root window is handed over once"))?;
        Ok(async move {
            let root = root
                .await
                .map_err(|_| std::io::Error::other("the session ended before it drew a window"))?;
            WaylandWindow::new(root)
                .map(|window| Box::new(window) as Box<dyn Compositable>)
                .ok_or_else(|| std::io::Error::other("the root window is already gone"))
        })
    }
}

impl WindowingServer for WaylandServer {
    #[cfg_attr(feature = "tracy", tracing::instrument(skip_all))]
    fn spawn(&mut self, path: &std::path::Path) -> std::io::Result<Option<std::io::PipeReader>> {
        // The program's own words, kept until someone takes them: what a program says belongs to
        // whoever shows it, and this side only started it. Both of its streams arrive here, in
        // the order it wrote them, because the session gives the child one pipe for both.
        self.stdout = Some(self.inner.spawn(path)?);
        Ok(None)
    }

    #[cfg_attr(feature = "tracy", tracing::instrument(skip_all))]
    fn take_stdout(&mut self) -> Option<std::io::PipeReader> {
        self.stdout.take()
    }

    #[cfg_attr(feature = "tracy", tracing::instrument(name = "compositor.advance", skip_all))]
    fn advance(&mut self, timeout: std::time::Duration) {
        self.inner.step(timeout);
    }

    #[cfg_attr(feature = "tracy", tracing::instrument(skip_all))]
    fn is_running(&self) -> bool {
        self.inner.is_running()
    }
}

// The macro, because the trait is written with it: an `async fn` in a trait the compositor holds
// as `dyn` is desugared into a boxed future, and an implementation spelled the language's own way
// describes a different signature than the one the vtable points at.
#[async_trait::async_trait]
impl Compositable for WaylandWindow {
    #[cfg_attr(feature = "tracy", tracing::instrument(skip_all))]
    fn key_input(&self, event: termina::event::KeyEvent) -> std::io::Result<()> {
        let window = self
            .inner
            .upgrade()
            .ok_or_else(|| std::io::Error::other("the window is gone"))?;
        window.queue_key(crate::windowing::event_map::key(&event));
        Ok(())
    }

    #[cfg_attr(feature = "tracy", tracing::instrument(skip_all))]
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
    /// Take the picture the window drew.
    ///
    /// The pixels, exactly as the program committed them: encoding is the sender's business, and
    /// a window that encoded for itself would be a window that knows about the link.
    ///
    /// It waits for the client's next commit when nothing has been drawn since the last take,
    /// which is what makes this the frame clock: the caller's `select!` wakes when a picture
    /// exists, not on a tick. `None` is a window the session has let go.
    #[cfg_attr(feature = "tracy", tracing::instrument(name = "window.capture", skip_all))]
    async fn capture(&self) -> std::io::Result<Option<Vec<u8>>> {
        let window = self.inner.upgrade().ok_or(Error::Gone)?;
        Ok(window.capture().await.map(|(pixels, _size)| pixels))
    }

    /// How big this window is, in pixels: what its program last committed, or the size it was
    /// adopted at before it drew.
    #[cfg_attr(feature = "tracy", tracing::instrument(skip_all))]
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
    #[cfg_attr(feature = "tracy", tracing::instrument(name = "window.set_size", skip_all))]
    fn set_size(&self, size: (u32, u32)) -> std::io::Result<()> {
        self.inner
            .upgrade()
            .ok_or_else(|| std::io::Error::other("the window is gone"))?
            .set_size(size);
        Ok(())
    }

    /// Whether this window is a popup, which its own program sized.
    #[cfg_attr(feature = "tracy", tracing::instrument(skip_all))]
    fn is_popup(&self) -> bool {
        self.inner
            .upgrade()
            .is_some_and(|window| window.is_popup())
    }

    /// The next window this one's program opens, or `None` once that program is over.
    #[cfg_attr(feature = "tracy", tracing::instrument(name = "window.children", skip_all))]
    async fn recv_new_child(&self) -> Option<Box<dyn Compositable>> {
        // A channel that closed is the session letting go of the window it belongs to, which is
        // its program being over: there is nothing more to open, and nothing wrong to report.
        let window = self.new_children.lock().await.recv().await.ok()?;
        WaylandWindow::new(window)
            .map(|window| Box::new(window) as Box<dyn Compositable>)
    }

    /// The next menu this one's program places inside it, or `None` once it is over.
    #[cfg_attr(feature = "tracy", tracing::instrument(name = "window.popups", skip_all))]
    async fn recv_new_popup(&self) -> Option<Box<dyn Compositable>> {
        let window = self.new_popups.lock().await.recv().await.ok()?;
        WaylandWindow::new(window)
            .map(|window| Box::new(window) as Box<dyn Compositable>)
    }

    /// Where this window's program asked to sit inside the window that opened it.
    ///
    /// In the parent's pixels, because that is what the client's own surface is measured in. The
    /// conversion into the pane's cells is the trait's, so it happens once for every backend.
    #[cfg_attr(feature = "tracy", tracing::instrument(skip_all))]
    fn offset_px(&self) -> std::io::Result<Option<(i32, i32)>> {
        Ok(*self
            .inner
            .upgrade()
            .ok_or_else(|| std::io::Error::other("the window is gone"))?
            .offset())
    }

    /// What the program calls its window: the title it set, or its app id when it set no title, or
    /// nothing when it set neither.
    #[cfg_attr(feature = "tracy", tracing::instrument(skip_all))]
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
    #[cfg_attr(feature = "tracy", tracing::instrument(skip_all))]
    fn terminate(&self) -> std::io::Result<()> {
        self.inner
            .upgrade()
            .ok_or_else(|| std::io::Error::other("the window is gone"))?
            .terminate();
        Ok(())
    }
}

