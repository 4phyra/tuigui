//! Every widget in the crate, drawn on one screen so they can be pressed.
//!
//! One column, centred in the pane, with [`SEPARATION`] blank rows between its elements: the
//! tab strip, a button for each of its tabs, and a line saying what was last pressed. What
//! the gallery shows is the whole dispatch rule — one [`Events`] is offered to each widget in
//! turn, in draw order, and a widget takes only the part it acts on, so a click reaches
//! exactly one of them.
//!
//! `q` or `Esc` ends it.

use std::io::Read;

use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use ratatui::crossterm::execute;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::StatefulWidget;
use termina::event::{Event, KeyCode, KeyEvent, KeyEventKind};
use termina::Parser;
use tuigui_components::{cross, Button, Cross, Events, Tabs};

/// The screens the tab strip chooses between.
const TABS: [(&str, Color); 3] = [
    ("Buttons", Color::Rgb(0xff, 0x69, 0xb4)),
    ("Chrome", Color::Rgb(0x69, 0xff, 0xb4)),
    ("About", Color::Rgb(0x69, 0xb4, 0xff)),
];

/// The blank rows between two elements of the list.
const SEPARATION: u16 = 2;

/// How many rows a button occupies.
const BUTTON_HEIGHT: u16 = 1;

/// How many rows the tab strip occupies.
const TABS_HEIGHT: u16 = 1;

/// How long one stop frame of a move is held before the next is drawn.
///
/// A move is a handful of discrete frames, so this is the whole of its speed. Ten times the
/// 40ms it plays at normally, for looking at: each frame is held long enough to be read on its
/// own, and the gaps between them are where the shape of the move is visible.
const FRAME: std::time::Duration = std::time::Duration::from_millis(25);

/// How many rows one line of text occupies.
const LINE_HEIGHT: u16 = 1;

/// Wait until the terminal has input, or until `timeout` has passed.
///
/// A `poll` of stdin rather than a sleep: the wait is for input *or* a deadline, and a sleep
/// would take the deadline every time even when a click was waiting. `None` blocks until
/// there is input, which is what a strip at rest wants — it needs no frames at all.
fn wait(timeout: Option<std::time::Duration>) -> std::io::Result<bool> {
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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _tty = Raw::install()?;
    let mut terminal = ratatui::try_init()?;
    // Mouse reporting is a mode of the terminal device rather than of the drawing, and it is
    // off until something asks for it: without this the widgets are pressable in principle
    // and unreachable in practice.
    execute!(std::io::stdout(), EnableMouseCapture)?;

    let mut gallery = Gallery::new();
    let mut parser = Parser::default();
    let mut bytes = [0u8; 256];

    // Drawn before anything is read: the read blocks until the terminal says something, so a
    // gallery that waited for a first event would show an empty pane until the user guessed.
    terminal.draw(|frame| {
        gallery.draw(frame.area(), frame.buffer_mut(), &mut Events::default());
    })?;

    // When the current stop frame started being shown, and whether the last frame drawn was
    // actually a new one. Two clocks, and this is the reason they are separate: `poll` returns
    // the moment there is *input*, and with any-event mouse tracking that is every twitch — so a
    // step counted per wake is a step per twitch, not a step per `FRAME`.
    let mut since = std::time::Instant::now();

    let ending = loop {
        // How long the wait may be: the rest of this frame's hold, or nothing at all when the
        // hold has already run out and the next frame is due.
        let moving = gallery.is_moving();
        let left = match moving {
            true => FRAME.saturating_sub(since.elapsed()),
            false => std::time::Duration::ZERO,
        };
        let wait_for = match moving {
            // Nothing is animating: sleep until input arrives.
            false => None,
            // The hold is spent: do not wait at all, draw the next frame now.
            true if left.is_zero() => Some(std::time::Duration::ZERO),
            true => Some(left),
        };

        let ready = wait(wait_for)?;
        let mut events = Events::default();
        if ready {
            match read_once(&mut parser, &mut bytes, &mut events) {
                Ok(false) => {}
                Ok(true) => {
                    if events
                        .take_key(|key| is_quit(key) && key.kind == KeyEventKind::Press)
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
                gallery.draw(frame.area(), frame.buffer_mut(), &mut events);
            })?;
            gallery.settle();
            if gallery.is_moving() {
                // A move already in flight moves on to its next frame; a move that the draw just
                // started stays on its first. Either way the hold begins now, so the frame that
                // was just drawn is shown for the whole of `FRAME`.
                if moving {
                    gallery.step();
                }
                since = std::time::Instant::now();
            }
        }
    };

    // Before the screen is restored: a terminal left reporting mouse events is one that
    // keeps printing them into the shell that comes back.
    execute!(std::io::stdout(), DisableMouseCapture)?;
    ratatui::restore();
    ending
}

