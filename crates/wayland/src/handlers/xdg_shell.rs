//! `xdg_shell` handler: the window-tree admission and its configure.

use std::sync::Arc;

use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::protocol::wl_seat::WlSeat;
use smithay::utils::Serial;
use smithay::wayland::shell::xdg::{PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler};
use wayland_protocols::xdg::shell::server::xdg_toplevel;

use crate::client::{RootClient, SubClient};
use crate::state::State;
use crate::window::{UnifiedSurface, Window};

impl XdgShellHandler for State
{
    fn xdg_shell_state(&mut self) -> &mut smithay::wayland::shell::xdg::XdgShellState {
        &mut self.globals.xdg_shell_state
    }

    #[tracing::instrument(name = "wayland.new_toplevel", skip_all)]
    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        tracing::debug!("compositor: new toplevel requested");
        let client = surface
            .wl_surface()
            .client()
            .expect("toplevel always has a client");

        // Whose connection produced this toplevel: the root's or a sub's.
        // The first toplevel of a client is its root window; the toplevels
        // its program spawns afterwards are that window's children.
        if client.get_data::<RootClient>().is_some() {
            admit_into_root_client(self, surface);
            return;
        }
        if client.get_data::<SubClient>().is_some() {
            admit_into_sub(self, surface);
            return;
        }

