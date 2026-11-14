//! The running program, drawn: its windows and the screen showing them.
//!
//! ```text
//!   Init ──stream──▶ Stream ──leave──▶ Logs ──enter──▶ Stream ──▶ …
//! ```
//!
//! Nothing here starts a program, holds a link or waits on one: the run path owns all of that. This
//! half is given a window and the program's output, and its whole job is to say where each window's
//! cells go.
//!
//! The compositor travels with the screen rather than sitting beside it: a window is only composited
//! while there is a screen to composite it on, so the two that have one carry it and `Init` — before
//! the program drew anything — carries nothing.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{StatefulWidget, Widget};
use ringbuf::traits::{Consumer as _, Observer as _, Producer as _, Split as _};
use std::io::BufRead as _;
use std::sync::Arc;

use compositor::{CompositableWindow, Compositor};

/// How many bytes of the program's output can wait to be looked at.
///
/// Public because the caller sizes the buffer it drains into to the ring that fills it.
pub const RING_BUF_CAPACITY: usize = 1 << 18;

/// The share of the ring the stream's screen keeps free.
const RING_BUF_FREE: f32 = 0.3;

/// The running program: the windows it draws, and which screen is showing them.
pub struct Stage {
    pub page: Page,
}

/// Which screen is up, and what belongs to it.
///
/// The terminal keeps two screens apart for us, so this is a choice between two things that already
/// exist rather than a page we compose: the **logs** screen is the terminal's own and holds every
/// line the client has printed, and the **stream** is the alternate screen with the program's window
/// on it.
#[derive(Default)]
pub enum Page {
    /// Before the program drew a window: nothing to composite, so the dock is the whole screen.
    #[default]
    Init,
    /// The alternate screen, with the program's window on it.
    ///
    /// The compositor is here because this is the screen that shows what it places, and the reading
    /// end of the program's output travels with the page because it is drained from wherever the page
    /// is shown and there is one of it.
    Stream {
        compositor: Compositor,
        stdout: ringbuf::HeapCons<u8>,
    },
    /// The terminal's own screen, with the program's output being printed onto it.
    ///
    /// The compositor comes along and is not drawn: the window is not on this screen, and its cells
    /// would be an anchor for pictures nobody can see. It is carried so that going back to the stream
    /// is a screen change rather than a rebuild.
    Logs {
        compositor: Compositor,
        stdout: ringbuf::HeapCons<u8>,
    },
    /// Over, and how far along the leaving is.
    ///
    /// Two frames, because leaving takes two: the frame that asks is still drawn on the screen it is
    /// leaving — the dock included — and the one after has to be drawn on the logs screen to clear
    /// what is left there.
    Exiting {
        /// Whether the screen has been crossed: the next frame draws the logs screen and ends.
        ready: bool,
    },
}

impl Page {
    /// Which of the two screens this is, as the dock's strip numbers them.
    ///
    /// `Init` is the logs screen: it is the terminal's own, and it is where the client has been
    /// printing all along.
    pub fn tab(&self) -> usize {
        match self {
            Page::Stream { .. } => 1,
            Page::Logs { .. } | Page::Init | Page::Exiting { .. } => 0,
        }
    }
}

/// What a frame of a running program is given that the program does not own: the terminal's own
/// report of itself.
pub struct StageProps<'a> {
    /// The terminal's report of itself, read once per frame by the caller.
    pub winsize: rustix::termios::Winsize,
    /// Where the program's words go, for the caller to put on the screen once the frame is up.
    pub program_stdout: &'a mut Vec<u8>,
}

/// The session's screen, or the failure that replaced it.
///
/// A wrapper because the trait is not this crate's: `StatefulWidget` cannot be implemented for
/// `Result`, which is not a local type. `Deref` keeps every reader of `session.stage` writing what it
/// did: the wrapper is only here for the raw render, which is the one place that draws a failure in
/// place of a screen.
#[derive(derive_deref::Deref)]
pub struct StageResult(pub Result<Stage, String>);

impl<'a> StatefulWidget for &'a mut StageResult {
    type State = StageProps<'a>;

    fn render(self, area: Rect, buf: &mut Buffer, props: &mut Self::State) {
        match &mut self.0 {
            Ok(stage) => {
                if let Err(e) = stage.render_inner(area, buf, props) {
                    log::warn!("{e}");
                }
            }
            Err(message) => {
                log::error!("{message}");
                ratatui::widgets::Clear.render(area, buf);
                buf.set_style(area, Style::new().bg(Color::Rgb(0x40, 0, 0)));
                let lines: Vec<Line> = std::iter::once(Line::from("the session failed"))
                    .chain(message.lines().map(Line::from))
                    .collect();
                ratatui::widgets::Widget::render(
                    ratatui::widgets::Paragraph::new(lines).centered(),
                    area,
                    buf,
                );
            }
        }
    }
}

