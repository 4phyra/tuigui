//! The running program: its windows, and the screen showing them.
//!
//! ```text
//!   Init ──stream──▶ Stream ──leave──▶ Logs ──enter──▶ Stream ──▶ …
//! ```
//!
//! Progress is made by drawing. A frame is one step: the input that arrived is given to
//! the windows, the windows are advanced, and — on the stream's screen — their pictures
//! are captured and sent. On the logs screen nothing is captured, because the window is
//! not on the screen and a picture of it would be pixels nobody can see.
//!
//! The link lives in the screen rather than on this type. A screen that has never
//! streamed has no link to hold, and one that has left the stream has finished with it.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{StatefulWidget, Widget};
use ringbuf::traits::{Consumer as _, Observer as _, Producer as _, Split as _};
use std::io::BufRead as _;
use wire::transport::Connection;

use compositor::Compositor;

use crate::windowing::PlatformWindowing;

/// How long a quiet session is paused before the window is looked at again.
///
/// A frame with nothing to send and nothing to read would otherwise return at once and be
/// drawn again immediately, so an idle program would cost a core. This is the pause that
/// keeps it cheap: the window is still looked at often enough to feel instant, and the
/// loop is not free-running.
const QUIET: std::time::Duration = std::time::Duration::from_millis(16);

/// How many bytes of the program's output can wait to be looked at.
///
/// Public because the caller sizes the buffer it drains into to the ring that fills it.
pub const RING_BUF_CAPACITY: usize = 1 << 18;

/// The share of the ring the stream's screen keeps free.
const RING_BUF_FREE: f32 = 0.3;

/// The running program: the windows it draws, and which screen is showing them.
///
/// A struct and not a stage in a sequence, because the compositor does not change with the
/// screen: a program keeps running while the logs screen is up, and its window keeps
/// drawing into a buffer nobody is looking at. What changes is `screen`, which is a move
/// within this type rather than a change of it.
pub struct Stage {
    pub compositor: Compositor<PlatformWindowing>,
    pub page: Page,
}

/// Which screen is up, and what belongs to it.
///
/// The terminal keeps two screens apart for us, so this is a choice between two things
/// that already exist rather than a page we compose: the **logs** screen is the terminal's
/// own and holds every line the client has printed, and the **stream** is the alternate
/// screen with the program's window on it.
#[derive(Default)]
pub enum Page {
    /// Before the first stream: nothing has been connected, so there is nothing to show
    /// and nothing to print. The dock is the whole of the screen.
    #[default]
    Init,
    /// The alternate screen, with the program's window on it.
    ///
    /// The reading end of the program's output travels with the page, because it is drained from
    /// wherever the page is shown and there is one of it. Nothing is printed here: the window is
    /// what is on screen, so the ring is only emptied to keep the thread reading the pipe moving.
    Stream {
        connection: Connection,
        stdout: ringbuf::HeapCons<u8>,
    },
    /// The terminal's own screen, with the program's output being printed onto it.
    ///
    /// Everything the ring holds is written straight out: this is the terminal the client's own
    /// output has always gone to, and the dock is what sits under it.
    Logs {
        connection: Connection,
        stdout: ringbuf::HeapCons<u8>,
    },
    /// Over, and how far along the leaving is.
    ///
    /// Two frames, because leaving takes two: the frame that asks is still drawn on the screen it
    /// is leaving — the dock included — and the one after has to be drawn on the logs screen to
    /// clear what is left there. A single frame cannot do both, since which terminal it draws on
    /// is decided before the frame's body runs.
    ///
    /// A page rather than a flag beside the stage, because it is the same question the others
    /// answer — which screen is up — and here the answer is "the logs one, and then the end".
    Exiting {
        /// Whether the screen has been crossed: the next frame draws the logs screen and ends.
        ready: bool,
    },
}

impl Page {
    /// Which of the two screens this is, as the dock's strip numbers them.
    ///
    /// `Init` is the logs screen: it is the terminal's own, and it is where the client has been
    /// printing all along. The number is what a press on the strip reports, so it is the page
    /// that says it rather than the caller counting variants.
    pub fn tab(&self) -> usize {
        match self {
            Page::Stream { .. } => 1,
            Page::Logs { .. } | Page::Init | Page::Exiting { .. } => 0,
        }
    }
}