        // XWayland's connection: its admitted surface is the root client's
        // window; an unadmitted X11 toplevel does not reach here (it comes
        // through the XWM).
        tracing::warn!("compositor: rejecting a toplevel of an unknown connection");
        surface.send_close();
    }

    #[tracing::instrument(name = "wayland.toplevel_destroyed", skip_all)]
    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        let Some(client) = surface.wl_surface().client() else { return };
        if let Some(root) = client.get_data::<RootClient>() {
            let was_root = {
                let tree = root.root_window.read().unwrap();
                tree.as_ref().is_some_and(|tree| {
                    tree.surface.wl_surface() == surface.wl_surface()
                })
            };
            if was_root {
                // The root window is gone: the program is over. The Weak
                // dies with the slot; the record drops (and kills the
                // child) when the connection does.
                *root.root_window.write().unwrap() = None;
                self.root_client = None;
                tracing::info!("compositor: the root window is gone; the root client is out");
            } else if let Some(tree) = root.root_window.read().unwrap().as_ref() {
                tree.remove_surface(surface.wl_surface());
            }
        } else if let Some(sub) = client.get_data::<SubClient>() {
            let was_root = {
                let tree = sub.root_window.read().unwrap();
                tree.as_ref().is_some_and(|tree| {
                    tree.surface.wl_surface() == surface.wl_surface()
                })
            };
            if !was_root {
                if let Some(tree) = sub.root_window.read().unwrap().as_ref() {
                    tree.remove_surface(surface.wl_surface());
                }
            } else {
                *sub.root_window.write().unwrap() = None;
            }
        }
    }

    /// A popup: a menu, a tooltip, a dropdown. It belongs to the window it was opened from and
    /// is placed inside it rather than beside it, which is why it is admitted as a child of
    /// that window and why its size comes from the client's positioner rather than from us.
    ///
    /// Position and size are taken from [`PositionerState::get_geometry`], which is the
    /// client's request — "below this rectangle, aligned left, offset by N" — resolved against
    /// the parent. Constraint adjustment (flipping a menu that would run off the edge) is not
    /// applied: that needs the parent's bounds, and a wrong guess puts the menu somewhere the
    /// client did not ask for. A menu that overhangs is at least where it was drawn.
    fn new_popup(&mut self, surface: PopupSurface, positioner: PositionerState) {
        let geometry = positioner.get_geometry();

        // Configured before it draws: a popup that is never configured has nothing to commit
        // against, and the client waits rather than drawing.
        let serial = match surface.send_configure() {
            Ok(serial) => serial,
            Err(e) => {
                // Reported and dropped: a popup that cannot be configured is one the client
                // will not draw, and there is nothing here to answer it with.
                log::warn!("a popup could not be configured: {e}");
                return;
            }
        };
        log::info!(
            "admitted a popup at ({}, {}) sized {}x{} (serial {})",
            geometry.loc.x,
            geometry.loc.y,
            geometry.size.w,
            geometry.size.h,
            u32::from(serial)
        );

        // The window it belongs to, found the way a commit finds its own: through the
        // connection that owns the parent surface. A popup that names no parent, or one whose
        // parent is not a window this session admitted, has nowhere to be placed.
        let parent_surface = surface.get_parent_surface();
        let parent = parent_surface
            .as_ref()
            .and_then(|parent| parent.client())
            .and_then(|client| {
                client
                    .get_data::<RootClient>()
                    .and_then(|root| root.root_window.read().unwrap().clone())
                    .or_else(|| {
                        client
                            .get_data::<SubClient>()
                            .and_then(|sub| sub.root_window.read().unwrap().clone())
                    })
            })
            .and_then(|tree| {
                parent_surface
                    .as_ref()
                    .and_then(|parent| crate::window::Window::commit_target(&tree, parent))
            });
        let Some(parent) = parent else {
            log::warn!("a popup arrived with no parent window; dropping it");
            return;
        };


        // Sized by its positioner, which is the one place a client states a size: a menu is as
        // big as its program said it would be, and that is the size it is drawn at until it
        // commits something. A positioner states everything in the parent's **logical** units,
        // and the compositor places and measures in physical pixels, so both the size and the
        // offset are multiplied by the session's scale on the way in.
        let physical = |logical: i32| ((f64::from(logical) * self.output_scale).round() as u32).max(1);
        let win = Window::new(
            UnifiedSurface::Popup(surface.clone()),
            (physical(geometry.size.w), physical(geometry.size.h)),
            self.output_scale,
        );
        let mut win = win;
        // Where it sits inside the parent, in the parent's own **physical** pixels. Clamped at
        // zero: a negative offset is legal — a menu may hang off the left edge — but nothing draws
        // outside the picture it is composited into.
        let scaled = |logical: i32| {
            ((f64::from(logical.max(0)) * self.output_scale).round() as i32).max(0)
        };
        win.offset = Some((scaled(geometry.loc.x), scaled(geometry.loc.y)));
        let win = Arc::new(win);

        // The serial of the configure just sent, so the client's ack can be matched to it.
        *win.last_configure_serial.lock().unwrap() = Some(u64::from(u32::from(serial)));

        parent.children.write().unwrap().push(win.clone());
        // Reported as a popup rather than a child: the holder places a child itself, and this
        // one's place came from the client.
        let _ = parent.new_popups.send(Arc::downgrade(&win));

        let held = self.pending_commits.remove(&surface.wl_surface().clone());
        apply_pending(held, &win);
        self.output.enter(surface.wl_surface());
    }

    /// A popup asking for the implicit pointer grab.
    ///
    /// Nothing here grabs anything: this side routes input to a surface by the box the pane drew
    /// it in, so a press inside a popup already reaches it. The request is not answered rather
    /// than ignored, because the seat is the only thing a grab could be taken from and this side
    /// never took one.
    fn grab(&mut self, _surface: PopupSurface, _seat: WlSeat, _serial: Serial) {}

    /// A popup went away: its client dismissed the menu, or its program closed it.
    ///
    /// The surface is gone, so nothing draws into it any more — but the window standing for it
    /// is still a child of whatever opened it, and a window with no surface keeps its place in
    /// the tree and keeps being read. Removing it here rather than relying on a weak handle
    /// expiring is what makes it go: the tree is what holds it alive.
    #[tracing::instrument(name = "wayland.popup_destroyed", skip_all)]
    fn popup_destroyed(&mut self, surface: PopupSurface) {
        let Some(client) = surface.wl_surface().client() else {
            return;
        };
        let tree = client
            .get_data::<RootClient>()
            .and_then(|root| root.root_window.read().unwrap().clone())
            .or_else(|| {
                client
                    .get_data::<SubClient>()
                    .and_then(|sub| sub.root_window.read().unwrap().clone())
            });
        // A popup whose parent is not a window this session admitted was never placed, so there
        // is nothing to remove: said out loud because it is indistinguishable from a popup that
        // stayed on screen.
        let Some(tree) = tree else {
            log::debug!("a popup was destroyed under a client with no window");
            return;
        };
        if !tree.remove_surface(surface.wl_surface()) {
            log::debug!("a popup was destroyed that this tree did not hold");
        }
    }

    fn reposition_request(
        &mut self,
        _surface: PopupSurface,
        _positioner: PositionerState,
        _token: u32,
    ) {
    }
}