impl Stage {
    /// One frame, with everything that can fail left to the caller.
    ///
    /// Input first, then the draw. In that order because the input is what a window's program acts
    /// on, and the draw is what the pane shows of how it answered.
    fn render_inner(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        props: &mut StageProps<'_>,
    ) -> std::io::Result<()> {
        match &mut self.page {
            // The screen with the window on it: the program is given this frame's input, and each
            // window's cells are filled with its surface, which is what the far side scans for to
            // find where a picture goes.
            //
            // Input only here, and not on the logs screen: there the window is not on the screen at
            // all, so a key typed is the furniture's — the strip's shortcut included — and
            // forwarding it would let the program act on a press the strip already claimed.
            Page::Stream { compositor, stdout } => {
                Widget::render(compositor, area, buf);

                // The words are not on this screen, so nothing here wants them — but the thread
                // reading the pipe is parked on it, and a pipe nobody drains stops the program. Room
                // is made for that thread by dropping the oldest lines, which are the ones that
                // would have scrolled away unseen anyway.
                let free = (RING_BUF_CAPACITY as f32 * RING_BUF_FREE) as usize;
                let vacant = stdout.vacant_len();
                if vacant < free {
                    stdout.skip(free - vacant);
                }
            }
            // Over. A page that is not ready is the frame that asked to leave: nothing is drawn on
            // it, because the screen it would be drawn on is the one being left. Flipping it ready is
            // what lets the next frame cross to the logs screen and clear what is left there.
            Page::Exiting { ready } => *ready = true,
            // The program's words go into the caller's buffer: this screen *is* the terminal
            // underneath, and only the half holding that terminal can put a line above the pane.
            Page::Logs { stdout, .. } => {
                while let Some(written) = stdout.write_into(props.program_stdout, None) {
                    written?;
                }
            }
            Page::Init => {}
        }
        Ok(())
    }
}

impl Stage {
    /// A stage with nothing running yet: the dock is the whole of the screen.
    pub fn new() -> Self {
        Self {
            page: Page::default(),
        }
    }

    /// The program is running and its window is up: there is something to composite.
    ///
    /// The compositor is built here rather than passed about, because a window is the one thing a
    /// compositor is made over and the run path is what hands it over. The program's words come with
    /// it: they belong to the screen the window is on.
    pub fn from_any_to_windows(
        &mut self,
        root: Arc<CompositableWindow>,
        stdout: std::io::PipeReader,
    ) {
        let compositor = Compositor::new(Arc::downgrade(&root));
        self.page = Page::Stream {
            compositor,
            stdout: ring(stdout),
        };
    }

    /// Back to the logs screen. The program keeps running — only the screen changes.
    pub fn from_streaming_to_logs(&mut self) {
        let Page::Stream { compositor, stdout } = std::mem::replace(&mut self.page, Page::Init)
        else {
            // Nothing was streaming, so there is nothing to leave.
            return;
        };
        self.page = Page::Logs { compositor, stdout };
    }

    /// Back to the program's window, after a look at the logs.
    pub fn from_logs_to_streaming(&mut self) {
        let Page::Logs { compositor, stdout } = std::mem::replace(&mut self.page, Page::Init)
        else {
            return;
        };
        self.page = Page::Stream { compositor, stdout };
    }

    /// Over, from wherever the session was, one frame short of the crossing.
    pub fn from_any_to_exiting(&mut self) {
        self.page = Page::Exiting { ready: false };
    }

    /// Offer one event to the program's windows, and answer whether a frame is owed.
    ///
    /// Only the screen with the window on it takes the event: on the logs screen the window is not
    /// on the screen at all, so a key typed there is the furniture's — the strip's shortcut included
    /// — and forwarding it would let the program act on a press the strip already claimed.
    pub fn handle_event(&mut self, event: &termina::Event, winsize: rustix::termios::Winsize) -> bool {
        let Page::Stream { compositor, .. } = &mut self.page else {
            return false;
        };
        compositor
            .handle_event(event.clone(), winsize)
            .map_err(|e| log::warn!("the event could not be offered: {e}"))
            .unwrap_or(false)
    }

    /// Do the work a window change wants.
    ///
    /// The borrow ends here rather than at the wait, which is why the two are separate: the task a
    /// wait answers borrows the stack it changes, and the stack can only lend it once.
    pub fn apply(&mut self, task: compositor::Task, winsize: rustix::termios::Winsize) {
        let compositor = match &mut self.page {
            Page::Stream { compositor, .. } | Page::Logs { compositor, .. } => compositor,
            _ => return,
        };
        task(compositor, winsize);
    }

    /// Do the work a window change wants, and tell the pane's program what size its box is.
    pub async fn step(&mut self, winsize: rustix::termios::Winsize) {
        let compositor = match &mut self.page {
            Page::Stream { compositor, .. } | Page::Logs { compositor, .. } => compositor,
            _ => return,
        };
        if let Err(e) = compositor.step(winsize).await {
            log::warn!("the pane's windows could not be placed: {e}");
        }
    }
    /// The page that is up.
    pub fn get_page(&self) -> &Page {
        &self.page
    }

    /// Whether the screen on show is the alternate one.
    pub fn is_alternate(&self) -> bool {
        matches!(self.page, Page::Stream { .. })
    }

    /// Whether the session is over: the user asked to leave and the frame that carries the screen
    /// across has been drawn.
    pub fn finished(&self) -> bool {
        matches!(self.page, Page::Exiting { ready: true })
    }
}

/// The program's output, put on a ring the frames drain.
///
/// The words arrive on a thread of their own: the ring is handed out as its reading end, and the
/// thread that fills it is detached because it lives exactly as long as the pipe does — the program
/// exiting closes it, and the read that was parked returns end-of-file.
fn ring(stdout: std::io::PipeReader) -> ringbuf::HeapCons<u8> {
    let (mut producer, consumer) = ringbuf::HeapRb::<u8>::new(RING_BUF_CAPACITY).split();
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout)
            .lines()
            .map_while(Result::ok)
        {
            // Full means the frames fell behind their margin. A producer has no blocking push and
            // no notification, so asking again and sleeping between asks is all there is.
            while let 0 = producer.push_slice(format!("{line}\n").as_bytes()) {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
    });
    consumer
}