/// Everything the gallery draws, and what it remembers between frames.
struct Gallery {
    /// The strip: the tabs, their colours, and which one is active. Drawn with the frame's
    /// events, so a click on it moves the active tab itself.
    tabs: Tabs,
    /// One button per tab. All of them are drawn: the list shows every component the crate
    /// has, and the strip above them only says which one a session would be showing.
    buttons: Vec<Button>,
    /// One slot per button, set by that button's own closure when it is pressed. Read and reset
    /// by the gallery after the frame.
    pressed: Vec<std::rc::Rc<std::cell::Cell<bool>>>,
    /// What the last press was, as the line under the list says it.
    last: String,
    /// The cross, and the two things it reports. Its slots are read after the frame, like the
    /// buttons'.
    cross: Cross,
    cross_pressed: std::rc::Rc<std::cell::Cell<bool>>,
    cross_released: std::rc::Rc<std::cell::Cell<bool>>,
}

impl Gallery {
    fn new() -> Self {
        // One press slot per button, shared with that button's closure: a closure cannot borrow
        // the gallery the buttons are fields of, so what it did is recorded in a cell it owns and
        // the gallery reads it back after the frame.
        let buttons = TABS
            .iter()
            .map(|(name, colour)| {
                let pressed = std::rc::Rc::new(std::cell::Cell::new(false));
                let theirs = std::rc::Rc::clone(&pressed);
                (
                    pressed,
                    Button::new(*name, *colour, move || theirs.set(true)),
                )
            })
            .collect::<Vec<_>>();
        let (pressed, buttons): (Vec<_>, Vec<_>) = buttons.into_iter().unzip();
        let cross_pressed = std::rc::Rc::new(std::cell::Cell::new(false));
        let cross_released = std::rc::Rc::new(std::cell::Cell::new(false));
        let pressed_slot = std::rc::Rc::clone(&cross_pressed);
        let released_slot = std::rc::Rc::clone(&cross_released);
        Self {
            tabs: Tabs::new(
                TABS.iter()
                    .map(|(name, colour)| ((*name).to_string(), *colour))
                    .collect(),
                0,
            ),
            buttons,
            pressed,
            last: "nothing pressed yet".into(),
            cross: Cross::new(
                move || pressed_slot.set(true),
                move || released_slot.set(true),
            ),
            cross_pressed,
            cross_released,
        }
    }

