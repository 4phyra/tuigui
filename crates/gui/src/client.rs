//! The run path: the session's program, its windows, and the pictures they draw.
//!
//! The other half of the client is the render path ([`crate::widget`]), which owns what a frame is
//! and draws it. This half owns the program: it starts the window session, waits for the window the
//! program drew, hands a `CompositableWindow` over it to whoever is drawing, and then serves every
//! channel that placement carries.
//!
//! # Why the channels are proxied here
//!
//! A `CompositableWindow` is what the compositor places and presses: a size ask goes down it, the
//! input it is typed with comes out, and the windows its program opens arrive through it. Those are
//! the backend's facts, which this half holds, so the placement is made here and its channels are
//! wired to the backend. The compositor never sees a `dyn Compositable`.
//!
//! # Who owns the link
//!
//! The render path binds a transport at startup — the pane's announcement is drawn from it — and
//! hands it over as a oneshot before this half starts. A transport has one owner, and that owner is
//! the side that accepts on it.

use std::io;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use compositor::CompositableWindow;
use futures::stream::FuturesUnordered;
use futures::stream::StreamExt as _;
use wire::transport::{Connection, Transport};

use crate::windowing::{Compositable, PlatformWindowing, WindowingServer};

/// A window being served, as one boxed future.
///
/// Boxed because a window's servant starts the servants of the windows it opens: that shape is a
/// future whose child futures are of its own type, which only a box can express.
type Proxy = futures::future::BoxFuture<'static, io::Result<()>>;

/// How long one turn of the window session dispatches for at most.
///
/// A turn is not a wait: it is the session dispatching what has arrived, which is what delivers a
/// key to the program and what lets it commit its next picture.
const STEP: std::time::Duration = std::time::Duration::from_millis(4);

/// How often a window's picture is taken when nothing else wakes its servant.
///
/// A program draws on its own clock — a page finishes loading, a cursor blinks, a video plays — and
/// none of that reaches this side as an event. The tick is what makes a commit visible at all: it is
/// the servant's own clock, not the pane's, because the pane redrawing is what a picture is *shown*
/// by and this is what it is *taken* for.
const FRAME: std::time::Duration = std::time::Duration::from_millis(16);

/// Run the session's program to its end.
///
/// `root_initializer` carries the placement for the window the program drew, and the program's
/// output with it: one-shot, because the render path waits for it exactly once, before its first
/// frame. A frame drawn before there is a root window would be a frame of nothing, and the pane
/// would have to be told that a blank is not a failure.
///
/// `transport` is the link the render path already bound and announced: it arrives once, and every
/// window's servant keeps a clone of the connection, so each streams at its own pace.
pub async fn run(
    root_initializer: tokio::sync::oneshot::Sender<(Arc<CompositableWindow>, io::PipeReader)>,
    options: crate::windowing::Options,
    codec: codecs::CodecId,
    program: std::path::PathBuf,
    transport: tokio::sync::oneshot::Receiver<Box<dyn Transport>>,
) -> io::Result<()> {
    // The link first: the dial is what starts the program, so a dial that never comes is a session
    // that never runs. The announcement has already reached the terminal — the render path drew it
    // before handing this over — so there is something for the other side to have dialed.
    let transport = transport
        .await
        .map_err(|_| io::Error::other("the render path is gone: no link to accept on"))?;
    let connection = transport.accept().await?;

    let mut windowing = PlatformWindowing::new(options, codec)?;
    windowing.spawn(&program)?;
    // The program's own words, for the screen that prints them. Taken once: a pipe has one reader.
    let stdout = windowing
        .take_stdout()
        .ok_or_else(|| io::Error::other("the program started without an output pipe"))?;

    // The window the program draws, and then every window it opens: one future, because the root is
    // simply the first window this session serves. It waits for the root itself rather than the
    // session, so the turn beside it can find the root: a wait holding the session would make the
    // very turn that answers it impossible.
    let wait = windowing.take_root()?;
    let mut serve = std::pin::pin!(async move {
        let root = wait.await?;
        let (placed, servant) = serve_window(root, connection)?;
        root_initializer.send((placed, stdout)).map_err(|_| {
            io::Error::other("the root window was not wanted: the render path is gone")
        })?;
        // Every window of the session, including the root's own: this is the session's whole life.
        servant.await
    });

    loop {
        tokio::select! {
            // The session's windows: it is over when the last of them is.
            ended = &mut serve => return ended,
            // A turn of the session: what dispatches a client's requests and what lets it commit its
            // next picture. Raced against the windows rather than run on a thread, because a session
            // nobody is turning is a program that never draws.
            _ = tokio::time::sleep(STEP) => {
                // A session whose program is over has nothing left to dispatch, and the servants
                // are what outlive it.
                if !windowing.is_running() {
                    return Ok(());
                }
                windowing.advance(STEP);
            }
        }
    }
}

