//! The dock: the three rows that get a session started and say what it is doing.
//!
//! It owns the stages before there is a program: nothing chosen yet, bound and waiting to
//! be dialed, and dialed. Once a program is running, the dock is furniture again: a line
//! saying what the session is, and — one day — buttons that switch screens.
//!
//! Nothing here reads the terminal. A dock is drawn with the stage as its state, which is
//! what lets it act on the session rather than merely describe it.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, StatefulWidget, Widget};

use super::{Page, Stage};

/// How many rows the dock occupies: a border, the row its content sits on, and a border.
pub const HEIGHT: u16 = 3;

/// The dock as the pane draws it: the same three rows in every state.
///
/// No `Default`, and no state before [`DockState::Waiting`]: a session starts dialing
/// rather than sitting idle, so there is nothing for a default to be.
pub struct Dock {
    /// Which stage of being reachable the dock is in. Its own, rather than passed about: a
    /// dock that did not remember being dialed would dial again every frame.
    pub state: DockState,
    /// The program this pane was started for, as the command line gave it.
    ///
    /// Shown in the row once there is a link: what the pane is running is the one thing worth
    /// saying there, and it is the only text the dock has of its own.
    pub program: String,
    /// The screen switch, at the right of the row.
    ///
    /// A tab strip rather than two hand-drawn labels: the band is what says which screen is up,
    /// and it moves when the other is pressed.
    tabs: tuigui_components::Tabs,
    /// The way out, to the right of the switch.
    ///
    /// A cross rather than a key, because leaving is what a user reaches for with the pointer, and
    /// it belongs beside the tabs that got them here.
    cross: tuigui_components::Cross,
}

/// The stages before there is a program: bound and waiting to be dialed, and dialed.
///
/// Both belong to the dock rather than to the session because both are about being
/// reachable, and the dock is the thing that is reachable: it binds the socket, it holds
/// the advert the other side scans for, and it is what waits to be dialed.
pub enum DockState {
    /// Bound and announced, waiting for the other side to dial.
    Pairing {
        /// The announcement as cells: the transport's own `to_magic`, cut into braille and held
        /// for the life of the socket.
        ///
        /// Held rather than encoded per frame because a record is a fixed thing, and drawing it
        /// fresh each frame would make a standing announcement flicker — the one thing a mark on a
        /// screen must not do.
        barcode: Vec<cellcode::Cell>,
    },
    /// The program is running and its window is being streamed.
    Streaming,
}

impl Dock {
    /// Announce a transport that is already reachable, so the other side can dial it.
    ///
    /// The address is the transport's own: it is asked for its announcement, and the
    /// announcement is what gets drawn. Nothing in this crate formats it, which is what keeps
    /// the text on screen and the text the far side scans for the same text.
    ///
    /// Bound by the caller, because binding is the one step the two channels do not share:
    /// a socket has a path to listen on and an endpoint has to be minted. What is common is
    /// everything this does — announce it, draw it, wait to be dialed.
    ///
    /// The announcement of an address that is already listening.
    ///
    /// Only the drawing is here. Waiting for the dial is the run path's, because a dial is what
    /// starts the program and the program is not this half's: what the dock says is where to reach
    /// this pane, and the transport itself has already gone to the side that waits on it.
    pub fn new(advert: String, program: String) -> std::io::Result<Self> {
        log::info!("announcing {advert}");
        // The record as cells, padded to a fixed length: blank braille in front, so the row is the
        // same every frame and two panes can be compared by eye.
        let mut barcode = Vec::new();
        cellcode::encode(&advert, |cell| barcode.push(cell))
            .map_err(|e| std::io::Error::other(format!("the announcement does not fit: {e}")))?;
        Ok(Self {
            state: DockState::Pairing { barcode },
            program,
            tabs: tuigui_components::Tabs::new(
                vec![
                    ("Logs".to_string(), None, tuigui_components::CORAL),
                    (
                        "Windows".to_string(),
                        Some(termina::event::KeyCode::Char('w')),
                        tuigui_components::VIOLET,
                    ),
                ],
                1,
            ),
            cross: tuigui_components::Cross::new(),
        })
    }
}

/// What the dock is handed for one frame: the session, and how long since the last one.
pub struct DockProps<'a> {
    /// The session, so the dock can act on it rather than describe it.
    pub stage: &'a mut Result<Stage, String>,
    /// Seconds since the previous frame, which is what paces everything animated.
    pub delta: f32,
}

impl<'a> StatefulWidget for &'a mut Dock {
    type State = DockProps<'a>;

