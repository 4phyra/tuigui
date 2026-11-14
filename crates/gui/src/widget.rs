//! The pane the client draws.
//!
//! ```text
//!   widget.rs          this: the terminals a frame is drawn on, and the session it draws
//!   widget/dock.rs     the three rows that start a session and switch screens
//!   widget/stage.rs    the program running: its windows, and the screen up
//!   session.rs         what a frame is: the screen, the dock, and the input that moves them
//! ```
//!
//! The session knows which screen it is on; the terminals are here, because a screen is a device
//! and only the half holding the writer can cross between them. A frame is drawn by asking this
//! to draw, and it decides which of the two the frame lands on.

pub(crate) mod dock;
pub(crate) mod stage;

pub use crate::session::Session;
pub use dock::HEIGHT as DOCK_HEIGHT;
pub use stage::{Page, Stage, StageProps, RING_BUF_CAPACITY};

use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;

use crate::session::SessionProps;

/// The pane: the session, and the two terminals it is drawn on.
///
/// Two terminals, because a `Terminal`'s viewport is fixed when it is built and the two screens
/// want different answers: the logs screen wants three rows anchored under the cursor, where lines
/// printed above them scroll into the terminal's own history, and the stream wants the whole
/// screen with a window on it.
///
/// Each backend gets this process's own stdout, and they never draw at the same time — a frame
/// picks one — so there is nothing for them to interleave and nothing to share: one writer, two
/// handles to it, one of them writing at any given moment.
pub struct MainWidget {
    /// What is being drawn: the screen, the dock, and the windows behind them.
    session: Session,
    /// The logs screen: three rows at the cursor, and the terminal's own history above them.
    primary: Terminal<CrosstermBackend<std::io::Stdout>>,
    /// The stream: the alternate screen, with the program's window on it.
    secondary: Terminal<CrosstermBackend<std::io::Stdout>>,
    /// Whether the previous frame was drawn on the alternate screen.
    ///
    /// Kept because entering and leaving are transitions of the terminal device rather than of a
    /// frame: the crossing happens once, on the frame the screen changes.
    was_alternate: bool,
    /// The area the last frame drew into.
    ///
    /// Kept because an event arrives between frames, and a press is aimed at a cell: the area is
    /// the one the frame the user is looking at was drawn in, and nothing outside `draw` can ask
    /// for it. On the inline screen it is three rows at the cursor, which the terminal decides.
    area: Rect,
    /// When the previous frame was drawn, which is what paces anything animated.
    ///
    /// Here because this is the only call that makes a frame, and how long one took is a fact
    /// nothing below can measure.
    last: std::time::Instant,
    /// Where a frame's worth of the program's words is collected.
    ///
    /// Sized to the ring that fills it, so a frame that drains a full ring does not grow it, and
    /// emptied rather than freed between frames so a chatty program allocates it once.
    program_stdout: Vec<u8>,
}

impl MainWidget {
    /// The pane, over a session, with both terminals built where the writer is.
    ///
    /// Built before the input thread exists, and that order is not cosmetic: `Viewport::Inline`
    /// asks the terminal where the cursor is, and crossterm reads that answer off stdin itself. A
    /// reader holding stdin would take the reply.
    pub fn new(session: Session) -> std::io::Result<Self> {
        Ok(Self {
            session,
            primary: Terminal::with_options(
                CrosstermBackend::new(std::io::stdout()),
                ratatui::TerminalOptions {
                    viewport: ratatui::Viewport::Inline(DOCK_HEIGHT),
                },
            )?,
            secondary: Terminal::new(CrosstermBackend::new(std::io::stdout()))?,
            was_alternate: false,
            area: Rect::ZERO,
            last: std::time::Instant::now(),
            program_stdout: Vec::with_capacity(RING_BUF_CAPACITY),
        })
    }

    /// Offer one event to the pane, and answer whether a frame is owed.
    ///
    /// The dock's rows are the bottom three of the area the frame drew, and the events that move
    /// them are the pointer's: a press is aimed at a cell, so the area has to be the one the user
    /// is looking at rather than the whole terminal.
    pub fn handle_event(&mut self, event: &termina::Event) -> std::io::Result<bool> {
        let winsize = rustix::termios::tcgetwinsize(std::io::stdin())?;
        let dock_area = Rect {
            y: self.area.bottom() - DOCK_HEIGHT,
            height: DOCK_HEIGHT,
            ..self.area
        };
        Ok(self.session.handle_event(event, winsize, dock_area))
    }

