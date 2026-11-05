//! Per-window bookkeeping: the window tree and the toplevels it holds.
//!
//! The session keeps a **tree**: the root client's window, and the toplevels
//! its program spawned afterwards branching under it. Detaching a window
//! detaches its subtree with it.
//!
//! Windows are shared, not owned by their parent: a child lives in an `Arc`,
//! and weak handles to it are what reach past the session. What the session
//! mutates on a window sits behind locks (the children, the committed frame),
//! so a borrow of a window is as good as a mutable one.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, RwLock, Weak};

use derive_getters::Getters;
use derive_setters::Setters;
use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::shell::xdg::{
    PopupSurface, ToplevelSurface, XdgToplevelSurfaceData,
};
use tokio::sync::{broadcast, watch};
use ui_events::keyboard::KeyboardEvent;
use ui_events::pointer::PointerEvent;

use crate::state::State;

/// One frame a client committed, waiting to be read.
///
/// A value rather than three fields, because the three only mean anything together: the pixels
/// are of that size and came from that buffer, and a buffer released before its pixels are sent
/// is a frame the client overwrites while it is being read.
#[derive(Clone)]
pub(crate) struct Committed {
    /// B, G, R, A at the size the client drew.
    pub(crate) pixels: Vec<u8>,
    /// The size those pixels are.
    pub(crate) size: (u32, u32),
    /// The buffer they came from, held until this frame is presented.
    ///
    /// `wl_buffer.release` is the client's permission to draw into it again, and a client with
    /// a pool does not wait for its frame callback when it has one: released at commit, every
    /// commit earns another render immediately, so a client draws several frames between two
    /// of ours and all but the last are thrown away. Held until presentation, the release
    /// happens once per frame we actually send, which makes the client's rate ours.
    ///
    /// `None` for a frame that arrived before its window existed: that buffer was released
    /// when it was read, because there was nothing to hold it for.
    pub(crate) buffer: Option<WlBuffer>,
}

/// The surface a window is made of, and the role that makes it one.
///
/// A `wl_surface` is a canvas with no meaning until a role is attached to it, and two roles
/// exist here. They answer the same questions differently — a toplevel is sized by us and
/// placed on the pane, a popup is sized by the client's positioner and lives inside its parent
/// — which is why this is an enum and not one handle standing in for both.
#[derive(Clone)]
pub(crate) enum UnifiedSurface {
    Toplevel(ToplevelSurface),
    Popup(PopupSurface),
}

impl UnifiedSurface {
    /// The canvas this role draws into. Both roles own one, so this is the one question they
    /// answer the same way.
    pub(crate) fn wl_surface(&self) -> &WlSurface {
        match self {
            Self::Toplevel(toplevel) => toplevel.wl_surface(),
            Self::Popup(popup) => popup.wl_surface(),
        }
    }

    /// The toplevel behind this surface, if it is one.
    pub(crate) fn toplevel(&self) -> Option<&ToplevelSurface> {
        match self {
            Self::Toplevel(toplevel) => Some(toplevel),
            Self::Popup(_) => None,
        }
    }
}

