//! `wl_compositor` handler: buffer commits become owned pixels.


use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::compositor::{
    BufferAssignment, CompositorHandler, CompositorState, SurfaceAttributes,
};

use crate::state::State;
use crate::utils::map_rgba;

impl CompositorHandler for State
{
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.globals.compositor_state
    }

    fn client_compositor_state<'a>(
        &self,
        client: &'a smithay::reexports::wayland_server::Client,
    ) -> &'a smithay::wayland::compositor::CompositorClientState {
        crate::client::compositor_client_state(client)
            .expect("client has neither RootClient, SubClient nor XWaylandClientData")
    }

    #[tracing::instrument(name = "wayland.commit", skip_all, fields(surface = ?surface.id()))]
    fn commit(&mut self, surface: &WlSurface) {
        // Software compositing: read the committed buffer straight into the
        // owning client's window and accumulate damage. (No renderer-side
        // on_commit_buffer_handler — we don't run a GPU renderer.)

        // Whose connection produced this commit: the root's, a sub's, or
        // XWayland's.
        let client = match surface.client() {
            Some(client) => client,
            None => return,
        };
        if let Some(root) = client.get_data::<crate::client::RootClient>() {
            let tree = root.root_window.read().unwrap().clone();
            match tree.and_then(|tree| crate::window::Window::commit_target(&tree, surface)) {
                Some(win) => commit_into(&win, surface),
                None => hold(self, surface),
            }
        } else if let Some(sub) = client.get_data::<crate::client::SubClient>() {
            let tree = sub.root_window.read().unwrap().clone();
            match tree.and_then(|tree| crate::window::Window::commit_target(&tree, surface)) {
                Some(win) => commit_into(&win, surface),
                // A client may commit between being configured and being admitted:
                // `xdg-shell` sends the configure while the window is still being built,
                // and this commit arrives before the window exists. Holding it is what
                // gives the window its first picture — nothing else would ask the client
                // to draw again, because frame callbacks are fired by `capture`, which
                // only runs once a window exists.
                None => hold(self, surface),
            }
        } else {
            // XWayland's connection: its admitted surface is the root
            // client's window.
            let admitted = self
                .root_client
                .as_ref()
                .and_then(|root| root.root_window.read().unwrap().clone())
                .map(|tree| tree.surface.wl_surface().clone());
            if let Some(admitted) = admitted {
                let is_win = admitted == *surface;
                let is_kid = !is_win
                    && smithay::wayland::compositor::get_parent(surface)
                        .map(|p| p == admitted)
                        .unwrap_or(false);
                if !(is_win || is_kid) {
                    return;
                }
                if let Some(win) = self
                    .root_client
                    .as_ref()
                    .and_then(|root| root.root_window.read().unwrap().clone())
                {
                    commit_into(&win, surface);
                }
            }
        }
    }
}

/// Hold a commit whose surface has no window yet.
///
/// The pixels are copied out now, because the buffer is released immediately after the
/// commit: waiting until the window exists would mean reading a buffer the client has
/// already been told to reuse.
///
/// The buffer is still released here for the same reason — a client waiting on
/// `wl_buffer.release` would otherwise never draw again, and the window would be waiting
/// for a picture from a client that is waiting for its buffer.
#[tracing::instrument(name = "wayland.hold", skip_all, fields(surface = ?surface.id()))]
fn hold(state: &mut State, surface: &WlSurface) {
    // A surface with a parent is a subsurface: client-side decorations, a tooltip, a cursor. It
    // belongs to the window that owns it rather than being a window of its own, so it will never
    // be admitted — copying its picture would be a leak that grows once per commit.
    //
    // Its buffer is still released: a buffer held is a client that never draws again, and for a
    // subsurface that is every decoration the program paints.
    if smithay::wayland::compositor::get_parent(surface).is_some() {
        smithay::wayland::compositor::with_states(surface, |states| {
            let mut attrs = states.cached_state.get::<SurfaceAttributes>();
            if let Some(BufferAssignment::NewBuffer(buffer)) = &attrs.current().buffer {
                buffer.release();
            }
        });
        return;
    }

    let mut pixels = Vec::new();
    let mut size = (0, 0);
    let mut release: Option<smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer> =
        None;
    smithay::wayland::compositor::with_states(surface, |states| {
        let mut attrs = states.cached_state.get::<SurfaceAttributes>();
        if let Some(BufferAssignment::NewBuffer(buffer)) = &attrs.current().buffer {
            match map_rgba(buffer, &mut pixels) {
                Ok(mapped) => {
                    size = mapped;
                    release = Some(buffer.clone());
                }
                Err(e) => log::warn!("a committed buffer could not be read: {e}"),
            }
        }
    });
    if let Some(buffer) = release {
        buffer.release();
    }
    if pixels.is_empty() {
        // No buffer in this commit: it was a state change, not a picture. Nothing to
        // hold, and no reason to say so.
        return;
    }
    log::debug!("holding a commit from a surface whose window is not built yet");
    state.pending_commits.insert(surface.clone(), (pixels, size));
}

#[tracing::instrument(name = "wayland.commit_into", skip_all, fields(surface = ?surface.id()))]
fn commit_into(win: &crate::window::Window, surface: &WlSurface) {
    let mut frame = None;
    smithay::wayland::compositor::with_states(surface, |states| {
        let mut attrs = states.cached_state.get::<SurfaceAttributes>();
        // Taken out of the surface's state, not merely copied from it. Smithay's `merge_into`
        // releases whatever buffer it replaces in `current`, and `apply_state` does that
        // before this handler runs — so a buffer left in place is released one commit later,
        // which is the eager release this exists to withhold.
        let attached = match attrs.current().buffer.as_ref() {
            Some(BufferAssignment::NewBuffer(buffer)) => Some(buffer.clone()),
            Some(BufferAssignment::Removed) | None => None,
        };
        let Some(attached) = attached else { return };

        // Mapped at the size the client drew, which is the only size it has: nothing here
        // chooses it, and the picture is placed by the pane rather than stretched by this side.
        let mut pixels = Vec::new();
        match map_rgba(&attached, &mut pixels) {
            Ok(size) => {
                attrs.current().buffer = None;
                frame = Some(crate::window::Committed {
                    pixels,
                    size,
                    buffer: Some(attached),
                });
            }
            Err(e) => log::warn!("a committed buffer could not be read: {e}"),
        }
    });

    // Offered to the window, which releases whatever frame this replaces: a picture drawn
    // again before the one before it was sent is a buffer its client is waiting on.
    if let Some(frame) = frame {
        win.commit(frame);
    }
}
