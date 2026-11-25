# COMP — tuigui's own Wayland compositor

Status: built. The headless session is `crates/wayland` (Smithay, pumped by
`WaylandServer::step`); the window tree and placement are `crates/compositor`.
Capture is shm-only. XWayland is wired up (`crates/wayland/src/handlers/xwayland.rs`)
but whether it is exercised is unverified.

## Why

The ancestor of this was `crates/cage`, an external wlroots `cage` in headless
mode with a separate screencopy client and virtual input devices — a process
boundary with protocol round-trips in both directions. It is gone. Owning the
compositor deletes the boundary: the frame being presented is already in our
address space, so "capture" is a buffer read, and input injection is a direct
event dispatch into the seat.

## Shape

What it is, from the caller's side — `gui` wraps all of this in
`crates/gui/src/windowing/wayland.rs`:

```rust
let mut server = WaylandServer::new(options)?;   // headless, private socket
server.spawn(path)?;                             // the child's WAYLAND_DISPLAY points at it
server.step(timeout);                            // the caller's loop is the session's heartbeat
let frame = window.capture();                    // the newest committed frame, if any
```

`spawn` launches the binary with `WAYLAND_DISPLAY` pointing at the
compositor's private socket. The first committed window is handed over through
`take_root_receiver`, so the caller never sees a half-alive client. `capture`
lives on `crates/wayland`'s `Window` and hands out the newest committed frame.

## Who uses it

`gui` is the only caller: given a program to run, it starts the session, runs
the program inside it, and sends the captured window as frames
(`crates/gui/src/windowing/wayland.rs`, over `crates/wayland`).

## What the capture can and cannot read

`capture()` copies **wl_shm buffers**. A client that renders through EGL/dmabuf
(kitty, foot's GL path, `weston-flower`) commits a dma-buf instead, which is not
readable this way: the window runs, but every captured frame is the empty
output. There is no GL capture path yet and no dma-buf code anywhere in the
tree as of 2026-11; where one would live is unverified.

## Window policy

**The first toplevel is the window; a second one is a sibling, not a rejection.**
`admit_into_root_client` (`crates/wayland/src/handlers/xdg_shell.rs`) admits
every toplevel into the root client's tree: the first fills the pane, later
ones are proposed the default size and placed by `crates/compositor` (centred,
stacked above, each with its own rank). Popups are admitted the same way and
placed inside their parent's box at the offset their positioner asked for.
Subsurfaces of a window are not placed at all today: a child surface's buffer
is dropped rather than composited, on the grounds that a missing decoration is
a smaller fault than a frame that flickers.

## Architecture

### Backend

Software compositing only — there is no GPU renderer. Smithay's compositor,
xdg-shell, shm, seat and XWayland frontends are driven by `WaylandServer::step`,
which the caller's loop calls; there is no dedicated compositor thread. The
output is created once, headless, at the size in `Options::resolution`, with a
frame-callback emulator firing `done()` so clients keep drawing.

The size a window is configured with is `Options::default_window_size` (the
pane, for the root; the default window size for later toplevels), and a window
that commits at any other size replaces it.

### Capture

There is no render pass and no GL read-back. The client's committed buffer is
copied straight into an owned `Vec<u8>` at commit time:

1. `map_rgba` (`crates/wayland/src/utils.rs`) copies the `wl_shm` buffer into
   an owned `Vec<u8>` at commit time (`crates/wayland/src/handlers/compositor.rs`),
   row by row (shm pools may be sparse, so the stride is never assumed to make
   rows contiguous),
2. `Window::capture()` (`crates/wayland/src/window.rs`) hands out the newest
   committed frame, taking it rather than copying it.

This is the **shm-first MVP**. A dmabuf/EGL client's buffer is not read at all,
so its window captures as black; no dma-buf code exists anywhere in the tree as
of 2026-11, and whether the GL path is still where it would go is unverified.
The output is `Argb8888`, whose bytes are B, G, R, A — the `Frame` is named
RGBA because that was the intent, and `gui` encodes it as `BGRX` to match what
is actually there.

### Input

We own the seat, so injection is direct:

- keyboard: synthesize `KeyPress/Release` keycodes (evdev codes + 8) through
  Smithay's `KeyboardHandle` input flow,
- pointer: `pointer_motion_absolute` + `pointer_button` + axis events through
  `PointerHandle`,
- scroll: pointer axis events, `AxisSource::Wheel`.

### Admission path

xdg_shell `get_xdg_toplevel` → `admit_into_root_client`: the first toplevel
fills the pane; later ones are pushed onto the root client's tree and reported
to `crates/compositor`, which places them. Subsurface commits are dropped.

### Threading

Smithay state (`State`) is owned by `WaylandServer` and advanced synchronously
by `step`, which the caller — the frame loop — drives. There is nothing to
send or wait on: capture is a read of the committed frame the commit path
already copied, and input is queued onto the window and dispatched on the next
`step`.

## Frame source integration

`crates/gui/src/windowing/wayland.rs` is that integration: it starts the
session, spawns the program, captures frames on the window's cadence, encodes
each as AV1 (the hardware codec; JPEG with `--jpeg`) and hands it to the
link, and routes this client's keyboard and pointer into the window.

Input reaches the program. Keys and pointer events are decoded from the
terminal's sequences in `gui` (`windowing/event_map.rs`) and queued onto the
window; pointer reporting is on by default, with `--dbg-no-mouse` to turn it
off. End-to-end pointer verification with a real terminal is unverified.
