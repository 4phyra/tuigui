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
use wire::transport::{BoxFuture, Connection, Transport};

use super::Stage;

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
}

/// The stages before there is a program: bound and waiting to be dialed, and dialed.
///
/// Both belong to the dock rather than to the session because both are about being
/// reachable, and the dock is the thing that is reachable: it binds the socket, it holds
/// the advert the other side scans for, and it is what waits to be dialed.
pub enum DockState {
    /// Bound and announced, waiting for the other side to dial.
    Pairing {
        /// The announcement as cells: the transport's own `to_magic`, cut into braille and
        /// held for the life of the socket.
        ///
        /// Held rather than encoded per frame because a record is a fixed thing, and drawing
        /// it fresh each frame would make a standing announcement flicker — the one thing a
        /// mark on a screen must not do.
        barcode: Vec<cellcode::Cell>,
        /// The wait, polled once per frame. Polled rather than awaited: a frame that
        /// awaited here would never finish, and the frame has to finish for the record it
        /// just drew to reach the terminal and cause the dial.
        waiting: BoxFuture<'static, std::io::Result<Connection>>,
    },
    /// Dialed. Nothing else: the link is the stage's, and it is handed over as soon as the
    /// other side answers.
    Paired,
}

impl Dock {
    /// Bind a socket and announce it, so the other side can dial this pane.
    ///
    /// The address is the transport's own: it is bound here, asked for its announcement,
    /// and the announcement is what gets drawn. Nothing in this crate formats it, which is
    /// what keeps the text on screen and the text the far side scans for the same text.
    ///
    /// Binding happens before the first frame, because the advert has nowhere to be until
    /// a socket answers at it: drawing one first would announce an address that refuses.
    ///
    /// The wait for a dial is **not** held here. It used to be a future polled once per frame with
    /// a no-op waker, on the theory that re-polling was all it needed; it is not. The reactor is
    /// edge-triggered, so read readiness is delivered once, to whichever poll registered a waker,
    /// and a no-op waker throws that away. The accept then stays pending however many frames pass,
    /// which looks exactly like a deadlock: a pane waiting for a dial that has already arrived.
    pub async fn new(path: std::path::PathBuf, program: String) -> std::io::Result<Self> {
        let socket = wire::unix::UnixSocket::new(path);
        socket.bind().await?;
        let advert = socket
            .to_magic()
            .ok_or_else(|| std::io::Error::other("a bound socket has no address to announce"))?;
        log::info!("announcing {advert}");
        // The record as cells, padded to a fixed length: blank braille in front, so the row
        // is the same every frame and two panes can be compared by eye.
        let mut barcode = Vec::new();
        cellcode::encode(&advert, |cell| barcode.push(cell))
            .map_err(|e| std::io::Error::other(format!("the announcement does not fit: {e}")))?;
        Ok(Self {
            state: DockState::Pairing {
                barcode,
                waiting: Box::pin(async move { socket.accept().await }),
            },
            program,
        })
    }
}

impl StatefulWidget for &mut Dock {
    /// The session's stage, so the dock can act on the session rather than describe it: it
    /// is what decides which screen is up, and a button that could only report a wish would
    /// put the swap one call away from the press that caused it.
    type State = Result<Stage, String>;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        // The wait moves the session before anything is drawn, so this frame shows what it
        // moved to rather than what it was.
        self.poll(state);

        Block::bordered()
            .border_type(ratatui::widgets::BorderType::Rounded)
            .border_style(Style::new().fg(Color::DarkGray))
            .render(area, buf);

        // The row inside the border, which is the whole of the dock's content: the box is
        // three rows because a box is what makes it read as furniture under a screen.
        let inner = Rect {
            x: area.x + 1,
            y: area.y + 1,
            width: area.width.saturating_sub(2),
            height: 1,
        };

        // The two screens as buttons, at the right edge, the one that is up marked. Nothing
        // presses them yet: a press needs the mouse and the dock does not read the terminal.
        const LOGS: &str = " Logs ";
        const WINDOWS: &str = " Windows ";
        const BUTTONS: [(&str, bool); 2] = [(LOGS, false), (WINDOWS, true)];

        let buttons_width: u16 = BUTTONS.iter().map(|(label, _)| label.len() as u16).sum();
        let buttons_x = inner.right().saturating_sub(buttons_width);

        // Everything left of the buttons is the state's to draw, so a long line is cut off
        // rather than drawn over them.
        let row = Rect {
            width: buttons_x.saturating_sub(inner.x),
            ..inner
        };

