//! One session: what the pane is showing, the dock that starts it, and the input that arrived.
//!
//! The widget that draws it ([`crate::widget::MainWidget`]) is the other half: this type is what a
//! frame is handed as its state, and what the draw loop drives.
//!
//! # What this does not own
//!
//! The program, its window session and its link. Those are the run path's ([`crate::client`]),
//! which starts the program, waits for the window it draws, and streams it. This side binds the
//! link and draws the announcement the other side dials; the link itself is handed over at once,
//! because a transport has one owner and that owner is the half that accepts on it. What arrives
//! back is a window that is already placed, and a reader on the program's output: enough to draw,
//! and nothing more.
//!
//! # One event, every half
//!
//! An event is offered to everything on the pane: the program's windows first, then the dock. Each
//! says whether it wants the frame drawn again, and the answer a caller acts on is the union — a
//! click the program took still moves the pointer, and one it refused belongs to the furniture.
//! Deciding that here is what keeps the draw loop from knowing which half acts on what.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::StatefulWidget;
use wire::transport::Transport;

use compositor::CompositableWindow;

use crate::widget::{dock, stage, Stage};

/// One session: what the pane is showing.
pub struct Session {
    /// Where the session is, or what went wrong getting there.
    ///
    /// A `Result` and not a state, because a step consumes the state it is given: there has to be
    /// something in its place while the step runs, and a failure is the only honest thing to put
    /// there.
    pub stage: stage::StageResult,
    /// The dock, which is what gets the session started and what switches screens once there is one.
    pub dock: dock::Dock,
}

impl Session {
    /// A pane waiting for its program: the dock announcing a link nothing has dialed yet.
    ///
    /// Nothing is started here — not the program, not the window session. This side binds the link
    /// and draws the announcement; the dial is what the run path turns into a program.
    ///
    /// The link leaves immediately, down `transport`: binding is this side's, because the
    /// announcement is what this side draws, but accepting on it is the run path's, and a transport
    /// has one owner. What stays here is the announcement, which is a record and not a link.
    pub async fn new(
        channel: crate::Channel,
        program: String,
        transport: tokio::sync::oneshot::Sender<Box<dyn Transport>>,
    ) -> std::io::Result<Self> {
        let bound: Box<dyn Transport> = match channel {
            crate::Channel::P2p => Box::new(wire::p2p::P2p::host().await?),
            crate::Channel::Unix => Box::new(wire::unix::UnixSocket::host().await?),
        };
        let advert = bound
            .to_magic()
            .ok_or_else(|| std::io::Error::other("a bound transport has no address to announce"))?;
        let dock = dock::Dock::new(advert, program)?;
        transport
            .send(bound)
            .map_err(|_| std::io::Error::other("the run path is gone: nothing to carry the link"))?;
        Ok(Self {
            stage: stage::StageResult(Ok(Stage::new())),
            dock,
        })
    }

    /// Whether the session has ended.
    pub fn done(&self) -> bool {
        match &self.stage.0 {
            // Failed, or finished: either way there is nothing left to do.
            Err(_) => true,
            Ok(stage) => stage.finished(),
        }
    }

    /// The program's window is up, and its output has somewhere to go.
    ///
    /// The window is already placed — the run path handed it over — and the output arrives as the
    /// reader on the program's own words: the two are what a screen needs, and neither exists
    /// before the program has drawn something.
    pub fn from_any_to_windows(
        &mut self,
        root: std::sync::Arc<CompositableWindow>,
        stdout: std::io::PipeReader,
    ) {
        if let Ok(stage) = &mut self.stage.0 {
            stage.from_any_to_windows(root, stdout);
        }
        self.dock.state = dock::DockState::Streaming;
    }

