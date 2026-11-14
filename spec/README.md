# README — the crate credits panel

Status: built. This is the shape of the "Built on excellent crates" block in
[README.md](../README.md), and the constraints that force it.

## What it looks like

One item per crate, two rows:

- **r0** — a large logo on the left, the crate's name in bold beside it.
- **r1** — the description, on one line, under the name.

No borders, no boxes, no dividers. Everything sits bottom-left.

```
   ████████                 ▣                 ▣
   ████████   smithay       ▣      tokio      ▣      ratatui

   the Wayland compositor   async runtime     widgets and layout
```

## The rules

- **The logo is big.** It is the thing worth looking at, so it is drawn large
  (not a 16px icon) and gets the room that implies.
- **The logo keeps its own aspect ratio.** No width is set, so a wide wordmark
  such as iroh's makes a wider item than a square mark does.
- **The description never wraps.** It is one line, and it starts under the name,
  not under the logo.
- **Items flow and wrap as a unit**, so a narrow window gives two or one across
  and never splits an item.
- **No borders.** The boxes in early sketches were only there to mark the cells;
  they are not part of the design.

## Can this be built?

Partly. The layout itself is reachable; the borderless two-row cell structure is
not, because of how GitHub renders markdown.

Reachable:

- a large inline logo with a bold name beside it, then the description under it —
  all inside one `<p>`, no table needed;
- logos at their natural aspect ratio at a shared height;
- items flowing side by side and wrapping.

Not reachable on GitHub:

- **A true two-row cell.** Aligning the description under the *name* while the
  logo spans both rows needs a table with `colspan`/`rowspan`, and GitHub gives
  every `td` a forced `1px solid` border and `6px 13px` padding that no markup
  can remove (`style`, `<style>`, `class` and `id` are stripped; `border="0"`,
  `cellspacing` and `cellpadding` are stripped or lose to the stylesheet).
- **A description that starts exactly under the name** without those cells. With
  inline flow the description starts wherever the line lies, so on a wrapped
  item it begins under the logo instead.

So the honest build is: big logo, bold name beside it, description on the next
line — visually the same as the sketch, without the guaranteed left alignment of
the description against the name on every wrapped item.
