//! One widget of the crate on an otherwise empty pane, for the component tests.
//!
//! The first argument names the widget: `button`, `tabs`, or `cross`. It is drawn in the middle
//! of the pane with one line under it saying what it reported — the only other thing on the
//! screen. The gallery draws all three; this draws one, so a snapshot of a pane changes when its
//! widget changed and for no other reason.
//!
//! `q` or `Esc` ends it.

use std::cell::Cell;
use std::io::Read;
use std::rc::Rc;
use std::time::{Duration, Instant};

use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use ratatui::crossterm::execute;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::Widget;
use termina::event::{Event, KeyCode, KeyEvent, KeyEventKind};
use termina::Parser;
use tuigui_components::events::take_key;
use tuigui_components::{cross, Button, Cross, PINK, Tabs};

/// The screens the tab strip chooses between.
const TABS: [(&str, Color); 3] = [
    ("Buttons", Color::Rgb(0xff, 0x69, 0xb4)),
    ("Chrome", Color::Rgb(0x69, 0xff, 0xb4)),
    ("About", Color::Rgb(0x69, 0xb4, 0xff)),
];

/// The blank row between the widget and the line that reports it.
const GAP: u16 = 1;

/// How long one stop frame of a move is held before the next is drawn.
///
/// A move is a handful of discrete frames, so this is the whole of its speed. Ten times the
/// 40ms it plays at normally, for looking at: each frame is held long enough to be read on its
/// own, and the gaps between them are where the shape of the move is visible.
const FRAME: Duration = Duration::from_millis(25);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let which = std::env::args().nth(1).unwrap_or_default();
    let mut pane = match which.as_str() {
        "button" => Pane::button(),
        "tabs" => Pane::tabs(),
        "cross" => Pane::cross(),
        other => {
            return Err(format!("pane takes `button`, `tabs` or `cross`, not {other:?}").into());
        }
    };

    let _tty = Raw::install()?;
    let mut terminal = ratatui::try_init()?;
    // Mouse reporting is a mode of the terminal device rather than of the drawing, and it is
    // off until something asks for it: without this the widget is pressable in principle and
    // unreachable in practice.
    execute!(std::io::stdout(), EnableMouseCapture)?;

    let mut parser = Parser::default();
    let mut bytes = [0u8; 256];

    // Drawn before anything is read: the read blocks until the terminal says something, so a
    // pane that waited for a first event would show an empty screen until the user guessed.
    terminal.draw(|frame| pane.draw(frame.area(), frame.buffer_mut(), &mut Vec::new()))?;

    // When the current stop frame started being shown, and whether the last frame drawn was
    // actually a new one. Two clocks, and this is the reason they are separate: `poll` returns
    // the moment there is *input*, and with any-event mouse tracking that is every twitch — so a
    // step counted per wake is a step per twitch, not a step per `FRAME`.
    let mut since = Instant::now();

    let ending = loop {
        // How long the wait may be: the rest of this frame's hold, or nothing at all when the
        // hold has already run out and the next frame is due.
        let moving = pane.is_moving();
        let left = match moving {
            true => FRAME.saturating_sub(since.elapsed()),
            false => Duration::ZERO,
        };
        let wait_for = match moving {
            // Nothing is animating: sleep until input arrives.
            false => None,
            // The hold is spent: do not wait at all, draw the next frame now.
            true if left.is_zero() => Some(Duration::ZERO),
            true => Some(left),
        };

        let ready = wait(wait_for)?;
        let mut events = Vec::new();
        if ready {
            match read_once(&mut parser, &mut bytes, &mut events) {
                Ok(false) => {}
                Ok(true) => {
                    if take_key(&mut events, |key| {
                        is_quit(key) && key.kind == KeyEventKind::Press
                    })
                    .is_some()
                    {
                        break Ok(());
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => break Ok(()),
                Err(error) => break Err(Box::new(error) as Box<dyn std::error::Error>),
            }
        }

        // The frame is drawn only when it is due: an input wake that arrives inside the hold
        // redraws nothing, because the picture on screen is still the frame being held.
        let due = !moving || since.elapsed() >= FRAME;
        if due {
            terminal.draw(|frame| {
                pane.draw(frame.area(), frame.buffer_mut(), &mut events);
            })?;
            if pane.is_moving() {
                // A move already in flight moves on to its next frame; a move that the draw just
                // started stays on its first. Either way the hold begins now, so the frame that
                // was just drawn is shown for the whole of `FRAME`.
                if moving {
                    pane.step(FRAME.as_secs_f32());
                    // The step that ends a move runs after the frame was drawn, so the line still
                    // says `moving` although the band has arrived. Draw the resting frame now,
                    // before the loop blocks on the next input.
                    if !pane.is_moving() {
                        terminal.draw(|frame| {
                            pane.draw(frame.area(), frame.buffer_mut(), &mut Vec::new());
                        })?;
                    }
                }
                since = Instant::now();
            }
        }
    };

    // Before the screen is restored: a terminal left reporting mouse events is one that keeps
    // printing them into the shell that comes back.
    execute!(std::io::stdout(), DisableMouseCapture)?;
    ratatui::restore();
    ending
}

/// The one widget this pane draws, and what it reported on the last frame.
enum Pane {
    /// A button and how many times its closure has run. The closure cannot borrow the field
    /// that owns it, so the count is a cell it owns and the pane reads back after the press.
    Button {
        clicks: Rc<Cell<u32>>,
        button: Button,
    },
    Tabs(Tabs),
    Cross(Cross),
}

impl Pane {
    fn button() -> Self {
        let clicks = Rc::new(Cell::new(0));
        let theirs = Rc::clone(&clicks);
        Self::Button {
            clicks,
            button: Button::new("Press me", PINK, move || theirs.set(theirs.get() + 1)),
        }
    }

    fn tabs() -> Self {
        Self::Tabs(Tabs::new(
            TABS.iter()
                .map(|(name, colour)| ((*name).to_string(), None, *colour))
                .collect(),
            0,
        ))
    }

    fn cross() -> Self {
        Self::Cross(Cross::new())
    }

    fn draw(&mut self, area: Rect, buf: &mut Buffer, events: &mut Vec<Event>) {
        let width = match self {
            Self::Button { button, .. } => button.width(),
            Self::Tabs(tabs) => tabs.width(),
            Self::Cross(_) => cross::WIDTH,
        }
        .min(area.width);
        // The widget, the gap, and the line: centred as one block.
        let y = area.y + area.height.saturating_sub(1 + GAP + 1) / 2;
        let widget = Rect::new(centred(area, width), y, width, 1);

        let said = match self {
            Self::Button { clicks, button } => {
                button.handle_events(widget, events);
                Widget::render(&*button, widget, buf);
                format!("clicks: {}", clicks.get())
            }
            Self::Tabs(tabs) => {
                tabs.handle_events(widget, events);
                Widget::render(&*tabs, widget, buf);
                match tabs.is_moving() {
                    true => format!("tab: {} moving", tabs.active()),
                    false => format!("tab: {} still", tabs.active()),
                }
            }
            Self::Cross(cross) => {
                cross.handle_events(widget, events);
                Widget::render(&*cross, widget, buf);
                match cross.is_held() {
                    true => "cross: down".into(),
                    false => "cross: up".into(),
                }
            }
        };

        // The answer is drawn after the widget has taken the frame's events, so a press and the
        // line that reports it are in the same frame.
        let width = u16::try_from(Line::raw(said.as_str()).width()).unwrap_or(u16::MAX);
        buf.set_string(
            centred(area, width.min(area.width)),
            y.saturating_add(1 + GAP),
            said,
            Style::new().fg(Color::Gray),
        );
    }

    /// Whether the strip is animating, which is what makes the loop keep drawing.
    fn is_moving(&self) -> bool {
        matches!(self, Self::Tabs(tabs) if tabs.is_moving())
    }

    /// Advance the strip by the time that passed, on the frame clock.
    fn step(&mut self, delta: f32) {
        if let Self::Tabs(tabs) = self {
            tabs.step(delta);
        }
    }
}

/// The column a row of `width` starts at, to be centred in `area`.
fn centred(area: Rect, width: u16) -> u16 {
    area.x + area.width.saturating_sub(width) / 2
}

/// Wait until the terminal has input, or until `timeout` has passed.
///
/// A `poll` of stdin rather than a sleep: the wait is for input *or* a deadline, and a sleep
/// would take the deadline every time even when a click was waiting. `None` blocks until there
/// is input, which is what a pane at rest wants — it needs no frames at all.
fn wait(timeout: Option<Duration>) -> std::io::Result<bool> {
    use rustix::event::{PollFd, PollFlags};

    let stdin = std::io::stdin();
    let mut fds = [PollFd::new(&stdin, PollFlags::IN)];
    let timeout = timeout.map(|timeout| rustix::event::Timespec {
        tv_sec: i64::try_from(timeout.as_secs()).unwrap_or(i64::MAX),
        tv_nsec: i64::from(timeout.subsec_nanos()),
    });
    // Interrupted by a signal is not a failure: the answer is "go round again", which is what
    // counting the ready descriptors gives when none is set.
    let ready = match rustix::event::poll(&mut fds, timeout.as_ref()) {
        Ok(ready) => ready,
        Err(rustix::io::Errno::INTR) => 0,
        Err(error) => return Err(error.into()),
    };
    Ok(ready > 0)
}

/// Read what the terminal has waiting into `events`, without waiting for more.
///
/// One read per call at most: more would be a loop that waits for input to stop arriving, and a
/// key held down never does. Answers whether anything arrived at all, since a frame with no
/// input has nothing new to act on.
fn read_once(
    parser: &mut Parser,
    bytes: &mut [u8; 256],
    events: &mut Vec<Event>,
) -> std::io::Result<bool> {
    let read = std::io::stdin().read(bytes)?;
    parser.parse(&bytes[..read], true);

    let mut arrived = false;
    while let Some(event) = parser.pop() {
        // Everything the parser made is kept: the widgets take what they act on, and a widget
        // that wanted a key would have nothing to look at if only the mouse were kept.
        events.push(event);
        arrived = true;
    }
    Ok(arrived)
}

/// Whether the event asks the pane to stop.
fn is_quit(key: &KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('q') | KeyCode::Escape)
}

/// Holds this terminal in raw mode, and restores the old settings when dropped.
///
/// Raw mode is what makes a key arrive as a byte rather than as a line, and it is the pane's own
/// business because the pane owns the terminal it draws on.
struct Raw {
    original: rustix::termios::Termios,
}

impl Raw {
    fn install() -> std::io::Result<Self> {
        let stdin = std::io::stdin();
        let original = rustix::termios::tcgetattr(&stdin).map_err(std::io::Error::from)?;
        let mut raw = original.clone();
        raw.make_raw();
        // `make_raw` clears `ISIG`, which turns `^C` from a signal into a byte. It is put back
        // so `^C` still stops this: a pane with no other way out is worse than one that cannot
        // see the key.
        raw.local_modes
            .insert(rustix::termios::LocalModes::ISIG);
        rustix::termios::tcsetattr(&stdin, rustix::termios::OptionalActions::Now, &raw)
            .map_err(std::io::Error::from)?;
        Ok(Self { original })
    }
}

impl Drop for Raw {
    fn drop(&mut self) {
        let _ = rustix::termios::tcsetattr(
            std::io::stdin(),
            rustix::termios::OptionalActions::Now,
            &self.original,
        );
    }
}
