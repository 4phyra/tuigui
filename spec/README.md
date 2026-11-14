# README — the crate credits panel

Status: built. This is the shape of the "Built on excellent crates" block in
[README.md](../README.md), and the constraints that force it.

## What it looks like

One box per crate. A box is split by a vertical line: the logo fills the left
cell, the crate's name and its job sit in the right cell.

```
┌──┬───────────────────┐ ┌────┬──────────┐ ┌──┬─────────────────┐ ┌────┬────────────┐
│▣ │ smithay           │ │ ▣  │ tokio    │ │▣ │ ratatui         │ │ ▣  │ crossterm  │
│  │ the Wayland comp. │ │    │ async    │ │  │ widgets, layout │ │    │ terminal I/O│
└──┴───────────────────┘ └────┴──────────┘ └──┴─────────────────┘ └────┴────────────┘

┌──┬────────────────┐ ┌──┬───────────────┐ ┌────┬───────────────────┐
│▣ │ termina        │ │▣ │ vt100         │ │ ▣  │ keycode           │
│  │ input parsing  │ │  │ emulation     │ │    │ physical key codes│
└──┴────────────────┘ └──┴───────────────┘ └────┴───────────────────┘

┌────────────────────┬──────────────────┐ ┌──┬──────────────┐ ┌──┬──────────────┐
│      i r o h       │ iroh             │ │▣ │ rkyv         │ │▣ │ flume        │
│                    │ QUIC transport   │ │  │ message fmt  │ │  │ channels     │
└────────────────────┴──────────────────┘ └──┴──────────────┘ └──┴──────────────┘

┌──┬────────────────┐
│▣ │ rustix         │
│  │ system calls   │
└──┴────────────────┘
```

## The rules

- **Equal height, natural width.** Every logo is drawn at the same height
  (`height="26"`). Nothing sets a width, so each cell is as wide as that image's
  aspect ratio makes it: square marks get a narrow cell, iroh's wordmark a wide
  one, and the whole box with it. A box never forces its logo to letterbox.
- **The logo fills its cell.** The cell carries no width of its own, so the image
  is the cell's content size, not something shrunk inside it.
- **Boxes wrap as a unit.** The gap between boxes is `&emsp;&emsp;`, and the
  entries flow, so a narrow window gives three, two or one per row and never
  splits a box.
- **The name is bold, the job is `<sub>`.** Two lines in the text cell, top and
  bottom, vertically centred against the logo.

## Why it is tables and not flex or CSS

GitHub's `.markdown-body` stylesheet gives every `th`/`td` a
`1px solid var(--borderColor-default)` border and `6px 13px` of padding, with no
class or id hook, and it strips `style`, `<style>`, `class` and `id` from
rendered markdown. So:

- The vertical split line and the box outline are that forced cell border. They
  cannot be removed, and here they are the thing being asked for.
- `border="0"`, `cellspacing`, `cellpadding` and `frame="void"` either are
  stripped or lose to the stylesheet, so a borderless box is not reachable.
- The 13px horizontal padding inside each cell is likewise fixed.

The consequence worth knowing: the vertical rule is the shared edge between the
two cells, the outer box is the table's own border, and both come from GitHub
rather than from this repository.

## What not to do

- Do not set `width` on a logo. It equalises the cells and makes a wide wordmark
  letterbox, which is the failure this layout exists to avoid.
- Do not replace the per-entry table with a single outer table. One table means
  equal-width columns, and the whole point is that each box is sized by its own
  logo and text.
- Do not add a spacer cell for the gap. Empty cells draw their borders, so the
  gap becomes a visible empty box.