    fn render(self, area: Rect, buf: &mut Buffer, props: &mut Self::State) {
        // The row inside the border, which is the whole of the dock's content: the box is
        // three rows because a box is what makes it read as furniture under a screen.
        let inner = Rect {
            x: area.x + 1,
            y: area.y + 1,
            width: area.width.saturating_sub(2),
            height: 1,
        };

        // The cross sits one cell in from the row's right edge, with the strip's own trailing gap
        // the only thing between them.
        let width = self.tabs.width().min(inner.width);
        let close = Rect::new(
            inner
                .right()
                .saturating_sub(tuigui_components::cross::WIDTH)
                .saturating_sub(1),
            inner.y,
            tuigui_components::cross::WIDTH,
            1,
        );
        // The strip's own width includes the gap after its last tab, so the cross starts where
        // that gap ends: the two are neighbours with nothing between them.
        let tabs = Rect::new(close.x.saturating_sub(width), inner.y, width, 1);

        // The strip moves its band and reports which tab it was; what the page becomes is the
        // session's to say. A press the session cannot honour — Windows before there is a link to
        // carry it — leaves the page where it is, so the band is put back on the page that is
        // really showing. The page is the truth here; the strip only draws it.
        if let Ok(stage) = props.stage {
            self.tabs.set_active(stage.get_page().tab());
        }
        Block::bordered()
            .border_type(ratatui::widgets::BorderType::Rounded)
            .border_style(Style::new().fg(Color::DarkGray))
            .render(area, buf);

        Widget::render(&self.tabs, tabs, buf);
        Widget::render(&self.cross, close, buf);
        // The strip is advanced by the frame's own length, so the move plays at the same speed
        // whatever rate this pane happens to draw at.
        self.tabs.step(props.delta);

        // Everything left of the buttons is the state's to draw, so a long line is cut off
        // rather than drawn over them.
        let row = Rect {
            width: tabs.x.saturating_sub(inner.x),
            ..inner
        };

        match &self.state {
            DockState::Pairing { barcode, .. } => {
                render_waiting(inner, row, barcode, buf, self.status(props.stage))
            }
            DockState::Streaming => render_program(row, buf, self.status(props.stage)),
        }

        // The light, always: on the dock's own top border, because what it says — this pane is
        // waiting, paired, or streaming — is true in every state. Only the word beside it and the
        // colour it breathes in change, so the mark itself never appears or disappears.
        let (said, colour) = match self.state {
            DockState::Pairing { .. } => ("pairing", (0xFF, 0x69, 0xB4)),
            DockState::Streaming => ("streaming", (0xFF, 0x7F, 0x50)),
        };
        light(area, buf, said, colour);
    }
}

impl Dock {
    /// Poll the wait once. A pending wait is the normal answer for every frame until the other
    /// side dials.
    /// What the row says: the program this pane was started for, once there is a link.
    ///
    /// A failure says what went wrong rather than what step it was in, because the failure is the
    /// more useful fact. While waiting there is nothing to say: the record is what the row carries,
    /// and it is drawn by [`render_waiting`].
    /// Whether something here is still moving, so the next frame is already owed.
    ///
    /// A tab strip walks its band over a few frames, and a frame nobody asked for is a band frozen
    /// halfway. Asking here rather than keeping a flag beside it keeps the answer where the motion
    /// is: the strip is the only thing in the dock that animates.
    pub fn is_animating(&self) -> bool {
        self.tabs.is_moving()
    }

    /// The program's words while it was watched, and the screen's own state, are the stage's.
    fn status<'a>(&'a self, stage: &'a Result<Stage, String>) -> &'a str {
        match stage {
            Err(message) => message,
            // The command is what this pane is for, so it is what the row says once the other side
            // has been reached — before that the row is the record's.
            Ok(_) => match self.state {
                DockState::Pairing { .. } => "",
                DockState::Streaming => &self.program,
            },
        }
    }