/// What a frame of a running program is given that the program does not own: the
/// terminal's own report of itself, and the input that arrived since the last frame.
///
/// Passed as the widget's state because a `render` is handed an area and a buffer and
/// nothing else, and these are needed to do the work: without them a frame could not tell
/// its windows how big the pane is, or what the user just pressed.
pub struct StageProps<'a> {
    /// The terminal's report of itself, read once per frame by the caller.
    pub winsize: rustix::termios::Winsize,
    /// This frame's input: the terminal's own events, drained once by the caller.
    ///
    /// Drained once and passed by reference rather than read here, because the dock is offered
    /// the same list: an event this half consumed would never reach the half that draws the
    /// furniture. A widget takes what it acts on and leaves the rest.
    pub events: &'a mut Vec<termina::Event>,
    /// Where the program's words go, for the caller to put on the screen once the frame is up.
    ///
    /// Filled rather than printed because the caller is the half holding the terminal: the logs
    /// screen is the terminal underneath the dock, and the pane a frame draws is every visible row
    /// of an inline viewport — its own top row *is* the cursor row, so a line written here would be
    /// painted over by the next frame. The caller puts these above the viewport instead.
    ///
    /// Empty on every other screen: there the window is what is on screen, so a line would be a
    /// line behind the picture, and the stream's frame keeps the ring from filling instead.
    pub program_stdout: &'a mut Vec<u8>,
}


