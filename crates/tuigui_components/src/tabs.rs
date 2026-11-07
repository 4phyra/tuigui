//! A row of tabs, one of them active.
//!
//! # The move, frame by frame
//!
//! A click sends the band from the active tab to the tab that was pressed. The move is a
//! handful of stop frames, drawn one at a time, and it is three things in order: it walks a
//! couple of cells per frame, it goes past the tab for three frames, and it settles.
//!
//! The strip is `one two six`, and the click is on `six`. `·` is blank and `█` is a cell of
//! band; `▕ ▐ ▌ ▉` are the part-cells of an edge, drawn with the band as their foreground and
//! no background of their own, so the rest of the cell stays whatever the row already was.
//!
//! ```text
//! ·█████···········  rest on `one`
//!  one  two  six
//!
//! ···█████·········  walk
//!  one  two  six
//!
//! ·····█████·······  walk
//!  one  two  six
//!
//! ·······█████·····  walk
//!  one  two  six
//!
//! ·········█████···  walk
//!  one  two  six
//!
//! ···········█████·  arrived on `six`
//!  one  two  six
//!
//! ···········▐████▌  overshoot: half a cell past
//!  one  two  six
//!
//! ···········▐████▌  held, five frames in all
//!  one  two  six
//!
//! ···········█████·  settled, exactly on `six`
//!  one  two  six
//! ```
//!
//! # Why it is shaped this way
//!
//! - **The walk is whole cells.** A band edge that sat mid-cell over a name would have to be
//!   drawn with a part-block in place of the letter. Whole cells keep every name readable, so
//!   the last frame of the walk is the tab itself: the band arrives, then bounces.
//! - **The overshoot is fractional, and only where there is room.** The band grows past its tab
//!   on both sides, into the blank cell that [`PADDING`] leaves beside every name and the
//!   [`GUTTER`] beside the strip. No letter is ever under a part-block.
//! - **A right-hand fraction is drawn by filling and cutting.** Unicode's right-aligned blocks
//!   exist only at an eighth and a half, but any fraction is drawable: fill the cell with the
//!   band and paint a left block of the complementary size in the row's own background over it.
//!   That works for every eighth, so the bounce can use the fractions it wants. `Color::Reset` is
//!   not that background — it is the terminal's default *text* colour, which is why an attempted
//!   complement showed as a white cell. The row's background is read from the row itself.
//! - **The bounce decays.** The band goes to the largest overshoot and then to fractions nearer
//!   the tab, one frame each, so the arrival is a settle rather than a snap.
//! - **A part-cell is a glyph, not a background.** The band colours the glyph and leaves the
//!   cell's background alone. Painting the background would fill the half of the cell the band
//!   does not cover, which is what makes an overshoot look like a tab growing instead of moving.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::StatefulWidget;
use termina::event::{MouseButton, MouseEventKind};

use crate::events::Events;

/// How many columns of padding sit on each side of a tab's name.
///
/// Room for the band's edge to sit beside a name without covering a letter.
const PADDING: u16 = 1;

/// How many columns sit between two tabs.
///
/// A gap rather than more padding: the tab's own cells are what the pointer presses, and a
/// spacer that belonged to one of them would be pressed along with it.
const GAP: u16 = 1;

/// The background a tab takes while the pointer is on it.
const HOVER: Color = Color::Rgb(0x3a, 0x3a, 0x3a);

/// How many columns the strip keeps free at each end: the room an end tab bounces into.
const GUTTER: u16 = 1;

/// How many stop frames the band walks for. The last of them is on the tab.
const WALK: u8 = 5;

/// The plate the strip sits on: the colour every cell of its row is filled with first.
///
/// A partial cell has to name both of its colours — a block is filled with the foreground and the
/// rest of the cell shows the background — and the part the band does not cover is this. Naming it
/// rather than leaving it alone is what makes a fraction drawable in either direction: the
/// uncovered part is filled with the plate, so no cell depends on a colour the widget cannot see.
/// `Color::Reset` would be the tempting choice and is the wrong one: as a foreground it is the
/// terminal's default *text* colour, which is a white patch.
const PLATE: Color = Color::Rgb(0x1e, 0x1e, 0x1e);

