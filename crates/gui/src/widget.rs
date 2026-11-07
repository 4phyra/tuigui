//! The pane the client draws, and the session it draws from.
//!
//! ```text
//!   widget.rs          this: the widget, the Session, and the screens
//!   widget/stage.rs    the program running: its link, its windows, and the screen up
//! ```

mod dock;
mod stage;

pub use dock::HEIGHT as DOCK_HEIGHT;
pub use stage::{Page, Stage, StageProps, RING_BUF_CAPACITY};

use codecs::CodecId;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::StatefulWidget;

/// This terminal's input, boxed so a caller may hand in its own.
///
/// The session holds one of these for its whole life, and `Gui::run` is generic
/// over what it was given, so the handle is boxed rather than named.
pub type Input = Box<dyn std::io::Read + Send>;

/// One session: where it is, and which screen it is showing.
pub struct Session {
    /// Where the session is, or what went wrong getting there.
    ///
    /// A `Result` and not a state, because a step consumes the state it is given: there
    /// has to be something in its place while the step runs, and a failure is the only
    /// honest thing to put there. A step that never returns its next state leaves the
    /// failure behind, and the pane shows it.
    pub stage: stage::StageResult,
    /// The dock, which is what gets a session started and what switches screens once
    /// there is one. It shares the stage above as its state, so a button press is the swap.
    pub dock: dock::Dock,
    /// This terminal's input, as parsed events.
    ///
    /// The reader thread owns the parsing and the reading; this end only
    /// receives. Nothing is lost while a state is not reading: the channel
    /// holds whatever arrived, and the streaming step drains it all at once.
    pub input_receiver: std::sync::mpsc::Receiver<termina::Event>,
}

impl Session {
    /// Start a session: the window session this machine composites, and the channel the
    /// other side dials.
    ///
    /// Built here rather than by the caller because it is the session's own part: the
    /// caller says which program, how big, how to encode and how to be reached, and
    /// everything a running window needs is assembled around that. The session is a
    /// compositor in this process, one output.
    ///
    /// The channel is bound and announced before the first frame is drawn, so the advert
    /// that frame puts on screen is one that answers.
    ///
    /// Binding is the one step the two channels do not share, which is why it is here and
    /// not in [`Dock::new`]: a socket picks a path and listens on it, and an endpoint is
    /// minted. Everything after it — announce, draw, wait to be dialed — is common.
    pub async fn new(
        size: (u32, u32),
        codec: CodecId,
        channel: crate::Channel,
        stdin: Input,
        program: String,
        scale: f64,
    ) -> std::io::Result<Self> {
        // Read and parsed on a thread of its own: a read of the raw terminal blocks until
        // the pane has something to say, and the draw loop cannot sit in that read. The
        // events wait in the channel until a frame drains them, so nothing typed while the
        // session connects is lost.
        let input_receiver = crate::inputs::start(std::io::BufReader::new(stdin))?;
        let server = crate::windowing::PlatformWindowing::new(
            wayland::Options {
                resolution: size,
                default_window_size: (size.0 / 2, size.1 / 2),
                dpi: scale,
                use_x11: false,
            },
            codec,
        )?;
        let transport: Box<dyn wire::transport::Transport> = match channel {
            crate::Channel::P2p => Box::new(wire::p2p::P2p::host().await?),
            crate::Channel::Unix => Box::new(wire::unix::UnixSocket::host().await?),
        };
        Ok(Self {
            stage: stage::StageResult(Ok(Stage::new(server, Page::default()))),
            dock: dock::Dock::new(transport, program).await?,
            input_receiver,
        })
    }

    /// Whether the session has ended.
    pub fn done(&self) -> bool {
        match &self.stage.0 {
            // Failed, or finished: either way there is nothing left to do.
            Err(_) => true,
            // A stage with nothing running yet has not ended: the dock is still working on
            // being reachable.
            Ok(stage) => stage.finished(),
        }
    }

