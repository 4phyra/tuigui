//! The pane the client draws, and the session it draws from.
//!
//! ```text
//!   widget.rs          this: the widget, the Session, and the screens
//!   widget/stage.rs    the program running: its link, its windows, and the screen up
//! ```

mod dock;
mod stage;

pub use dock::HEIGHT as DOCK_HEIGHT;
pub use stage::{Page, Props, Stage};

use codecs::CodecId;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, StatefulWidget, Widget as _};

/// The socket a Unix Socket session dials.
///
/// One fixed name, so a user and a server can both write it down without a scan, and it is
/// the server's to bind.
///
/// Under `XDG_RUNTIME_DIR` and not `/tmp`: a socket in `/tmp` is reachable by every user on
/// the machine, and this is the platform's directory for exactly this.
pub fn socket_path() -> std::path::PathBuf {
    let dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
    std::path::Path::new(&dir).join("tuigui.sock")
}

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
    pub stage: Result<Stage, String>,
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
    /// Start a session: the window session this machine composites, and the socket the
    /// other side dials.
    ///
    /// Built here rather than by the caller because it is the session's own part: the
    /// caller says which program, how big, and how to encode, and everything a running
    /// window needs is assembled around that. The session is a compositor in this
    /// process, one output.
    ///
    /// The socket is bound and announced before the first frame is drawn, so the advert
    /// that frame puts on screen is one that answers.
    pub async fn new(
        size: (u32, u32),
        codec: CodecId,
        stdin: Input,
        program: String,
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
                dpi: 1.0,
                use_x11: false,
            },
            codec,
        )?;
        Ok(Self {
            stage: Ok(Stage::new(server, Page::default())),
            dock: dock::Dock::new(socket_path(), program).await?,
            input_receiver,
        })
    }

    /// Whether the session has ended.
    pub fn done(&self) -> bool {
        match &self.stage {
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
        self.stage.as_ref().is_ok_and(Stage::showing_stream)
    }
}

/// The client itself: a widget over [`Session`].
///
/// No state of its own and no parameter: there is one session, so the widget is the
/// type and `Session` is all it needs.
#[derive(Default)]
pub struct GuiWidget;

impl StatefulWidget for GuiWidget {
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

        // The program's frame. Its own `render` is where the work is: it takes the input
        // that arrived, advances the windows, and draws them when they are the thing on
        // screen. The size and the input channel go in as its state, because a `render` is
        // handed an area and a buffer and nothing else.
        match &mut session.stage {
            Ok(stage) => {
                let mut props = Props {
                    winsize,
                    events: &session.input_receiver,
                };
                StatefulWidget::render(stage, content, buf, &mut props);
            }
            // A failure is drawn here and not by a step: the pane is this widget's, and a
            // step that drew its own failure would be drawing in a place it does not own.
            Err(message) => {
                log::error!("{message}");
                ratatui::widgets::Clear.render(content, buf);
                buf.set_style(content, Style::new().bg(Color::Rgb(0x40, 0, 0)));
                let lines: Vec<Line> = std::iter::once(Line::from("the session failed"))
                    .chain(message.lines().map(Line::from))
                    .collect();
                Paragraph::new(lines).centered().render(content, buf);
            }
        }

        // The dock, under the screen: the one part of the pane that is the same in every
        // state, and the one thing a user can press to leave a screen whose content takes
        // the keyboard. It shares the stage as its state, so a press is the swap itself.
        //
        // Borrowed field by field rather than through a method: the dock and the stage are
        // different fields of one struct, so two mutable borrows of them at once are two
        // distinct places and the compiler accepts both.
        StatefulWidget::render(&mut session.dock, dock_area, buf, &mut session.stage);
    }
}
