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
use wire::message::Message;
use wire::transport::{Connection, Transport};

use crate::windowing::{Compositable, PlatformWindowing, WindowingServer};
use crate::Error;

/// A window being served, as one boxed future.
///
/// Boxed because a window's servant starts the servants of the windows it opens: that shape is a
/// future whose child futures are of its own type, which only a box can express.
/// A window's task ended without an answer: its own panic, or its abort.
fn join_error(e: tokio::task::JoinError) -> Error {
    std::io::Error::other(format!("a window's task ended: {e}")).into()
}

type Proxy = futures::future::BoxFuture<'static, Result<(), Error>>;

/// How long one turn of the window session dispatches for at most.
///
/// A turn is not a wait: it is the session dispatching what has arrived, which is what delivers a
/// key to the program and what lets it commit its next picture.
const STEP: std::time::Duration = std::time::Duration::from_millis(4);

/// Run the session's program to its end.
///
/// `root_initializer` carries the placement for the window the program drew, and the program's
/// output with it: one-shot, because the render path waits for it exactly once, before its first
/// frame. A frame drawn before there is a root window would be a frame of nothing, and the pane
/// would have to be told that a blank is not a failure.
///
/// `transport` is the link the render path already bound and announced: it arrives once, and every
/// window's servant keeps a clone of the connection, so each streams at its own pace.
#[tracing::instrument(skip_all)]
pub async fn run(
    root_initializer: tokio::sync::oneshot::Sender<(CompositableWindow, io::PipeReader)>,
    options: crate::windowing::Options,
    codec: codecs::CodecId,
    debug: crate::DebugOptions,
    program: std::path::PathBuf,
    transport: tokio::sync::oneshot::Receiver<Box<dyn Transport>>,
) -> Result<(), Error> {
    let mut windowing = PlatformWindowing::new(options, codec, debug)?;
    windowing.spawn(&program)?;
    // The program's own words, for the screen that prints them. Taken once: a pipe has one reader.
    let stdout = windowing
        .take_stdout()
        .ok_or_else(|| io::Error::other("the program started without an output pipe"))?;

    let recv_root = windowing.take_root()?;

    // The link first: the dial is what starts the program, so a dial that never comes is a session
    // that never runs. The announcement has already reached the terminal — the render path drew it
    // before handing this over — so there is something for the other side to have dialed.
    let transport = transport
        .await
        .map_err(|_| io::Error::other("the render path is gone: no link to accept on"))?;
    let connection = if debug.no_accept {
        // Never answer the dial: the process stays on the pairing screen for as long as it
        // lives, which is the state the server's dial path is exercised against.
        std::future::pending().await
    } else {
        transport.accept().await?
    };

    // What the far side says, handed to every servant waiting on it.
    let (rebroadcaster, rebroadcaster_recvr) = tokio::sync::broadcast::channel(64);

    // The pinned future below takes what it uses, so the reader it needs is cloned first: the loop
    // still owns its own.
    let serve_connection = connection.clone();

    // The window the program draws, and then every window it opens: one future, because the root is
    // simply the first window this session serves. It waits for the root itself rather than the
    // session, so the turn beside it can find the root: a wait holding the session would make the
    // very turn that answers it impossible.
    let mut serve = std::pin::pin!(async move {
        let (placed, servant) =
            serve_window(recv_root.await?, serve_connection, rebroadcaster_recvr).await?;
        root_initializer.send((placed, stdout)).map_err(|_| {
            io::Error::other("the root window was not wanted: the render path is gone")
        })?;
        // Every window of the session, including the root's own: this is the session's whole
        // life. Spawned, not awaited here: each window captures and encodes in its own task, so
        // one window's encode cannot hold up another's frame or the session's own turns.
        //
        // Held in a set rather than a bare handle: this future is dropped when the session ends,
        // and a dropped set aborts its task. A detached servant would keep the link open and keep
        // streaming into a session that is over.
        let mut windows = tokio::task::JoinSet::new();
        windows.spawn(servant);
        match windows.join_next().await {
            Some(ended) => ended.map_err(join_error)?,
            None => Ok(()),
        }
    });

    loop {
        tokio::select! {
            // The session's windows: it is over when the last of them is.
            ended = &mut serve => return ended,
            // What the far side says, handed to every servant waiting on it — except the far side
            // saying something failed. That one is the session's ending, and the servants are what
            // it is about: handing it on would be asking them to fail one by one. It is returned
            // instead, so the draw loop can cross back to the screen it is read on.
            msg = connection.recv() => match msg? {
                Some(Message::Error(said)) => return Err(Error::Reported(said)),
                Some(msg) => drop(rebroadcaster.send(msg)),
                None => (),
            },
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
///
/// The announcement is sent first and its answer waited for, because the far side blanks the cells
/// carrying a surface it knows about: a picture placed before that registration exists is a picture
/// behind cells nobody could blank. The wait is safe here because the caller polls the link beside
/// this future, which is what carries the answer.
#[async_recursion::async_recursion]
#[tracing::instrument(skip_all)]
async fn serve_window(
    window: Box<dyn Compositable>,
    connection: Connection,
    mut rebroadcaster: tokio::sync::broadcast::Receiver<wire::message::Message>,
) -> io::Result<(CompositableWindow, Proxy)> {
    let (set_size, recv_set_size) = flume::unbounded();
    let (input, recv_input) = flume::unbounded();
    let (children, new_children) = flume::unbounded();
    let (popups, new_popups) = flume::unbounded();
    let (close, recv_close) = flume::unbounded();
    let rank = Arc::new(AtomicU32::new(0));
    let anchor = window.surface();

    let placed = CompositableWindow {
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
        render: Box::new(paint_surface(anchor)),
    };

    // Announced before anything is drawn: the far side places a picture on the cells carrying
    // its surface, and a frame for a surface it has never been told about is refused rather
    // than drawn. Said here because this is the first moment the link exists.
    connection
        .send(&wire::message::Message::OpenWindow {
            surface: window.surface(),
            codec: window.codec(),
        })
        .await?;
    // And the answer is waited for before a cell is drawn. The far side blanks the cells carrying
    // a surface it knows about, and it learns of one by reading this announcement — a different
    // stream from the pty those cells come out of, so nothing else orders the two.
    while !matches!(rebroadcaster.recv().await,
        Ok(Message::WindowCreated { surface }) if surface == anchor)
    {}

    let servant: Proxy = Box::pin(async move {
        // The windows this one's program opened, each already being served.
        let mut futures = tokio::task::JoinSet::new();
        // The box the pane placed this window in, in cells. What a frame reports, because that is
        // where the picture goes: the program's own committed size is a different number — a client
        // may draw at whatever size it chose — and placing by that is a picture that overflows the
        // box it was given.
        let mut box_cells = window.size_cells(winsize()?)?;
        // What this window's pictures are encoded with, built on the first one that has pixels:
        // an encoder is made for a size, and the size is only known once the window has drawn.
        let mut encoder: Option<Box<dyn codecs::Encoder>> = None;
        // How long the pane's last pictures took to be placed, as it last said. `None` until the
        // first word arrives, and a client with no clock sends unpaced — what it did before
        // there was one.
        let mut pace: Option<std::time::Duration> = None;
        // When the next picture may be taken. In the past until the pane says otherwise, which
        // is what unpaced means. A guard on the capture rather than a sleep: a sleep would hold
        // the pointer and the program still while it ran.
        let mut next = tokio::time::Instant::now();
        let served = loop {
            tokio::select! {
                // The pane's own clock, reported after it placed a picture. Every window of the
                // session reads the same channel; a pace another window names is not this one's.
                message = rebroadcaster.recv() => {
                    // A lag is messages the session shed while this window was busy sending:
                    // the next `Pace` is a fresh answer, so there is nothing to recover. Any
                    // other failure is the session's own news being gone, which `?` reports.
                    let message = match message {
                        Ok(message) => message,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(e) => return Err(e.into()),
                    };
                    if let Message::Pace { surface, micros } = message {
                        if surface == anchor {
                            pace = Some(std::time::Duration::from_micros(micros.into()));
                            // One report per picture the pane placed, which is the count the
                            // dock compares with the frames sent.
                            crate::stats::record_placed();
                        }
                    }
                },
                // The compositor asked for a size. The program decides whether it follows: its
                // next commit is the answer, and a window that refused still draws at its own.
                size = recv_set_size.recv_async() => {
                    let Ok(size) = size else { break Ok(()) };
                    box_cells = compositor::widgets::cells(size, winsize()?);
                    window.set_size(size)?;
                    // The box the picture goes in is the frame's other half: a window dragged or
                    // resized has moved without drawing, and this is what tells the pane so.
                    send_frame(
                        &connection,
                        window.surface(),
                        rank.load(Ordering::Relaxed),
                        box_cells,
                        None,
                    )
                    .await?;
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
                    let (placed, servant) =
                        serve_window(child, connection.clone(), rebroadcaster.resubscribe())
                            .await?;
                    // A failed send is the compositor having dropped this window's placement: the
                    // window is over, and so is the news of it.
                    (children.send(placed).ok(), futures.spawn(servant));
                }
                // A menu inside this window: placed the same way, announced on its own channel.
                popup = window.recv_new_popup() => {
                    let Some(popup) = popup else { break Ok(()) };
                    let (placed, servant) =
                        serve_window(popup, connection.clone(), rebroadcaster.resubscribe())
                            .await?;
                    (popups.send(placed).ok(), futures.spawn(servant));
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
                // learns it when the child's channel closes — so the news is only that a servant
                // finished.
                Some(ended) = futures.join_next() => ended.map_err(join_error)??,
                // The program committed a picture, and the pane's clock has come round. The
                // period is measured from this capture rather than from the send, so the encode
                // and the send are spent inside it: a picture that arrived early is the one that
                // waited, and one that took longer than the pace goes as soon as it is ready.
                // The wait is inside this arm's future rather than a guard on the branch: a
                // disabled branch is never polled, so nothing would wake the loop when the
                // cooldown ended. Here the timer is the wake, and the capture is what the loop
                // waits on once it has been spent — so a pointer still moves during it.
                captured = async {
                    tokio::time::sleep_until(next).await;
                    window.capture().await
                } => {
                    let started = tokio::time::Instant::now();
                    let Some(pixels) = captured? else { continue };
                    let bytes = encode(&*window, &mut encoder, &pixels)?;
                    let picture = bytes.is_some();
                    send_frame(
                        &connection,
                        window.surface(),
                        rank.load(Ordering::Relaxed),
                        box_cells,
                        bytes,
                    )
                    .await?;
                    if picture {
                        if let Some(pace) = pace {
                            next = started + pace;
                        }
                    }
                }
            }
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

/// Encode one picture for the link.
///
/// `None` is a codec holding the frame back: not a failure, and not pixels either — the picture
/// the pane already holds stays in the box.
#[tracing::instrument(skip(window, encoder, pixels))]
fn encode(
    window: &dyn Compositable,
    encoder: &mut Option<Box<dyn codecs::Encoder>>,
    pixels: &[u8],
) -> Result<Option<Vec<u8>>, Error> {
    let codec = window.codec();
    let size = window.size_px()?;
    let encoder = match encoder {
        Some(encoder) => encoder,
        None => encoder.insert(codec.to_encoder(size)?),
    };
    let encoded = encoder.encode(pixels, size.0, size.1, false)?;
    Ok((!encoded.is_empty()).then_some(encoded))
}

/// Put one window frame on the wire, and cut the profiler's timeline where it went.
///
/// `bytes` is the encoded picture, or `None` for a frame that only moves the picture — a window
/// resized, dragged, or raised without its program drawing anything. The rank is read by the
/// caller at the moment of sending, so a raise costs no message of its own.
#[tracing::instrument(skip(connection, bytes))]
async fn send_frame(
    connection: &Connection,
    surface: wire::message::SurfaceId,
    rank: u32,
    cells: (u16, u16),
    bytes: Option<Vec<u8>>,
) -> io::Result<()> {
    let payload = bytes.as_ref().map_or(0, |bytes| bytes.len() as u64);
    let message = Message::Frame {
        surface,
        position: wire::message::FramePosition {
            z: rank,
            cols: u32::from(cells.0),
            rows: u32::from(cells.1),
        },
        bytes,
    };
    connection.send(&message).await?;
    // A frame mark is a picture that actually went on the wire: an encode the codec held back
    // leaves a gap here, which is exactly what a missing send looks like on the timeline.
    crate::frame_mark();
    crate::stats::record(payload);
    Ok(())
}

/// The terminal's own report of itself, which is what a picture's size is measured in.
///
/// Under `test-windowing` there is no terminal to ask, so a fixed 80x24 stands in: the scripted
/// window never reads it anyway, but the frame path measures in cells and needs an answer.
#[tracing::instrument(skip_all)]
fn winsize() -> io::Result<rustix::termios::Winsize> {
    if cfg!(feature = "test-windowing") {
        return Ok(rustix::termios::Winsize {
            ws_row: 24,
            ws_col: 80,
            ws_xpixel: 800,
            ws_ypixel: 480,
        });
    }
    rustix::termios::tcgetwinsize(std::io::stdin()).map_err(io::Error::from)
}

/// How a window draws itself: its box filled with its own surface signature.
///
/// That signature is not decoration. The far side finds where a picture goes by scanning the pane
/// for these cells, so a window drawn without them is a window nothing can be placed on — the
/// pictures arrive and are dropped for want of an anchor.
#[tracing::instrument(skip_all)]
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
            .fg(ratatui::style::Color::Rgb(
                surface.fg.0,
                surface.fg.1,
                surface.fg.2,
            ))
            .bg(ratatui::style::Color::Rgb(
                surface.bg.0,
                surface.bg.1,
                surface.bg.2,
            ));
        for row in area.y..area.bottom() {
            for column in area.x..area.right() {
                if let Some(cell) = buf.cell_mut((column, row)) {
                    cell.set_char(surface.symbol).set_style(style);
                }
            }
        }
    }
}

#[cfg(all(test, feature = "test-windowing"))]
#[path = "../tests/unit/frame_backpressure.rs"]
mod frame_backpressure;