        match &self.state {
            DockState::Pairing { barcode, .. } => {
                render_waiting(inner, row, barcode, buf, self.status(state))
            }
            DockState::Paired => render_program(row, buf, self.status(state)),
        }

        // The light, always: on the dock's own top border, because what it says — this pane is
        // waiting, or it is paired — is true in every state. Only the word beside it and the
        // colour it breathes in change, so the mark itself never appears or disappears.
        let (said, colour) = match self.state {
            DockState::Pairing { .. } => ("pairing", (0xFF, 0x69, 0xB4)),
            DockState::Paired => ("paired", (0x69, 0xFF, 0xB4)),
        };
        light(area, buf, said, colour);

        let showing_stream = state.as_ref().is_ok_and(Stage::showing_stream);
        let mut x = buttons_x;
        for (label, screen) in BUTTONS {
            let style = match screen == showing_stream {
                true => Style::new()
                    .fg(Color::Black)
                    .bg(Color::Rgb(0xff, 0x69, 0xB4)),
                false => Style::new().fg(Color::DarkGray),
            };
            buf.set_string(x, inner.y, label, style);
            x += label.len() as u16;
        }
    }
}

impl Dock {
    /// Poll the wait once, and move to [`DockState::Connected`] when the other side dials.
    ///
    /// Polled rather than awaited: the frame has to finish for the announcement it drew to
    /// reach the other side, and the dial only starts once it has. A poll that is pending is
    /// the normal case for every frame until then.
    ///
    /// Polled by hand rather than with `futures::poll!`, because a `render` is not async and
    /// there is no task to be polled inside: the waker is a no-op and every frame asks again,
    /// which is all this wait needs — nothing here is waiting to be woken.
    fn poll(&mut self, stage: &mut Result<Stage, String>) {
        let DockState::Pairing { waiting, .. } = &mut self.state else {
            return;
        };
        match poll_once(waiting.as_mut()) {
            std::task::Poll::Ready(Ok(connection)) => {
                log::info!("dialed by the other side");

                // The program starts now, and not before: a client that ran before there was a
                // link would draw into a compositor whose frames had nowhere to go — and the
                // link is what tells it which display to reach, so there was nothing to start it
                // with until now.
                //
                // The session is given the connection first, because the program's windows are
                // what send the frames: a window minted before the session knows where they go
                // has nowhere to write, and there is no window to mint until the program runs.
                //
                // Started before the swap so a failure is reported as one, rather than as a
                // stream that never draws anything.
                let started = match stage {
                    Ok(stage) => {
                        stage.compositor.windowing_mut().set_connection(connection.clone());
                        stage
                            .compositor
                            .spawn(std::path::Path::new(&self.program))
                            .map_err(|e| format!("could not start {}: {e}", self.program))
                    }
                    Err(e) => Err(e.clone()),
                };
                match started {
                    Ok(()) => {
                        if let Ok(stage) = stage {
                            stage.swap(connection);
                        }
                        self.state = DockState::Paired;
                    }
                    Err(e) => {
                        log::error!("{e}");
                        *stage = Err(e);
                    }
                }
            }
            std::task::Poll::Ready(Err(e)) => {
                log::error!("could not be dialed: {e}");
                *stage = Err(format!("could not be dialed: {e}"));
            }
            std::task::Poll::Pending => {}
        }
    }

    /// What the row says: the program this pane was started for, once there is a link.
    ///
    /// A failure says what went wrong rather than what step it was in, because the failure is
    /// the more useful fact. While waiting there is nothing to say: the record is what the row
    /// carries, and it is drawn by [`render_waiting`].
    fn status<'a>(&'a self, stage: &'a Result<Stage, String>) -> &'a str {
        match stage {
            Err(message) => message,
            // The command is what this pane is for, so it is what the row says once the other
            // side has been reached — before that the row is the record's.
            Ok(_) => match self.state {
                DockState::Pairing { .. } => "",
                DockState::Paired => &self.program,
            },
        }
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

/// Poll a future once, from a caller that has no task to be polled in.
///
/// `render` is synchronous and is called from the draw, so there is nothing to await on and
/// no waker that will ever be called. That is exactly what this wait wants: the socket is
/// checked once per frame, and the frame is what gives the advert it drew a chance to reach
/// the other side. A no-op waker is not a shortcut here, it is the honest description —
/// nothing is lost by not being woken, because the next frame arrives anyway.
fn poll_once<F: std::future::Future + ?Sized>(
    mut future: std::pin::Pin<&mut F>,
) -> std::task::Poll<F::Output> {
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    future.as_mut().poll(&mut context)
}