/// The session's screen, or the failure that replaced it.
///
/// A wrapper because the trait is not this crate's: `StatefulWidget` cannot be implemented for
/// `Result`, which is not a local type. `Deref` keeps every reader of `session.stage` writing what
/// it did: the wrapper is only here for the raw render, which is the one place that draws a
/// failure in place of a screen.
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
    /// Input first, then the capture, then the draw. In that order because a capture is what the
    /// program committed since the last one: an event delivered after the capture is an event the
    /// next capture answers.
    ///
    /// Capturing only happens on the stream's screen. On the logs screen the window is not on the
    /// screen at all, so an encoded frame would be pixels nobody can see and a placement the other
    /// side cannot resolve.
    fn render_inner(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        props: &mut StageProps<'_>,
    ) -> std::io::Result<()> {
        for event in props.events.iter() {
            self.compositor
                .input(event, props.winsize)
                .map_err(std::io::Error::other)?;
        }

        match &mut self.page {
            Page::Stream { connection, stdout } => {
                // Whatever the other side has said since the last frame. Read before the capture,
                // so a message it sent in answer to the previous frame is acted on by this one.
                //
                // Only `Error` is handled: a reported failure is the far end saying the pane is
                // broken, and there is nothing here that could carry on meaningfully — the
                // placeholder is to make it impossible to miss, rather than to keep a half-working
                // session alive.
                while let Ok(Some(message)) = connection.recv_next() {
                    if let wire::message::Message::Error(why) = message {
                        return Err(std::io::Error::other(format!(
                            "the server reported a failure: {why}"
                        )));
                    }
                }

                self.compositor
                    .frame(QUIET, props.winsize)
                    .map_err(std::io::Error::other)?;
                // The draw is not decoration: each window's cells are filled with its surface, and
                // those cells are what the other side scans for to find where a picture goes. A
                // frame that did not draw is a frame with nothing to place.
                Widget::render(&mut self.compositor, area, buf);

                // The words are not on this screen, so nothing here wants them — but the thread
                // reading the pipe is parked on it, and a pipe nobody drains stops the program.
                // Room is made for that thread by dropping the oldest lines, which are the ones
                // that would have scrolled away unseen anyway.
                let free = (RING_BUF_CAPACITY as f32 * RING_BUF_FREE) as usize;
                let vacant = stdout.vacant_len();
                if vacant < free {
                    stdout.skip(free - vacant);
                }
            }
            // Over. A page that is not ready is the frame that asked to leave: nothing is drawn on
            // it, because the screen it would be drawn on is the one being left. Flipping it ready
            // is what lets the next frame cross to the logs screen and clear what is left there.
            Page::Exiting { ready } => *ready = true,
            // The program's words go into the caller's buffer: this screen *is* the terminal
            // underneath, and only the half holding that terminal can put a line above the pane.
            //
            // TODO: the ring is drained into a Vec here and split into lines there, so a frame's
            // words are copied twice before they reach the screen. Draining straight into the
            // terminal, or handing out the ring's own slices, would do it once — worth doing if a
            // program ever prints in bulk rather than a line at a time.
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
    /// A stage over a window session, starting on `screen`.
    ///
    /// The compositor is built from the server rather than passed in: what a caller has is
    /// a session, and the compositor over it is this type's own part.
    pub fn new(server: PlatformWindowing, page: Page) -> Self {
        Self {
            compositor: Compositor::new(server),
            page,
        }
    }

    /// The first stream: a link was dialed, so the program starts and its window goes up.
    ///
    /// The session is given the connection first, because the program's windows are what send the
    /// frames: a window minted before the session knows where they go has nowhere to write, and
    /// there is no window to mint until the program runs.
    ///
    /// Started before the page changes, so a failure is reported as one rather than as a stream
    /// that never draws anything: the page stays on the logs screen and the error goes to the
    /// caller, which is the pane that draws it.
    ///
    /// The program's output arrives on a thread of its own and travels with the page from then on:
    /// the ring is handed out as its reading end, and the thread that fills it is detached because
    /// it lives exactly as long as the pipe does — the program exiting closes it, and the read that
    /// was parked returns end-of-file.
    pub fn from_init_to_streaming(
        &mut self,
        connection: Connection,
        program: &str,
    ) -> Result<(), String> {
        self.compositor
            .windowing_mut()
            .set_connection(connection.clone());
        self.compositor
            .spawn(std::path::Path::new(program))
            .map_err(|e| format!("could not start {program}: {e}"))?;
        let stdout = self
            .compositor
            .take_stdout()
            .ok_or_else(|| format!("{program} started without an output pipe"))?;

        let (mut producer, consumer) = ringbuf::HeapRb::<u8>::new(RING_BUF_CAPACITY).split();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout)
                .lines()
                .map_while(Result::ok)
            {
                // Full means the frames fell behind their margin. A producer has no blocking push
                // and no notification, so asking again and sleeping between asks is all there is.
                while let 0 = producer.push_slice(format!("{line}\n").as_bytes()) {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            }
        });

        self.page = Page::Stream {
            connection,
            stdout: consumer,
        };
        Ok(())
    }

    /// Back to the logs screen. The program keeps running — only the screen changes.
    pub fn from_streaming_to_logs(&mut self) {
        let Page::Stream { connection, stdout } = std::mem::replace(&mut self.page, Page::Init)
        else {
            // Nothing was streaming, so there is nothing to leave.
            return;
        };
        self.page = Page::Logs { connection, stdout };
    }

    /// Back to the program's window, after a look at the logs.
    ///
    /// The link is the one already in the page: the program never stopped, so there is nothing to
    /// start and nothing to hand over.
    pub fn from_logs_to_streaming(&mut self) {
        let Page::Logs { connection, stdout } = std::mem::replace(&mut self.page, Page::Init)
        else {
            return;
        };
        self.page = Page::Stream { connection, stdout };
    }

    /// Over, from wherever the session was, one frame short of the crossing.
    ///
    /// The screen goes back first — the alternate screen is the program's, and a session that ended
    /// on it would hand back a hole — and that takes two frames: this one asks, and the next is
    /// drawn on the logs screen where there is something to clear.
    pub fn from_any_to_exiting(&mut self) {
        self.page = Page::Exiting { ready: false };
    }

    /// The page that is up.
    pub fn get_page(&self) -> &Page {
        &self.page
    }

    /// Whether the session is over: the program ended, or the user asked to leave and the frame
    /// that carries the screen across has been drawn.
    ///
    /// A page still asking is not over: the frame that asks is drawn on the screen it is leaving,
    /// and the one after it is the one that hands the logs screen back.
    pub fn finished(&self) -> bool {
        matches!(self.page, Page::Exiting { ready: true }) || self.compositor.finished()
    }
}