/// The fractions past its tab the band passes through, one frame each.
///
/// The largest first, then nearer and nearer the tab: the move arrives with too much speed and
/// gives it back in steps, so the arrival is a settle rather than a snap.
const BOUNCE: [f32; 5] = [0.625, 0.625, 0.5, 0.25, 0.125];

/// How many stop frames a move draws in total.
const FRAMES: u8 = WALK + BOUNCE.len() as u8 + 1;

/// The left-aligned blocks, one per eighth of a cell: what a band's right edge is drawn with.
const EIGHTHS: [char; 8] = ['▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];

/// A row of tabs, one of them active.
///
/// Each tab carries the colour it is drawn in, so the strip is the caller's palette rather than
/// this crate's. The active tab is a band of its colour with the name cut out of it, and a click
/// on another tab walks the band there, overshooting it before it settles.
pub struct Tabs {
    tabs: Vec<(String, Color)>,
    active: usize,
    /// The tab the band is coming from. Equal to `active` at rest.
    from: usize,
    /// How many of a move's frames have been drawn. [`FRAMES`] is at rest, held there by
    /// `from == active`.
    shown: u8,
}

impl Tabs {
    pub fn new(tabs: Vec<(String, Color)>, active: usize) -> Self {
        Self {
            tabs,
            active,
            from: active,
            shown: FRAMES,
        }
    }

    /// The index of the active tab.
    pub fn active(&self) -> usize {
        self.active
    }

    /// Whether the band is between two tabs, which is when a caller has to keep drawing.
    pub fn is_moving(&self) -> bool {
        self.shown < FRAMES
    }

    /// Advance the move by one stop frame. A no-op at rest.
    ///
    /// Drawing does not do this. A frame is shown for as long as the caller says, and only the
    /// caller knows when that is: a draw that advanced the move itself would move on every
    /// redraw, and a redraw happens for reasons that are not time — a key, a pointer motion, a
    /// pane that changed size — so the animation would skip frames whenever the mouse moved.
    pub fn step(&mut self) {
        if self.shown < FRAMES {
            self.shown += 1;
        }
    }

    /// The columns the whole strip occupies, gutters and gaps included.
    pub fn width(&self) -> u16 {
        self.tabs
            .iter()
            .fold(GUTTER * 2, |total, (name, _)| {
                total.saturating_add(span(name).saturating_add(GAP))
            })
            // The last tab has no gap after it, and the fold counted one.
            .saturating_sub(match self.tabs.is_empty() {
                true => 0,
                false => GAP,
            })
    }

    /// The columns each tab occupies, measured from the first tab's own left edge.
    ///
    /// One answer for the draw, the hit test and the band, so a tab is pressed where it is drawn.
    /// A tab's box is its own cells: the gap after it belongs to the row, not to either neighbour.
    fn offsets(&self) -> Vec<(u16, u16)> {
        let mut start = 0u16;
        self.tabs
            .iter()
            .map(|(name, _)| {
                let end = start.saturating_add(span(name));
                let range = (start, end);
                start = end.saturating_add(GAP);
                range
            })
            .collect()
    }

    /// The band's two edges for the frame being drawn, in columns from the first tab's edge.
    ///
    /// `None` for a strip with nothing on it.
    fn edges(&self) -> Option<(f32, f32)> {
        let offsets = self.offsets();
        let (from_start, from_end) = *offsets.get(self.from)?;
        let (to_start, to_end) = *offsets.get(self.active)?;
        let (from_start, from_end) = (f32::from(from_start), f32::from(from_end));
        let (to_start, to_end) = (f32::from(to_start), f32::from(to_end));

        if self.from == self.active || self.shown >= FRAMES {
            return Some((to_start, to_end));
        }

        if self.shown < WALK {
            // A whole-cell step, and the last of them is the tab: `(shown + 1) / WALK` reaches
            // one, so the walk lands rather than stopping short of the bounce.
            let step = f32::from(self.shown + 1) / f32::from(WALK);
            return Some((
                mix(from_start, to_start, step).round(),
                mix(from_end, to_end, step).round(),
            ));
        }

        // The bounce: past the tab and back onto it.
        let past = BOUNCE
            .get(usize::from(self.shown - WALK))
            .copied()
            .unwrap_or(0.0)
            * if to_start > from_start { 1.0 } else { -1.0 };
        Some((to_start + past, to_end + past))
    }

    /// The colour the band is drawn in, crossing from the tab it left to the one it reached.
    fn colour(&self) -> Color {
        let (Some((_, from)), Some((_, to))) = (self.tabs.get(self.from), self.tabs.get(self.active))
        else {
            return Color::Reset;
        };
        let step = f32::from(self.shown.saturating_add(1)) / f32::from(FRAMES);
        mix_colour(*from, *to, step.min(1.0))
    }
}

/// The columns one tab occupies: its name padded on both sides. The gap between tabs is
/// [`GAP`] and is added by whoever lays them out, not here.
fn span(name: &str) -> u16 {
    let width = u16::try_from(Line::raw(name).width()).unwrap_or(u16::MAX);
    width.saturating_add(PADDING * 2)
}

/// The point `factor` of the way from `from` to `to`.
fn mix(from: f32, to: f32, factor: f32) -> f32 {
    from + (to - from) * factor
}

/// The colour `factor` of the way from one to another.
///
/// Only truecolour can be crossed: a named or indexed colour has no place between it and
/// another, so the change is made half way rather than invented.
fn mix_colour(from: Color, to: Color, factor: f32) -> Color {
    let (Color::Rgb(fr, fg, fb), Color::Rgb(tr, tg, tb)) = (from, to) else {
        return if factor < 0.5 { from } else { to };
    };
    let channel = |from: u8, to: u8| {
        mix(f32::from(from), f32::from(to), factor)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Color::Rgb(channel(fr, tr), channel(fg, tg), channel(fb, tb))
}

/// How much of cell `column` the band covers, and from which side.
///
/// `column` is measured from the first tab's own left edge and may be negative: the gutter is a
/// column too, and an end tab's overshoot reaches into it.
///
/// `None` when the band covers none of it, `Some(true)` for its left-hand part and
/// `Some(false)` for its right-hand part — which is which side has a block to draw it with.
fn part((left, right): (f32, f32), column: i32) -> Option<(f32, bool)> {
    let start = f32::from(i16::try_from(column).unwrap_or(i16::MAX));
    let covered = (right.min(start + 1.0) - left.max(start)).clamp(0.0, 1.0);
    (covered > 0.0).then_some((covered, left <= start))
}

impl StatefulWidget for &mut Tabs {
    type State = Events;

    fn render(self, area: Rect, buf: &mut Buffer, events: &mut Self::State) {
        // The tab space starts one gutter in, which is the room the band bounces into.
        let left = area.x.saturating_add(GUTTER);
        let offsets = self.offsets();
        let areas: Vec<Rect> = offsets
            .iter()
            .map(|(start, end)| {
                Rect::new(
                    left.saturating_add(*start),
                    area.y,
                    end.saturating_sub(*start),
                    area.height,
                )
            })
            .collect();

        // A click on another tab starts a move, and is taken so that whatever is drawn after the
        // strip does not act on the same press. Only a press on a tab is taken: a click past the
        // last name belongs to the row, not to the strip.
        let pressed = events
            .mouse_hits
            .filter(|mouse| matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)))
            .and_then(|mouse| {
                let at = Rect::new(mouse.column, mouse.row, 1, 1);
                areas.iter().position(|tab| tab.intersects(at))
            });
        if let Some(index) = pressed {
            events.take_press(area);
            if index != self.active {
                self.from = self.active;
                self.active = index;
                self.shown = 0;
            }
        }

        // The strip's row is filled with the plate, gutters included. This is both the clear —
        // nothing of the last frame survives where the band is no longer — and the colour a
        // partial cell shows where the band does not cover it.
        let under = PLATE;
        for x in area.left()..area.left().saturating_add(self.width()) {
            if let Some(cell) = buf.cell_mut((x, area.y)) {
                cell.set_char(' ').set_style(Style::new().bg(under));
            }
        }
        // This frame's position. Drawing is a read of where the band is, not a step of it:
        // `step` is the caller's, so a redraw caused by a key or a pointer motion shows the same
        // frame again rather than moving the band on.
        let edges = self.edges();
        let colour = self.colour();
        let Some(edges) = edges else {
            return;
        };

        // The columns run from the near gutter to the far one, because an end tab overshoots into
        // whichever gutter is behind it: the first tab bounces left of column zero, and the last
        // one bounces past the last name.
        for column in -(GUTTER as i32)..self.width() as i32 {
            let Some((covered, at_start)) = part(edges, column) else {
                continue;
            };
            let Some(cell) = buf
                .cell_mut((left.saturating_add_signed(i16::try_from(column).unwrap_or(0)), area.y))
            else {
                continue;
            };
            if covered >= 1.0 {
                // A whole cell: only the background changes, so the name keeps its character.
                cell.set_style(Style::new().bg(colour));
                continue;
            }
            if at_start {
                // The band's right edge is inside this cell: its left part is covered, and there
                // is a left-aligned block for every eighth. The block is the band over the plate.
                let eighths = ((covered * 8.0).round() as usize).clamp(1, 7);
                cell.set_char(EIGHTHS[eighths - 1])
                    .set_fg(colour)
                    .set_bg(under);
            } else {
                // The band's left edge is inside this cell: its right part is covered, and there
                // is no right-aligned block for most eighths. So the left part is drawn instead,
                // with the block of the complementary size: the band fills the cell's background
                // and the plate paints the part the band does not cover.
                let eighths = ((covered * 8.0).round() as usize).clamp(1, 7);
                cell.set_char(EIGHTHS[8 - eighths - 1])
                    .set_fg(under)
                    .set_bg(colour);
            }
        }

        for ((name, tab_colour), tab) in self.tabs.iter().zip(areas.iter()) {
            let hovered = events.hovering(*tab);
            // A hover changes the tab's background rather than its text: it is the cell the
            // pointer is on that has to answer, and an underline answers for one row of it.
            if hovered && self.tabs.get(self.active).map(|(_, colour)| *colour) != Some(*tab_colour) {
                for x in tab.left()..tab.right() {
                    if let Some(cell) = buf.cell_mut((x, area.y)) {
                        cell.set_style(Style::new().bg(HOVER));
                    }
                }
            }
            let mut x = tab.x.saturating_add(PADDING);
            for grapheme in Line::raw(name.as_str()).styled_graphemes(Style::new()) {
                let column = i32::from(x.saturating_sub(left));
                let covered = part(edges, column).map_or(0.0, |(cov, _)| cov);
                let style = if covered > 0.0 {
                    // Over the band the name is cut out of it, and cut out by as much as the band
                    // covers it: a letter the band is half over is half way to the cut-out colour.
                    Style::new().fg(mix_colour(*tab_colour, Color::Black, covered))
                } else if hovered {
                    Style::new().fg(*tab_colour).bg(HOVER)
                } else {
                    Style::new().fg(*tab_colour)
                };
                let (next, _) = buf.set_stringn(x, area.y, grapheme.symbol, 1, style);
                x = next;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area() -> Rect {
        Rect::new(0, 0, 20, 1)
    }

    fn tabs(active: usize) -> Tabs {
        Tabs::new(
            vec![
                ("one".to_string(), Color::Rgb(255, 0, 0)),
                ("two".to_string(), Color::Rgb(0, 0, 255)),
            ],
            active,
        )
    }

    fn draw(tabs: &mut Tabs, events: &mut Events) -> Buffer {
        let mut buf = Buffer::empty(area());
        StatefulWidget::render(tabs, area(), &mut buf, events);
        buf
    }

    fn click(column: u16) -> Events {
        Events {
            keypresses: Vec::new(),
            mouse_hits: Some(termina::event::MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column,
                row: 0,
                modifiers: termina::event::Modifiers::NONE,
            }),
        }
    }

    #[test]
    fn the_band_rests_on_the_active_tab() {
        let buf = draw(&mut tabs(1), &mut Events::default());
        // One gutter, then `one` at 1..6, a gap at 6, and `two` at 7..12 whose name starts at 8.
        assert_eq!(buf[(7, 0)].bg, Color::Rgb(0, 0, 255));
        assert_eq!(buf[(2, 0)].fg, Color::Rgb(255, 0, 0));
        // The gap between the two is plate to the edges of their boxes, nothing more.
        assert_eq!(buf[(6, 0)].bg, PLATE);
    }

    #[test]
    fn the_walk_lands_on_the_tab() {
        let mut tabs = tabs(0);
        draw(&mut tabs, &mut click(8));
        while tabs.shown < WALK - 1 {
            draw(&mut tabs, &mut Events::default());
        }
        // The last frame of the walk is the tab itself, not a step short of it.
        assert_eq!(tabs.edges(), Some((6.0, 11.0)));
    }

    #[test]
    fn the_walk_moves_whole_cells_only() {
        let mut tabs = tabs(0);
        draw(&mut tabs, &mut click(8));
        for _ in 1..WALK {
            let (left, right) = tabs.edges().expect("a strip with tabs has edges");
            assert_eq!(left, left.round(), "a fractional walk edge: {left}");
            assert_eq!(right, right.round(), "a fractional walk edge: {right}");
            draw(&mut tabs, &mut Events::default());
        }
    }

    #[test]
    fn the_bounce_decays_to_the_tab() {
        let mut tabs = tabs(0);
        draw(&mut tabs, &mut click(8));
        for _ in 1..WALK {
            draw(&mut tabs, &mut Events::default());
        }
        for expected in BOUNCE {
            let (left, right) = tabs.edges().expect("a strip with tabs has edges");
            assert_eq!(left, 6.0 + expected, "left edge of the bounce");
            assert_eq!(right, 11.0 + expected, "right edge of the bounce");
            draw(&mut tabs, &mut Events::default());
        }
        // And then it is exactly on the tab.
        assert_eq!(tabs.edges(), Some((6.0, 11.0)));
    }

    #[test]
    fn every_bounce_fraction_lands_on_an_eighth() {
        // Each edge covers some number of eighths, and both directions of edge have a block:
        // a left-aligned one from `EIGHTHS`, and a right-hand one cut out of the band.
        for past in BOUNCE {
            let eighths = (past * 8.0).round() as usize;
            assert!((1..=7).contains(&eighths), "{past} is not an eighth of a cell");
        }
    }

    #[test]
    fn a_move_ends_after_every_frame() {
        let mut tabs = tabs(0);
        draw(&mut tabs, &mut click(8));
        for _ in 1..FRAMES {
            assert!(tabs.is_moving());
            draw(&mut tabs, &mut Events::default());
        }
        assert!(!tabs.is_moving());
    }

    #[test]
    fn a_cell_the_band_left_goes_back_to_the_plate() {
        let mut tabs = tabs(0);
        let mut buf = draw(&mut tabs, &mut Events::default());
        assert_ne!(buf[(1, 0)].bg, PLATE);
        draw(&mut tabs, &mut click(8));
        for _ in 1..FRAMES {
            buf = draw(&mut tabs, &mut Events::default());
        }
        assert_eq!(buf[(1, 0)].bg, PLATE);
    }

    #[test]
    fn a_hovered_tab_takes_the_hover_background() {
        let mut buf = Buffer::empty(area());
        StatefulWidget::render(
            &mut tabs(0),
            area(),
            &mut buf,
            &mut Events {
                keypresses: Vec::new(),
                mouse_hits: Some(termina::event::MouseEvent {
                    kind: MouseEventKind::Moved,
                    column: 8,
                    row: 0,
                    modifiers: termina::event::Modifiers::NONE,
                }),
            },
        );
        // `two` is not active, so it answers with its background rather than with the band.
        assert_eq!(buf[(8, 0)].bg, HOVER);
        assert_eq!(buf[(3, 0)].bg, PLATE);
    }

    #[test]
    fn a_click_past_the_last_tab_is_left_alone() {
        let mut tabs = tabs(0);
        let mut events = click(18);
        draw(&mut tabs, &mut events);
        assert_eq!(tabs.active(), 0);
        assert!(events.mouse_hits.is_some());
    }

    #[test]
    fn the_colour_crosses_rather_than_switches() {
        let mut tabs = tabs(0);
        draw(&mut tabs, &mut click(8));
        draw(&mut tabs, &mut Events::default());
        let halfway = tabs.colour();
        let Color::Rgb(red, _, blue) = halfway else {
            panic!("truecolour stays truecolour: {halfway:?}");
        };
        assert!(red > 0 && blue > 0, "{red} {blue}");
    }

    #[test]
    fn a_part_cell_reports_which_side_is_covered() {
        // The band starts inside cell 2, so what is covered there is its right-hand part.
        assert_eq!(part((2.5, 3.5), 2), Some((0.5, false)));
        // It ends inside cell 3, so there its left-hand part is covered.
        assert_eq!(part((2.5, 3.5), 3), Some((0.5, true)));
        assert_eq!(part((2.5, 3.5), 4), None);
        assert_eq!(part((2.0, 4.0), 3), Some((1.0, true)));
    }

    #[test]
    fn the_strip_is_its_names_plus_a_gutter_and_the_gaps() {
        // Two five-column tabs, one gap between them, one gutter at each end.
        assert_eq!(tabs(0).width(), 13);
    }

    #[test]
    fn dump() {
        for (name, target) in [("right", 2usize), ("left", 0usize)] {
            let mut tabs = Tabs::new(
                vec![
                    ("one".to_string(), Color::Rgb(255, 0, 0)),
                    ("two".to_string(), Color::Rgb(0, 0, 255)),
                    ("six".to_string(), Color::Rgb(0, 255, 0)),
                ],
                match name {
                    "right" => 0,
                    _ => 2,
                },
            );
            let mut buf = Buffer::empty(area());
            StatefulWidget::render(&mut tabs, area(), &mut buf, &mut Events::default());
            let row = |buf: &Buffer| {
                (0..20)
                    .map(|x| buf[(x, 0)].symbol().to_string())
                    .collect::<String>()
            };
            println!("--- moving {name}: rest edges {:?}", tabs.edges());
            println!("{}", row(&buf));

            // Click the target tab's own column.
            let columns = [2, 7, 12];
            let mut events = click(columns[target]);
            for _ in 0..FRAMES {
                StatefulWidget::render(&mut tabs, area(), &mut buf, &mut events);
                events = Events::default();
                let drawn = tabs.shown.saturating_sub(1);
                tabs.shown = drawn;
                let edges = tabs.edges();
                tabs.shown = drawn + 1;
                println!("f{drawn:<3} edges {edges:?}");
                println!("{}", row(&buf));
            }
        }
    }
}