    fn draw(&mut self, area: Rect, buf: &mut Buffer, events: &mut Events) {
        // The column is as tall as its elements plus the gaps between them, and it starts
        // where centring it leaves it: the list is laid out before anything is drawn, so a
        // frame that is too small clips rather than drawing a skewed list.
        //
        // One strip, one line per button, the cross, and the answer under them all.
        let elements = self.buttons.len().saturating_add(3);
        let total = TABS_HEIGHT
            .saturating_add(BUTTON_HEIGHT * self.buttons.len() as u16)
            .saturating_add(LINE_HEIGHT * 2)
            .saturating_add(SEPARATION * (elements as u16).saturating_sub(1));
        let mut y = area.y + area.height.saturating_sub(total) / 2;

        // Each element is centred on its own row, so a narrow button sits under a wide strip
        // rather than against its left edge.
        let width = self.tabs.width().min(area.width);
        StatefulWidget::render(
            &mut self.tabs,
            Rect::new(centred(area, width), y, width, TABS_HEIGHT),
            buf,
            events,
        );
        y = y.saturating_add(TABS_HEIGHT + SEPARATION);

        for button in &mut self.buttons {
            let width = button.width().min(area.width);
            StatefulWidget::render(
                button,
                Rect::new(centred(area, width), y, width, BUTTON_HEIGHT),
                buf,
                events,
            );
            y = y.saturating_add(BUTTON_HEIGHT + SEPARATION);
        }
        // The cross on a row of its own, three cells wide: the glyph is the target, and the block
        // it becomes while held needs the cells either side of it.
        StatefulWidget::render(
            &mut self.cross,
            Rect::new(centred(area, cross::WIDTH), y, cross::WIDTH, LINE_HEIGHT),
            buf,
            events,
        );
        y = y.saturating_add(LINE_HEIGHT + SEPARATION);

        // The answer, in the same column: which tab is up, and what was last pressed.
        let said = format!("{} · showing tab {}", self.last, self.tabs.active());
        let width = u16::try_from(Line::raw(said.as_str()).width()).unwrap_or(u16::MAX);
        buf.set_string(
            centred(area, width.min(area.width)),
            y,
            said,
            Style::new().fg(Color::Gray),
        );
    }

    /// Whether the strip is animating, which is what makes the loop keep drawing.
    fn is_moving(&self) -> bool {
        self.tabs.is_moving()
    }

    /// Advance the strip by one stop frame, on the frame clock.
    fn step(&mut self) {
        self.tabs.step();
    }

    /// Take what each button's closure recorded during the frame.
    fn settle(&mut self) {
        for (index, pressed) in self.pressed.iter().enumerate() {
            if pressed.replace(false) {
                let name = TABS.get(index).map_or("?", |(name, _)| name);
                self.last = format!("pressed {name}");
            }
        }
        // The cross reports both ends of a press, so the line can say which one just happened.
        if self.cross_pressed.replace(false) {
            self.last = "cross pressed".into();
        }
        if self.cross_released.replace(false) {
            self.last = "cross released".into();
        }
    }
}

/// The column a row of `width` starts at, to be centred in `area`.
fn centred(area: Rect, width: u16) -> u16 {
    area.x + area.width.saturating_sub(width) / 2
}

/// Read what the terminal has waiting into `events`, without waiting for more.
///
/// One read per call at most: more would be a loop that waits for input to stop arriving, and
/// a key held down never does. Answers whether anything arrived at all, since a frame with no
/// input has nothing new to act on.
fn read_once(
    parser: &mut Parser,
    bytes: &mut [u8; 256],
    events: &mut Events,
) -> std::io::Result<bool> {
    let read = std::io::stdin().read(bytes)?;
    parser.parse(&bytes[..read], true);

    let mut arrived = false;
    while let Some(event) = parser.pop() {
        match event {
            Event::Key(key) => {
                events.keypresses.push(key);
                arrived = true;
            }
            // Only the last pointer event is kept: a pointer's history is where it is now, so
            // drawing every motion between two frames would draw the same answer repeatedly.
            Event::Mouse(mouse) => {
                events.mouse_hits = Some(mouse);
                arrived = true;
            }
            _ => {}
        }
    }
    Ok(arrived)
}

/// Whether the event asks the gallery to stop.
fn is_quit(key: &KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('q') | KeyCode::Escape)
}

/// Holds this terminal in raw mode, and restores the old settings when dropped.
///
/// Raw mode is what makes a key arrive as a byte rather than as a line, and it is the
/// gallery's own business because the gallery owns the terminal it draws on.
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
        // so `^C` still stops this: a gallery with no other way out is worse than one that
        // cannot see the key.
        raw.local_modes.insert(rustix::termios::LocalModes::ISIG);
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