/// One admitted window: its surface, its input mailbox, and the toplevels its
/// program spawned.
#[derive(Getters, Setters)]
#[setters(generate = false)]
pub struct Window {
    /// The surface, and the role that makes it a window.
    pub(crate) surface: UnifiedSurface,
    /// The last committed frame, and whether anyone has read it.
    ///
    /// A `watch` channel because the two questions are "what is new" and "is anything new",
    /// and it answers both: `send` keeps only the newest value, `has_changed` says whether the
    /// reader has seen it, and `borrow_and_update` takes it without a second call. A commit
    /// that replaces an unseen frame releases the one it replaced, so a client is never left
    /// waiting on a buffer whose picture nobody wanted.
    pub(crate) frame: watch::Sender<Option<Committed>>,
    /// Serial of the last configure sent (spawn waits for the first commit).
    pub(crate) last_configure_serial: Mutex<Option<u64>>,
    /// Where this window sits inside its parent, in the parent's pixels.
    ///
    /// Only a popup has one: a toplevel is placed on the pane by the compositor, which owns
    /// that decision, while a popup's position is the client's request resolved against its
    /// parent. `None` means "not a placement this carries" — the parent's own frame.
    pub(crate) offset: Option<(i32, i32)>,
    /// Toplevels this window's program spawned after being admitted, shared
    /// so weak handles can reach them from the session's holder. Detaching
    /// this window detaches its subtree with it.
    pub(crate) children: RwLock<Vec<Arc<Window>>>,
    /// The broadcast that reports children and popups as they are admitted, as weak handles.
    ///
    /// `broadcast` rather than a queue because the reader may join at any time: the holder asks
    /// for what is new on every frame, and a subscription starts from the point it was made —
    /// which is what lets a window be read without the side that admits children knowing who is
    /// listening.
    ///
    /// `Weak` because this is news, not ownership: the tree owns its children, and one that died
    /// before the news was read simply fails to upgrade.
    ///
    /// Two of them because the two are asked separately — what the compositor places, and what
    /// this window's own program placed — and one would make every reader sort them out again.
    pub(crate) new_children: broadcast::Sender<Weak<Window>>,
    pub(crate) new_popups: broadcast::Sender<Weak<Window>>,
    /// Key events queued by the holder, drained at the session's next step.
    ///
    /// A plain queue behind a mutex rather than a channel: nothing waits on a
    /// receiver, so a channel's single-consumer machinery only buys the type
    /// `!Sync`.
    ///
    /// Keys and pointers are queued apart because they are not the same kind of
    /// event: a key is a position and a state, while a pointer event carries a
    /// whole state. One queue would only mean sorting them out again.
    pub(crate) keys: Mutex<VecDeque<KeyboardEvent>>,
    /// Pointer events queued by the holder.
    pub(crate) pointers: Mutex<VecDeque<PointerEvent>>,
    /// The size this window is, in pixels: what its program last committed, or the size it was
    /// adopted at before it drew.
    ///
    /// A fact about the window rather than about what is new about it, which is why it is not
    /// part of the committed frame: reading the picture empties that, and the size stays true
    /// until the program draws again. Never absent, because a window always has one — the size it
    /// was configured for, or for a popup the size its positioner asked for — so there is nothing
    /// for a caller to interpret an absence of.
    ///
    /// Read through [`Window::size`] rather than the field: the lock is this window's business,
    /// and a handle to it handed out is a lock the holder has to remember to release.
    #[getter(skip)]
    pub(crate) size: Mutex<(u32, u32)>,
}

impl Window {
    /// This window's size in pixels: what its program last committed, or the size it was adopted
    /// at before it drew.
    pub fn size(&self) -> (u32, u32) {
        *self.size.lock().unwrap_or_else(|e| e.into_inner())
    }
    /// A window that is `size` pixels until its program commits something else.
    pub(crate) fn new(surface: UnifiedSurface, size: (u32, u32)) -> Self {
        Window {
            surface,
            frame: watch::channel(None).0,
            last_configure_serial: Mutex::new(None),
            offset: None,
            children: RwLock::new(Vec::new()),
            // Capacity is what a reader may fall behind by before it is told it missed
            // something. A window is admitted rarely — a program opening another, a menu
            // appearing — so a handful is already more than a frame's worth, and the holder
            // subscribes afresh rather than following one stream.
            new_children: broadcast::channel(8).0,
            new_popups: broadcast::channel(8).0,
            keys: Mutex::new(VecDeque::new()),
            pointers: Mutex::new(VecDeque::new()),
            size: Mutex::new(size),
        }
    }

    /// This window's size in pixels: what its program last committed, or the size it was adopted
    /// at before it drew anything.
    ///
    /// Generated by [`Getters`](derive_getters::Getters) from the field of the same name, which
    /// is a `Mutex` because a commit writes it while the compositor is reading it.

    /// Queue one key event; drained by the session's next step.
    pub fn queue_key(&self, event: KeyboardEvent) {
        self.keys.lock().unwrap().push_back(event);
    }

    /// Queue one pointer event; drained by the session's next step.
    pub fn queue_pointer(&self, event: PointerEvent) {
        self.pointers.lock().unwrap().push_back(event);
    }

    /// Offer one committed frame, releasing whatever it replaces.
    ///
    /// The replaced frame is one nobody read: the client drew again before the picture it had
    /// already drawn was sent. Its buffer is released here rather than dropped, because a
    /// client waiting on that release would otherwise never draw again.
    pub(crate) fn commit(&self, frame: Committed) {
        *self.size.lock().unwrap_or_else(|e| e.into_inner()) = frame.size;
        let replaced = self.frame.send_replace(Some(frame));
        if let Some(replaced) = replaced.and_then(|replaced| replaced.buffer) {
            replaced.release();
        }
    }