    /// Whether the alternate screen is where this session draws.
    ///
    /// Asked by the caller because the terminals are the caller's: the session knows which
    /// screen it wants, and the two — one three rows at the cursor, one the whole screen —
    /// are built and owned where the writer is.
    pub fn is_alternate(&self) -> bool {
        self.stage
            .as_ref()
            .is_ok_and(|stage| matches!(stage.get_page(), Page::Stream { .. }))
    }

}

/// The client itself: a widget over [`Session`].
///
/// The delta is the one thing a frame knows that the session does not: how long it has been since
/// the last draw. Anything animated is paced by it, so a slower pane plays the same move at the
/// same speed rather than in fewer, longer steps.
pub struct GuiWidget<'a> {
    /// Seconds since the previous frame.
    pub delta: f32,
    /// Where the program's words are collected, for the caller to put on the screen.
    ///
    /// Sized to the ring that fills it, so a frame that drains a full ring does not grow it: the
    /// words handed on are exactly what the ring held, and nothing beyond that can arrive.
    pub program_stdout: &'a mut Vec<u8>,
}

impl StatefulWidget for GuiWidget<'_> {
    type State = Session;

    /// One frame.
    ///
    /// The span is here and not around the draw that calls it: this is the one
    /// call that knows what a frame is made of — the session's step, the
    /// capture, the encode — so this is where the profiler's timeline should be
    /// divided. Every stage below it lands inside the frame it belongs to.
    #[tracing::instrument(name = "frame", skip_all)]
    fn render(self, area: Rect, buf: &mut Buffer, session: &mut Session) {
        // The dock is the last three rows and the content is what is above them. On the
        // inline terminal those three rows are the whole area, so the content is empty and
        // drawing into it draws nothing; on the fullscreen one the stream gets the rest.
        //
        // A terminal with no room above the dock has no layout to give it: there is no
        // frame that satisfies it, and this is not a size to invent one for.
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

        // The terminal's own report of itself, read once here because this is the call
        // that draws: the grid says how many cells the pane is, and `ws_xpixel`/
        // `ws_ypixel` say how many pixels, which is the size the session's program is
        // configured at. Read rather than kept, because a terminal changes shape without
        // anything here being told: a font that grew keeps the grid and changes the
        // picture.
        let winsize = rustix::termios::tcgetwinsize(std::io::stdin())
            .expect("the terminal must report its own size");

        // One drain for the whole frame: the program's half and the furniture's half are
        // offered the same list, and each takes what it acts on. A widget that read the channel
        // itself would eat a click the other half is waiting for.
        let mut events = Vec::new();
        while let Ok(event) = session.input_receiver.try_recv() {
            events.push(event);
        }

        // The program's frame. The wrapper's `render` is where the work is: it hands the input to
        // the windows, advances them, draws them when they are the thing on screen — or draws the
        // session's failure in their place.
        let mut props = StageProps {
            winsize,
            events: &mut events,
            program_stdout: self.program_stdout,
        };
        StatefulWidget::render(&mut session.stage, content, buf, &mut props);

        // The frame the session asked to end on is the last one: the dock's rows are blanked
        // rather than skipped. Skipping leaves whatever the previous frame wrote there — the
        // terminal only redraws the cells that changed — so the way out is a clear, and the logs
        // screen underneath is what is left.
        if !session.stage.as_ref().is_ok_and(Stage::finished) {
            // The dock, under the screen: the one part of the pane that is the same in every
            // state, and the one thing a user can press to leave a screen whose content takes
            // the keyboard. It shares the stage as its state, so a press is the swap itself, and
            // it is offered the same events the screen above it was: a click the program did not
            // take is still there for the furniture.
            //
            // Borrowed field by field rather than through a method: the dock and the stage are
            // different fields of one struct, so two mutable borrows of them at once are two
            // distinct places and the compiler accepts both.
            StatefulWidget::render(
                &mut session.dock,
                dock_area,
                buf,
                &mut dock::DockProps {
                    stage: &mut session.stage.0,
                    events: &mut events,
                    delta: self.delta,
                },
            );
        }
    }
}
