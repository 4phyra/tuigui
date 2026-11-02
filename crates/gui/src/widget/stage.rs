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
use ratatui::widgets::{StatefulWidget, Widget};
use wire::transport::Connection;

use compositor::Compositor;

use compositor::WindowingServer;

/// How long a quiet session is paused before the window is looked at again.
///
/// A frame with nothing to send and nothing to read would otherwise return at once and be
/// drawn again immediately, so an idle program would cost a core. This is the pause that
/// keeps it cheap: the window is still looked at often enough to feel instant, and the
/// loop is not free-running.
const QUIET: std::time::Duration = std::time::Duration::from_millis(16);

/// Where a program's output waits while its window is on screen.
///
/// Read while the window is up — a program blocked on a full pipe would stop drawing —
/// and printed when the logs screen is back.
#[derive(Clone, Default)]
pub struct RingBuffer;

impl RingBuffer {
    pub fn push(&self, _bytes: &[u8]) {
        todo!("a bounded, line-wise ring of what the program said")
    }

    /// Every line waiting, oldest first.
    pub fn take(&self) -> Vec<String> {
        todo!()
    }

    pub fn len(&self) -> usize {
        todo!()
    }
}

/// The running program: the windows it draws, and which screen is showing them.
///
/// A struct and not a stage in a sequence, because the compositor does not change with the
/// screen: a program keeps running while the logs screen is up, and its window keeps
/// drawing into a buffer nobody is looking at. What changes is `screen`, which is a move
/// within this type rather than a change of it.
pub struct Stage<T: WindowingServer> {
    pub compositor: Compositor<T>,
    /// The codec this session streams with, announced on the wire before the first frame:
    /// the other side builds its decoder from it rather than assuming one.
    pub codec: codecs::CodecId,
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
    /// The terminal's own screen, after a stream: every line the client has printed is
    /// still there, and the link that streamed them is kept.
    Logs { connection: Connection },
    /// The alternate screen, with the program's window on it.
    ///
    /// The buffer belongs to this screen: it is the output that arrived while the window
    /// was up, and it exists exactly as long as this screen does. Leaving the stream
    /// drains it onto the logs screen, where it is printed.
    Stream {
        connection: Connection,
        buffer: RingBuffer,
    },
}

/// What a frame of a running program is given that the program does not own: the
/// terminal's own report of itself, and the input that arrived since the last frame.
///
/// Passed as the widget's state because a `render` is handed an area and a buffer and
/// nothing else, and these are needed to do the work: without them a frame could not tell
/// its windows how big the pane is, or what the user just pressed.
pub struct Props<'a> {
    /// The terminal's report of itself, read once per frame by the caller.
    pub winsize: rustix::termios::Winsize,
    /// This terminal's input, drained once per frame.
    pub events: &'a std::sync::mpsc::Receiver<termina::Event>,
}

impl<'a, T: WindowingServer> StatefulWidget for &'a mut Stage<T> {
    type State = Props<'a>;

    /// One frame of the running program: deliver input, advance it, and draw it.
    ///
    /// Input first, then the capture, then the draw. In that order because a capture is
    /// what the program committed since the last one: an event delivered after the capture
    /// is an event the next capture answers.
    ///
    /// Capturing only happens on the stream's screen. On the logs screen the window is not
    /// on the screen at all, so an encoded frame would be pixels nobody can see and a
    /// placement the other side cannot resolve.
    fn render(self, area: Rect, buf: &mut Buffer, props: &mut Props<'_>) {
        while let Ok(event) = props.events.try_recv() {
            if let Err(e) = self.compositor.input(&event, props.winsize) {
                log::warn!("could not deliver input: {e}");
                return;
            }
        }

        if let Page::Stream { connection, .. } = &self.page {
            // Whatever the other side has said since the last frame. Read before the capture, so a
            // message it sent in answer to the previous frame is acted on by this one.
            //
            // Only `Error` is handled: a reported failure is the far end saying the pane is broken,
            // and there is nothing here that could carry on meaningfully — the placeholder is to
            // make it impossible to miss, rather than to keep a half-working session alive.
            while let Ok(Some(message)) = connection.recv_next() {
                if let wire::message::Message::Error(why) = message {
                    panic!("the server reported a failure: {why}");
                }
            }

            self.compositor.frame(connection, QUIET, props.winsize);
            // The draw is not decoration: each window's cells are filled with its surface,
            // and those cells are what the other side scans for to find where a picture
            // goes. A frame that did not draw is a frame with nothing to place.
            Widget::render(&mut self.compositor, area, buf);
        }
    }
}

impl<T: WindowingServer> Stage<T> {
    /// A stage over a window session, starting on `screen`.
    ///
    /// The compositor is built from the server rather than passed in: what a caller has is
    /// a session — wayland, dummy, or later a remote one — and the compositor over it is
    /// this type's own part.
    pub fn new(server: T, codec: codecs::CodecId, page: Page) -> Self {
        Self {
            compositor: Compositor::new(server),
            codec,
            page,
        }
    }

    /// Swap the screen, and hand back what the stream was holding.
    ///
    /// Entering the stream needs the link, which only a connection can give, so it is
    /// passed in. Leaving drains the buffer and returns the lines: the caller prints them,
    /// because the terminal belongs to the caller and not to this type.
    pub fn swap(&mut self, connection: Connection) -> Vec<String> {
        match std::mem::replace(&mut self.page, Page::Init) {
            // Leaving the stream: its buffer is the last thing it said, and it has to be
            // printed before the screen that held it goes.
            Page::Stream { connection, buffer } => {
                self.page = Page::Logs { connection };
                buffer.take()
            }
            // Entering it: a fresh buffer, because the program's output is about to have
            // nowhere to go again.
            Page::Logs { .. } | Page::Init => {
                self.page = Page::Stream {
                    connection,
                    buffer: RingBuffer::default(),
                };
                Vec::new()
            }
        }
    }

    /// Whether the stream's screen is the one up.
    ///
    /// The terminal is not this type's to touch, so it answers the question and the caller
    /// does the crossing: entering and leaving the alternate screen, and holding the mouse
    /// only while the program is the thing on screen.
    pub fn showing_stream(&self) -> bool {
        matches!(self.page, Page::Stream { .. })
    }

    /// Whether the program has ended.
    pub fn finished(&self) -> bool {
        self.compositor.finished()
    }
}