/// Wire one window's channels, and hand back its placement and the servant that answers them.
///
/// Two ends of every channel are made here: the side the compositor writes to, which goes into the
/// placement, and the side this half answers on, which stays inside the servant. Nothing is dropped,
/// because a dropped sender is what a window being over looks like — and this window is not over.
fn serve_window(
    window: Box<dyn Compositable>,
    connection: Connection,
) -> io::Result<(Arc<CompositableWindow>, Proxy)> {
    let (set_size, recv_set_size) = flume::unbounded();
    let (input, recv_input) = flume::unbounded();
    let (children, new_children) = flume::unbounded();
    let (popups, new_popups) = flume::unbounded();
    let (close, recv_close) = flume::unbounded();
    let rank = Arc::new(AtomicU32::new(0));

    let placed = Arc::new(CompositableWindow {
        set_size,
        size: Arc::new(Mutex::new(window.size_px()?)),
        title: Arc::new(Mutex::new(window.title()?)),
        is_popup: window.is_popup(),
        offset_px: window.offset_px()?,
        input,
        new_children,
        new_popups,
        close,
        rank: rank.clone(),
        render: Box::new(paint_surface(window.surface())),
    });

    // The servant's own handle on the window it serves. The compositor holds a `Weak`, so without
    // this the window would die as soon as the placement was made: its channel senders would drop
    // with it, every `recv` below would answer `Err`, and a window that is only four frames old
    // would be reported over. The servant is the right owner because its lifetime *is* the
    // window's.
    let alive = placed.clone();

    let servant = Box::pin(async move {
        let _alive = alive;
        // Announced before anything is drawn: the far side places a picture on the cells carrying
        // its surface, and a frame for a surface it has never been told about is refused rather
        // than drawn. Said here because this is the first moment the link exists.
        connection
            .send(&wire::message::Message::OpenWindow {
                surface: window.surface(),
                codec: window.codec(),
            })
            .await?;
        // The windows this one's program opened, each already being served.
        let mut futures = FuturesUnordered::new();
        // The box the pane placed this window in, in cells. What a frame reports, because that is
        // where the picture goes: the program's own committed size is a different number — a client
        // may draw at whatever size it chose — and placing by that is a picture that overflows the
        // box it was given.
        let mut box_cells = window.size_cells(winsize()?)?;
        let served = loop {
            tokio::select! {
                // The compositor asked for a size. The program decides whether it follows: its
                // next commit is the answer, and a window that refused still draws at its own.
                size = recv_set_size.recv_async() => {
                    let Ok(size) = size else { break Ok(()) };
                    box_cells = compositor::widgets::cells(size, winsize()?);
                    window.set_size(size)?
                }
                // The compositor sent input: a key with its modifiers, or a press at a pixel.
                event = recv_input.recv_async() => {
                    let Ok(event) = event else { break Ok(()) };
                    match event {
                        termina::Event::Key(key) => window.key_input(key)?,
                        termina::Event::Mouse(mouse) => window.pointer_input(mouse)?,
                        _ => {}
                    }
                }
                // The program opened another window. It is placed here rather than by the
                // compositor, because placing means wiring channels and only this half has them.
                // `None` is that program being over, which ends this servant on its last frame.
                child = window.recv_new_child() => {
                    let Some(child) = child else { break Ok(()) };
                    let (placed, servant) = serve_window(child, connection.clone())?;
                    // A failed send is the compositor having dropped this window's placement: the
                    // window is over, and so is the news of it.
                    (children.send(placed).ok(), futures.push(servant));
                }
                // A menu inside this window: placed the same way, announced on its own channel.
                popup = window.recv_new_popup() => {
                    let Some(popup) = popup else { break Ok(()) };
                    let (placed, servant) = serve_window(popup, connection.clone())?;
                    (popups.send(placed).ok(), futures.push(servant));
                }
                // The compositor asked this window to close. A request: the program is told and
                // decides, and the window it chose to keep is one this keeps serving. A receive
                // that failed is that ask's sender being gone, which is the pane being over.
                asked = recv_close.recv_async() => {
                    asked.map_err(|_| {
                        std::io::Error::other("the close request was dropped unread")
                    })?;
                    window.terminate()?;
                }
                // One of the windows this one opened is over. Nothing to place — the compositor
                // learns that from its own `Weak` — so the news is only that a servant finished.
                Some(ended) = futures.next() => ended?,
                // The program committed a picture nobody typed to ask for. A window draws on its own
                // clock and not on this side's events, so a frame has to be taken whether or not
                // anything arrived: without this arm a window is only ever captured in answer to
                // input, and a program that draws by itself is never seen at all.
                _ = tokio::time::sleep(FRAME) => {}
            }

            // A picture leaves here: once per turn, so the encode and the send happen in the same
            // breath as the event that made them worth taking. The rank is read at that moment, so
            // a raise costs no message of its own.
            window
                .frame(&connection, rank.load(Ordering::Relaxed), box_cells)
                .await?;
        };

        // The window is over, so the far side is told to forget it: its last picture would
        // otherwise stay where it was for the rest of the session.
        connection
            .send(&wire::message::Message::CloseWindow {
                surface: window.surface(),
            })
            .await?;
        served
    });
    Ok((placed, servant))
}

/// The terminal's own report of itself, which is what a picture's size is measured in.
fn winsize() -> io::Result<rustix::termios::Winsize> {
    rustix::termios::tcgetwinsize(std::io::stdin()).map_err(io::Error::from)
}

/// How a window draws itself: its box filled with its own surface signature.
///
/// That signature is not decoration. The far side finds where a picture goes by scanning the pane
/// for these cells, so a window drawn without them is a window nothing can be placed on — the
/// pictures arrive and are dropped for want of an anchor.
fn paint_surface(
    surface: wire::message::SurfaceId,
) -> impl Fn(
    ratatui::layout::Rect,
    &mut ratatui::buffer::Buffer,
    &mut compositor::widgets::WindowState,
) + Send
       + Sync {
    move |area, buf, _state| {
        let style = ratatui::style::Style::new()
            .fg(ratatui::style::Color::Rgb(surface.fg.0, surface.fg.1, surface.fg.2))
            .bg(ratatui::style::Color::Rgb(surface.bg.0, surface.bg.1, surface.bg.2));
        for row in area.y..area.bottom() {
            for column in area.x..area.right() {
                if let Some(cell) = buf.cell_mut((column, row)) {
                    cell.set_char(surface.symbol).set_style(style);
                }
            }
        }
    }
}
