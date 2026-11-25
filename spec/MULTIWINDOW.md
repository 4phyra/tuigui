# MULTIWINDOW — who holds a window, and who draws it

Status: decided and substantially built. The split below is the one in the code
(`crates/compositor`, `crates/wayland`, `crates/gui`): windows arrive from the
client, are placed in pane cells by the compositor, stack by rank, and reach the
terminal as rank-ordered placements. Popups and multiple toplevels are placed;
detach modes 2 and 3 are not built.

## The decision

The compositor **hands windows up**; it does not composite them into one picture.

- The compositor holds the **registry**: which surfaces exist, their kind (toplevel,
  popup), their parent, and their pixels. It originates nothing about layout.
- `gui` holds the **layout**: placement, z-order, hit-testing, focus. It is the
  authority on where a window is and who is on top, and it forwards input to the
  window it hit-tested.
- Detach is a **predicate over the registry**, not a second path: "this surface
  renders to its own window instead of into the composite."

## Why not flatten

Flatten means detach needs a per-window path beside the composite path. Both must
decide what a window *is* and where it *is*, over the same surfaces, and stay in
step. That is the permanent version of the coordinate-drift bug seen in the
first windowing code: a fact derived twice, reconciled by hand.

The split that makes hand-up clean is that registry and layout are *different
facts*, so one on each side is not duplication. Flatten-plus-detach duplicates
**layout**: the flatten layout and the detached layout are the same information
computed twice. Hand-up has one tracker, and detach is a filter over the list it
already has.

A second reason, for the AV1 path: a flattened frame re-encodes the whole scene on
every window move, and a move larger than the motion-search range leaves residual
artifacts in both the moved region and the region it revealed, visible until the
next intra-refresh. Separate streams make a move a re-placement with zero bits and
no artifacts.

## The one-way residual

The compositor still needs each surface's current placement, for two things it alone
can do:

- convert an incoming click (surface-local, sent by `gui`) into what the seat
  expects, and
- send a configure size to the client.

`gui` originates that placement and tells the compositor, the way a compositor tells
a client its configure. Single owner, one direction. There is no second derivation,
because the compositor never computes a placement: it receives the current one and
uses it.

## What was wrong when this was written, and what is true now

Fixed since this was written:

- a child surface copied into the window's pixel store at the origin — children
  are placed now: popups at their positioner's offset inside their parent
  (`crates/compositor/src/lib.rs`), extra toplevels admitted into the root
  client's tree and placed as siblings (`crates/wayland/src/handlers/xdg_shell.rs`,
  `admit_into_root_client`),
- a second `xdg_toplevel` rejected outright — admitted, and centred in the pane,
- a popup dismissed on sight — popups are configured, admitted, and placed.

Still true:

- a subsurface of a window is not placed at all: its buffer is dropped rather
  than composited (`crates/wayland/src/window.rs`, `commit_target`), so a window's
  picture is its own surface only. A subsurface that must become visible is an
  open problem, not a solved one.

## Wire

The registry is visible to `gui`, so the wire carries a **surface signature** in
both directions:

- up with each window: one `SurfaceId` (`OpenWindow` / `CloseWindow`, and on every
  `Frame` with the box in cells and a stacking rank); pixels stay opaque, so
  `easytgp` is unaffected — see `crates/wire/src/message.rs`,
- down with input: which window an event targets. The compositor lives in the
  same process as the routing (`crates/compositor` inside `gui`), so this does
  not cross the wire today; `CompositorRequest` on the wire is the seam for when
  a remote compositor needs it.

Image ids are not fixed and are not negotiated: each window's id is derived from
its `SurfaceId` by `tui::session::image_id` (`crates/tui/src/session.rs`), so two
windows never collide and both ends agree without a round trip. The rank that
orders placements rides on each `Frame` as `z`, shifted into the kitty
sub-text band by `stacking` in the same file.

## Detach

"Detach" names three different things, and they are not one feature:

1. a window rendered in the same pane, on its own — a renderer choice;
2. a window rendered in a second pane, its own session and image — needs the
   per-session ids above;
3. a window rendered as a native window on the viewer's desktop — a real window with
   real input events, not a placement at all.

Under flatten, (3) needs the second path this document rejects. Under hand-up it is
the predicate applied to one surface.

## Open

- Does a detached window leave a hole in the pane, or float over it?
- Native OS window events for a detached window, or terminal input routed through
  `gui` as for any other?
- Does `gui` poll the registry, or receive surface-appeared events?
- Where placement lives in cell space vs pixel space, and the one function that
  converts for both the draw and the hit-test path.
- Decoder topology on the AV1 path: one decoder per window in `tui`, or `gui`
  composites decoded frames and sends one stream.