/// The admission common to both client kinds: configure, activate, and build
/// the window that will hold the surface.
///
/// `physical_size` is the size this side proposes, which is its own decision and never the
/// client's: an xdg toplevel has no way to state one. A root window is proposed the output's
/// resolution because it fills the pane; every other window is proposed `default_window_size`
/// and is free to commit at whatever size it likes instead. What the configure carries is the
/// **logical** size, so the program lays its interface out `logical_scale` times larger than the
/// physical box.
fn admit(
    surface: &ToplevelSurface,
    physical_size: (u32, u32),
    logical_scale: f64,
    serial: Serial,
) -> Window {
    let win = Window::new(
        UnifiedSurface::Toplevel(surface.clone()),
        physical_size,
        logical_scale,
    );
    let logical = |px: u32| ((f64::from(px) / logical_scale).round() as i32).max(1);
    let (width, height) = (logical(physical_size.0), logical(physical_size.1));
    surface.with_pending_state(|pending| {
        pending.size = Some((width, height).into());
        pending.states.set(xdg_toplevel::State::Activated);
    });
    surface.send_configure();
    *win.last_configure_serial.lock().unwrap() = Some(u32::from(serial) as u64);
    win
}

/// Admit a toplevel into the root client's window tree.
///
/// The first toplevel is the client's root window and fills the output; later
/// ones are children, which are proposed the session's default window size.
fn admit_into_root_client(state: &mut State, surface: ToplevelSurface)
{
    let serial = state.next_serial();
    let root = state.root_client.as_ref().expect("root data seen");

    // Whether the root slot is occupied decides the configured size, so the
    // tree is consulted before the window is built. The root fills the pane,
    // so it is proposed the output; a child is proposed the default and may
    // commit at its own size instead.
    let has_root = root.root_window.read().unwrap().is_some();
    let size = if has_root {
        state.default_window_size
    } else {
        state.server_size
    };
    let win = admit(&surface, size, state.output_scale, serial);

    let mut tree = root.root_window.write().unwrap();
    let target = match tree.as_ref() {
        Some(tree) => {
            let admitted = Arc::new(win);
            tree.children.write().unwrap().push(admitted.clone());
            // Reported so the holder can wrap it: the tree is this crate's, and only the
            // announcement crosses over.
            let _ = tree.new_children.send(Arc::downgrade(&admitted));
            tree.clone()
        }
        None => {
            let admitted = Arc::new(win);
            *tree = Some(admitted.clone());
            admitted
        }
    };
    drop(tree);

    // A commit that arrived before this window existed was held for it: that picture is
    // the only one the client will draw until something asks it again, so it goes in
    // now. See `hold` in `handlers/compositor.rs`.
    let held = state.pending_commits.remove(&surface.wl_surface().clone());
    apply_pending(held, &target);

    // The window "fills" the output. Send `wl_surface.enter` so the client
    // knows an output is attached and can start presenting. Clients like
    // Firefox's WebRender withhold content until they see this.
    state.output.enter(surface.wl_surface());
    log::info!("admitted a window into the root client's tree");
}

/// Apply a commit held for a window that has just been admitted.
fn apply_pending(held: Option<(Vec<u8>, (u32, u32))>, tree: &Arc<Window>) {
    let Some((pixels, size)) = held else {
        return;
    };
    // Offered as a frame like any other, so the window's bookkeeping stays in one place. The
    // buffer is already released: this picture was read before there was anywhere to keep it.
    tree.adopt(pixels, size);
    log::info!("applied a held commit to the window it belongs to");
}

/// Admit a toplevel into a sub-client's window tree.
fn admit_into_sub(state: &mut State, surface: ToplevelSurface)
{
    // A sub-client's windows never fill the output: one window does, and it
    // is the root client's. They are proposed the default size, and a program
    // that wants another commits at it.
    let serial = state.next_serial();
    let client = surface.wl_surface().client().expect("seen above");
    let sub = client.get_data::<SubClient>().expect("seen above");
    let win = Arc::new(admit(
        &surface,
        state.default_window_size,
        state.output_scale,
        serial,
    ));

    let mut tree = sub.root_window.write().unwrap();
    let target = match tree.as_ref() {
        Some(tree) => {
            tree.children.write().unwrap().push(win.clone());
            let _ = tree.new_children.send(Arc::downgrade(&win));
            tree.clone()
        }
        None => {
            *tree = Some(win.clone());
            win
        }
    };
    drop(tree);
    // A sub-client's window is drawn as a window of the session, so the holder is told about it
    // the same way it is told about the root client's children. The announcement goes on the root
    // window's channel because there is one holder for the whole session, and it is reading that
    // one.
    if let Some(root) = state
        .root_client
        .as_ref()
        .and_then(|root| root.root_window.read().unwrap().clone())
    {
        let _ = root.new_children.send(Arc::downgrade(&target));
    }
    // The commit that raced this admission goes in now: it is the picture the client
    // drew when it was configured, and nothing else will ask it to draw.
    let held = state.pending_commits.remove(&surface.wl_surface().clone());
    apply_pending(held, &target);

    state.output.enter(surface.wl_surface());
    log::info!("admitted a window into a sub-client's tree");
}