    /// Offer one event to the dock's furniture, and answer whether a frame is owed.
    ///
    /// The strip and the cross are the two things here that take input, and both answer through
    /// their own state machines: what the dock does is act on what they report. A press on the cross
    /// ends the session; a press on a tab asks for the other screen, and whether the session moves is
    /// the session's to say.
    pub fn handle_event(
        &mut self,
        event: &termina::Event,
        area: Rect,
        stage: &mut Result<Stage, String>,
    ) -> bool {
        // The geometry is the row inside the border, which is what the frame draws the furniture in.
        let inner = Rect {
            x: area.x + 1,
            y: area.y + 1,
            width: area.width.saturating_sub(2),
            height: 1,
        };
        let width = self.tabs.width().min(inner.width);
        let close = Rect::new(
            inner
                .right()
                .saturating_sub(tuigui_components::cross::WIDTH)
                .saturating_sub(1),
            inner.y,
            tuigui_components::cross::WIDTH,
            1,
        );
        let tabs = Rect::new(close.x.saturating_sub(width), inner.y, width, 1);

        let mut events = vec![event.clone()];
        self.tabs.handle_events(tabs, &mut events);
        self.cross.handle_events(close, &mut events);

        if self.cross.was_released() {
            // The session is always there: a dock only exists inside one.
            if let Ok(stage) = stage {
                stage.from_any_to_exiting();
            }
            return true;
        }

        // The strip moves its band and reports which tab it was; what the page becomes is the
        // session's to say. A press the session cannot honour — Windows before there is a link to
        // carry it — leaves the page where it is, so the band is put back on the page that is really
        // showing. The page is the truth here; the strip only draws it.
        let mut changed = self.tabs.is_moving();
        if let Ok(stage) = stage {
            if let Some(chosen) = self.tabs.chosen() {
                if chosen != stage.get_page().tab() {
                    match stage.get_page() {
                        Page::Stream { .. } => drop(stage.from_streaming_to_logs()),
                        Page::Logs { .. } | Page::Init => stage.from_logs_to_streaming(),
                        // Already over: the frame that asked is the one this was drawn on, and
                        // there is no screen after it to move to.
                        Page::Exiting { .. } => {
                            unreachable!("a press was read after the session ended")
                        }
                    }
                }
            }
            self.tabs.set_active(stage.get_page().tab());
        }
        changed |= self.tabs.is_moving();
        changed
    }
}

/// The dock waiting to be dialed: the barcode, and whatever the state has to say.
fn render_waiting(
    inner: Rect,
    row: Rect,
    barcode: &[cellcode::Cell],
    buf: &mut Buffer,
    status: &str,
) {
    // The barcode, as it was decided when the socket was bound: the same cells every frame,
    // because an announcement is a standing mark rather than a picture.
    let drawn = (ratatui::text::Line::from(status).width() as u16).min(row.width);
    ratatui::widgets::Paragraph::new(status).render(row, buf);

    let mut x = row.x + drawn + 1;
    for cell in barcode {
        if let Some(target) = buf.cell_mut((x, inner.y)) {
            target
                .set_char(cell.symbol)
                .set_fg(Color::Rgb(cell.fg.0, cell.fg.1, cell.fg.2));
        }
        x += 1;
    }
}

/// The dot that says what this pane is doing, and the word for it.
///
/// Drawn on the dock's top border, so it is the same place in every state: the mark is
/// furniture, and only what it says changes. Its brightness comes from the clock alone — a
/// sine with a two-second cycle, never fully dark and never at the full colour — because a
/// stored brightness would need a caller to advance it, and every frame already redraws.
fn light(area: Rect, buf: &mut Buffer, said: &str, rgb: (u8, u8, u8)) {
    /// The dot that breathes.
    const DOT: char = '•';

    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_millis());
    let level = match elapsed {
        Ok(millis) => {
            let wave = (millis as f64 / 1000.0 * std::f64::consts::PI).sin() * 0.5 + 0.5;
            // Between a fifth and the full colour: enough of a range to read as a fade, and
            // not so much that it ever looks unlit.
            (0.2 + wave * 0.8) as f32
        }
        Err(_) => 1.0,
    };
    let colour = Color::Rgb(
        (rgb.0 as f32 * level) as u8,
        (rgb.1 as f32 * level) as u8,
        (rgb.2 as f32 * level) as u8,
    );

    // On the border, a cell in from the corner: the word is drawn after the border, so it
    // lands in cells the border already put a line in, and they are blanked first.
    let start = area.x + 2;
    let to = start + said.len() as u16 + 2;
    for x in start.saturating_sub(1)..=to {
        buf.set_string(x, area.y, " ", Style::new());
    }
    buf.set_string(start, area.y, DOT.to_string(), Style::new().fg(colour));
    buf.set_string(
        start + 2,
        area.y,
        said,
        Style::new().fg(Color::Rgb(0x80, 0x80, 0x80)),
    );
}

/// The dock dialed: a coral bar at the start of the row, and the command after it.
///
/// The bar is a left one-eighth block, not a box-drawing vertical and not a half block: a box
/// glyph is drawn down the middle of its cell, so it reads as a bar floating in a space, and a
/// half block is bolder than a mark on the margin wants to be. This one inks the leftmost
/// eighth, so it sits flush with the edge of the row and stays thin.
///
/// The bar is coral and the command is not, so the row is marked as the command's without
/// recolouring the text: what the pane is running reads like anything else it prints.
fn render_program(row: Rect, buf: &mut Buffer, status: &str) {
    /// The bar, inked to the left edge of its cell.
    const BAR: char = '▏';

    buf.set_string(
        row.x + 0,
        row.y,
        BAR.to_string(),
        Style::new().fg(Color::Rgb(0xFF, 0x7F, 0x50)),
    );
    // One cell clear of the bar.
    buf.set_string(row.x + 1, row.y, status, Style::new());
}
