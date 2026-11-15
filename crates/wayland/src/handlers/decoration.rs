//! `xdg-decoration-unstable-v1`: the answer to "who draws the titlebar".
//!
//! The protocol cannot be enforced — a client's client-side decorations are its own
//! surface content, and Wayland has no field that says otherwise — so the whole of this
//! side is the answer to the one question the client may ask. It always asks for
//! [`Mode::ServerSide`], because every window here is drawn by this session: there is no
//! frame, no shadow and no grab strip a client could draw that this side would agree to
//! place, and a client that draws them anyway has made a choice this side cannot undo.

use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode;
use smithay::wayland::shell::xdg::decoration::XdgDecorationHandler;
use smithay::wayland::shell::xdg::ToplevelSurface;

use crate::state::State;

impl XdgDecorationHandler for State {
    /// The client made a decoration object for this toplevel: answer it before it draws.
    ///
    /// The answer rides a configure, which the client needs anyway, so this is one
    /// message rather than a separate round trip: `send_configure` emits the decoration
    /// mode when it has changed or was never sent, and the toplevel's own pending state
    /// is where the mode comes from.
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        tracing::info!("compositor: a toplevel asked for decorations; answering server-side");
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(Mode::ServerSide);
        });
        toplevel.send_configure();
    }

    /// The client stated a preference. It is still answered with server-side.
    ///
    /// A client that asks for `ClientSide` cannot be made to stop drawing, but it can be
    /// told what this session wants, and most toolkits drop their own chrome on the next
    /// buffer when the answer disagrees with the request.
    fn request_mode(&mut self, toplevel: ToplevelSurface, mode: Mode) {
        tracing::debug!("compositor: a toplevel requested {mode:?} decoration");
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(Mode::ServerSide);
        });
        toplevel.send_configure();
    }

    /// The client withdrew its preference. The session's answer does not change.
    ///
    /// Nothing is sent: the client said it no longer cares, so it is not waiting on an
    /// answer, and a configure nobody asked for is a resize it would have to handle.
    fn unset_mode(&mut self, _toplevel: ToplevelSurface) {
        tracing::debug!("compositor: a toplevel withdrew its decoration preference");
    }
}