    /// Offer one event to the whole pane, and answer whether a frame is owed.
    ///
    /// Every half is offered it and the answers are unioned: a press the program claimed still moves
    /// the pointer, and one it refused is the dock's. A half that is not there — a stage that failed
    /// — takes nothing and asks for nothing.
    pub fn handle_event(
        &mut self,
        event: &termina::Event,
        winsize: rustix::termios::Winsize,
        dock_area: Rect,
    ) -> bool {
        // The program first: it is what the pane shows, and the pointer and the keyboard belong to
        // it while its window is on screen.
        let painted = match &mut self.stage.0 {
            Ok(stage) => stage.handle_event(event, winsize),
            Err(_) => false,
        };

        // Then the furniture, which is offered the same event rather than the leftovers: the dock
        // draws the strip and the cross, and neither is placed by the compositor.
        let furniture = self.dock.handle_event(event, dock_area, &mut self.stage.0);

        painted || furniture
    }

    /// Do the work a window change wants, and set the root's box to the pane.
    pub async fn step(&mut self, winsize: rustix::termios::Winsize) {
        if let Ok(stage) = &mut self.stage.0 {
            stage.step(winsize).await;
        }
    }

    /// Whether something on the pane is still moving, so the next frame is already owed.
    ///
    /// The dock's strip is the only thing that animates over many frames; everything else is drawn
    /// once and stands until something says otherwise. Asked after a frame rather than kept as a
    /// flag, because the answer belongs to whatever is moving.
    pub fn is_animating(&self) -> bool {
        self.dock.is_animating()
    }

    /// Whether the alternate screen is where this session draws.
    ///
    /// Asked by the caller because the terminals are the caller's: the session knows which screen it
    /// wants, and the two — one three rows at the cursor, one the whole screen — are built and owned
    /// where the writer is.
    pub fn is_alternate(&self) -> bool {
        self.stage.as_ref().is_ok_and(Stage::is_alternate)
    }
}

/// What one frame of a session is given that the session does not own.
pub struct SessionProps<'a> {
    /// Seconds since the previous frame, which paces anything animated.
    pub delta: f32,
    /// Where the program's words go, for the caller to put on the screen once the frame is up.
    ///
    /// Filled rather than printed because the caller is the half holding the terminal: the logs
    /// screen is the terminal underneath the dock, and the pane a frame draws is every visible row
    /// of an inline viewport — its own top row *is* the cursor row, so a line written here would be
    /// painted over by the next frame. The caller puts these above the viewport instead.
    pub program_stdout: &'a mut Vec<u8>,
}

impl<'a> StatefulWidget for &'a mut Session {
    type State = SessionProps<'a>;

    /// One frame of the session: the program's screen above, the dock below.
    #[tracing::instrument(name = "frame", skip_all)]
    fn render(self, area: Rect, buf: &mut Buffer, props: &mut Self::State) {
        // A terminal with no room above the dock has no layout to give it: there is no frame that
        // satisfies it, and this is not a size to invent one for.
        let content = Rect {
            height: area
                .height
                .checked_sub(dock::HEIGHT)
                .expect("this terminal has no room above the dock"),
            ..area
        };
        let dock_area = Rect {
            y: area.bottom() - dock::HEIGHT,
            height: dock::HEIGHT,
            ..area
        };

        // The terminal's own report of itself, read once here because this is the call that draws.
        let winsize = rustix::termios::tcgetwinsize(std::io::stdin())
            .expect("the terminal must report its own size");

        StatefulWidget::render(
            &mut self.stage,
            content,
            buf,
            &mut stage::StageProps {
                winsize,
                program_stdout: props.program_stdout,
            },
        );

        // The frame the session asked to end on is the last one: the dock's rows are blanked rather
        // than skipped, and the logs screen underneath is what is left.
        if self.stage.as_ref().is_ok_and(Stage::finished) {
            return;
        }
        StatefulWidget::render(
            &mut self.dock,
            dock_area,
            buf,
            &mut dock::DockProps {
                stage: &mut self.stage.0,
                delta: props.delta,
            },
        );
    }
}