    /// Offer pixels that arrived before this window existed.
    ///
    /// The buffer is already gone — released when it was read, because there was nothing to
    /// hold it for — so this is the one frame with no release waiting on presentation.
    pub(crate) fn adopt(&self, pixels: Vec<u8>, size: (u32, u32)) {
        *self.size.lock().unwrap_or_else(|e| e.into_inner()) = size;
        let _ = self.frame.send_replace(Some(Committed {
            pixels,
            size,
            buffer: None,
        }));
    }

    /// What the window last drew, if it has drawn anything the reader has not taken.
    ///
    /// Taking is the read: the offer is emptied, so a second call with no commit in between
    /// answers `None` instead of sending the same picture again.
    #[tracing::instrument(name = "wayland.capture", skip_all)]
    pub fn capture(&self) -> Option<(Vec<u8>, (u32, u32))> {
        // Taken, not read: the offer is emptied by the read, so the next call answers `None`
        // unless something committed in between. That is what "anything new" means here, and a
        // receiver would not answer it — `subscribe` starts at the current value and considers
        // it already seen.
        let frame = self.frame.send_replace(None)?;
        // Both answers to "you may draw again", given here rather than at commit. The pixels
        // are already copied out, so releasing the buffer cannot disturb them: the client
        // renders the next frame into it, and that frame is read when this is called again.
        // One commit in flight, so a client cannot get further ahead than one frame.
        if let Some(buffer) = frame.buffer {
            buffer.release();
        }
        crate::utils::fire_frame_callbacks(self.surface.wl_surface());
        Some((frame.pixels, frame.size))
    }

    /// Ask this window to draw at `size` pixels instead of its current one.
    /// The client answers by committing a buffer at whatever size it chose.
    ///
    /// Only a toplevel can be asked: a popup's size comes from its positioner, which the client
    /// owns. Asking one would be proposing a size it has no way to accept.
    pub fn set_size(&self, size: (u32, u32)) {
        let Some(toplevel) = self.surface.toplevel() else {
            log::info!("compositor: a popup was asked for {:?}; its positioner owns its size", size);
            return;
        };
        log::info!("compositor: asking the program for {:?}", size);
        toplevel.with_pending_state(|pending| {
            pending.size = Some((size.0 as i32, size.1 as i32).into());
        });
        toplevel.send_configure();
    }

    /// What this window calls itself: the title its program set, or the app id it
    /// set instead. `None` when it set neither, which is a window with no name
    /// rather than one with an invented name.
    pub fn title(&self) -> Option<String> {
        smithay::wayland::compositor::with_states(self.surface.wl_surface(), |states| {
            let attributes = states.data_map.get::<XdgToplevelSurfaceData>()?;
            let attributes = attributes.lock().unwrap();
            attributes
                .title
                .clone()
                .or_else(|| attributes.app_id.clone())
        })
    }

    /// Whether this window is a popup: a menu, a tooltip, a dropdown.
    ///
    /// The distinction that matters to the compositor, because a popup's size is its client's
    /// positioner's to state and cannot be asked for. A dialog is a toplevel, however much it
    /// looks like a small window, and is resized like any other.
    pub fn is_popup(&self) -> bool {
        matches!(self.surface, UnifiedSurface::Popup(_))
    }

    /// Ask the program to close this window: an xdg close request, which the
    /// client may ignore. The window outlives this call until it does not.
    pub fn terminate(&self) {
        match &self.surface {
            UnifiedSurface::Toplevel(toplevel) => toplevel.send_close(),
            // A popup is dismissed, not asked to close: it belongs to its parent's menu, and
            // the client tears it down when it is told the popup is done.
            UnifiedSurface::Popup(popup) => popup.send_popup_done(),
        }
    }

    /// Where this window asked to sit inside its parent, in the parent's own pixels.
    ///
    /// Only a popup has an answer: a toplevel's position is the compositor's to decide, so `None`
    /// there means "somewhere you choose" rather than "the top left". Clamped at zero when it was
    /// recorded, because a negative offset is legal — a menu may hang off an edge — but nothing
    /// draws outside the picture it is composited into.
    ///
    /// Generated by [`Getters`](derive_getters::Getters) from the field of the same name; the
    /// doc lives here because that is where the meaning is.

    /// Subscribe to the children this window gains from now on.
    ///
    /// Afresh for each reader, which is what a broadcast is for: the holder asks once per frame
    /// and gets the window's answer, instead of the window having to know who is listening. A
    /// subscription starts at the moment it is made, so nothing earlier is replayed — what a
    /// reader is late for is the frame it did not draw.
    pub fn subscribe_new_children(&self) -> broadcast::Receiver<Weak<Window>> {
        self.new_children.subscribe()
    }