    /// Do a turn of the pane: a window changed, and the root is told the pane's size.
    ///
    /// One call, because the wait and the work cannot be held at once: what the wait answers borrows
    /// the stack it changes, so the borrow has to end between the two.
    pub async fn step(&mut self) {
        let winsize = rustix::termios::tcgetwinsize(std::io::stdin())
            .expect("the terminal must report its own size");
        self.session.step(winsize).await;
    }

    /// The program's window is up, and its output has somewhere to go.
    pub fn from_any_to_windows(
        &mut self,
        root: std::sync::Arc<compositor::CompositableWindow>,
        stdout: std::io::PipeReader,
    ) {
        self.session.from_any_to_windows(root, stdout);
    }

    /// Whether the session is over.
    pub fn done(&self) -> bool {
        self.session.done()
    }

    /// One frame: the screen the session is on, and the program's words where they belong.
    ///
    /// The span is here and not around the caller: this is the one call that knows what a frame is
    /// made of, so this is where the profiler's timeline should be divided.
    #[tracing::instrument(name = "frame", skip_all)]
    pub fn render(&mut self) -> std::io::Result<()> {
        let delta = std::mem::replace(&mut self.last, std::time::Instant::now())
            .elapsed()
            .as_secs_f32();

        // The screen is the session's to name and the terminals are this side's, so the crossing
        // happens here: two modes, set once and never toggled per frame, because both belong to
        // the terminal device rather than to a screen.
        match (self.session.is_alternate(), self.was_alternate) {
            (true, false) => {
                crossterm::execute!(
                    self.secondary.backend_mut(),
                    crossterm::terminal::EnterAlternateScreen,
                    crossterm::event::EnableMouseCapture
                )?;
                // The terminal diffs against the buffer it drew last, and that buffer belongs to
                // the screen it drew on. Coming back to the alternate screen whose contents were
                // dropped by the crossing makes that diff a lie, so the back buffer is discarded
                // and the whole screen drawn again.
                //
                // `resize` and not `clear`: the two do the same work for a fullscreen viewport —
                // clear it all, reset the back buffer — and only `clear` reads the cursor back to
                // restore it, which cannot be answered here. This process's stdin is the reader
                // thread's.
                self.secondary.resize(self.secondary.size()?.into())?;
                self.was_alternate = true;
            }
            (false, true) => {
                crossterm::execute!(
                    self.secondary.backend_mut(),
                    crossterm::terminal::LeaveAlternateScreen,
                    crossterm::event::DisableMouseCapture
                )?;
                self.was_alternate = false;
            }
            // The same screen as the last frame: no crossing, because the device is already there.
            _ => {}
        }


        // The program's words while the window was up were held, and this is where they are
        // released: the screen they were written on is the one that is up again, and the client's
        // own printing is the only thing that may move it.
        self.print_program_stdout()?;

        let terminal = match self.was_alternate {
            true => &mut self.secondary,
            false => &mut self.primary,
        };
        terminal.draw(|frame| {
            let area = frame.area();
            frame.render_stateful_widget(
                &mut self.session,
                area,
                &mut SessionProps {
                    delta,
                    program_stdout: &mut self.program_stdout,
                },
            );
        })?;
        log::info!("render: the frame is drawn");
        self.area = match self.was_alternate {
            true => self.secondary.size()?.into(),
            false => {
                let area = self.primary.size()?;
                Rect::new(0, area.height.saturating_sub(DOCK_HEIGHT), area.width, DOCK_HEIGHT)
            }
        };

        // After the draw, because only the logs screen fills this — the stream keeps the ring from
        // filling instead of printing it.
        self.print_program_stdout()?;
        Ok(())
    }

    /// Put whatever the program said on the terminal's own screen, above the pane.
    ///
    /// Above and not in the pane, because the pane the inline terminal draws is its visible rows
    /// and its own top row *is* the cursor row: a line written inside it would be painted over by
    /// the next frame, which is exactly the output a user wants to keep reading.
    fn print_program_stdout(&mut self) -> std::io::Result<()> {
        if self.program_stdout.is_empty() {
            return Ok(());
        }
        // A line each: the thread that reads the pipe puts the terminator back on every line it
        // takes off, so the ring's bytes are newline-separated throughout and `lines` is exactly
        // what the program printed.
        let text = String::from_utf8_lossy(&self.program_stdout);
        let lines: Vec<&str> = text.lines().collect();
        self.primary.insert_before(lines.len() as u16, |buf| {
            for (row, line) in lines.iter().enumerate() {
                buf.set_string(0, row as u16, line, ratatui::style::Style::new());
            }
        })?;
        // Emptied, not freed: a program that prints every frame allocates this buffer once for the
        // whole run rather than once per frame.
        self.program_stdout.clear();
        Ok(())
    }
}