    /// Subscribe to the popups this window's program places from now on.
    ///
    /// Apart from [`Window::subscribe_new_children`] because the two are placed differently: a
    /// child is put somewhere by the compositor, a popup by the client that asked for it.
    pub fn subscribe_new_popups(&self) -> broadcast::Receiver<Weak<Window>> {
        self.new_popups.subscribe()
    }

    /// Everything under this window: the windows its program opened after it,
    /// then the root windows of the helper clients that program started.
    ///
    /// A helper's window lives beside this one rather than inside it, but it
    /// is drawn as a window of the session all the same, so it is read here.
    pub fn descendants(&self) -> Vec<Arc<Window>> {
        let mut all = self.children.read().unwrap().clone();
        for sub in self.sub_clients() {
            if let Some(tree) = sub.root_window.read().unwrap().clone() {
                all.push(tree);
            }
        }
        all
    }

    /// The helper clients this window's program started, found through the
    /// connection that owns this window.
    fn sub_clients(&self) -> Vec<Arc<crate::client::SubClient>> {
        use smithay::reexports::wayland_server::Resource as _;
        let Some(client) = self.surface.wl_surface().client() else {
            return Vec::new();
        };
        // Copied out: the guard `get_data` hands back borrows a handle that
        // lives only until the end of this statement.
        let Some(root) = client.get_data::<crate::client::RootClient>() else {
            return Vec::new();
        };
        let subs: Vec<_> = root
            .sub_clients
            .read()
            .unwrap()
            .iter()
            .filter_map(std::sync::Weak::upgrade)
            .collect();
        subs
    }

    /// Dispatch this window's queued inputs, then its children's.
    ///
    /// The tree is walked shallow-to-deep; each window dispatches its own
    /// mailbox against itself, so an input reaches the window it was queued for.
    ///
    /// Keys go first, and as a group: a modified key is several key events that
    /// only mean something in order, so slipping a pointer event between the
    /// control press and the `c` press would leave the modifier dangling.
    pub(crate) fn dispatch_inputs(&self, state: &mut State) {
        let keys: Vec<KeyboardEvent> = self.keys.lock().unwrap().drain(..).collect();
        for event in &keys {
            if let Err(e) = state.dispatch_key_input(self.surface.wl_surface(), event) {
                tracing::debug!("a queued key did not dispatch: {e}");
            }
        }
        // What this window's queue did not carry is a key nobody renewed: the terminal
        // has no way to say a release, so letting go is the only thing that stops the
        // program repeating it forever. Only the focused window does this — the guard is
        // inside, because the pressed keys belong to the seat and not to a window.
        state.release_unrenewed(self.surface.wl_surface(), &keys);
        let pointers: Vec<PointerEvent> = self.pointers.lock().unwrap().drain(..).collect();
        for event in pointers {
            if let Err(e) = state.dispatch_mouse_input(self.surface.wl_surface(), &event) {
                tracing::debug!("a queued pointer event did not dispatch: {e}");
            }
        }
        for child in &*self.children.read().unwrap() {
            child.dispatch_inputs(state);
        }
    }

    /// The window whose own surface is `surface`, if any in this tree.
    ///
    /// Only the window's *own* surface, not a subsurface of it. A window has one picture store
    /// and it is the whole pane, so a child's buffer written into it does not composite — it
    /// replaces the parent's frame outright, and the pane shows whichever surface committed
    /// last. Until children have somewhere of their own to land, they are dropped rather than
    /// allowed to overwrite: a missing decoration is a smaller fault than a frame that
    /// flickers between two buffers.
    pub(crate) fn commit_target(this: &Arc<Self>, surface: &WlSurface) -> Option<Arc<Self>> {
        if this.surface.wl_surface() == surface {
            return Some(this.clone());
        }
        for child in &*this.children.read().unwrap() {
            if let Some(found) = Self::commit_target(child, surface) {
                return Some(found);
            }
        }
        None
    }

    /// Remove the window whose surface is `surface` (a child, or a child's
    /// child). Returns whether something was removed.
    pub(crate) fn remove_surface(&self, surface: &WlSurface) -> bool {
        let mut children = self.children.write().unwrap();
        let before = children.len();
        children.retain(|child| child.surface.wl_surface() != surface);
        let mut removed = children.len() != before;
        for child in &*children {
            removed |= child.remove_surface(surface);
        }
        drop(children);
        removed
    }
}
