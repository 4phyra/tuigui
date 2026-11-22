//! The pane drawn from a session with nothing behind it.
//!
//! A session with no program behind it. Every window paints a capybara, and `C` or `W` opens
//! another under the one the key reached. The compositor cannot tell a dummy window from a real
//! one, which is the whole point: what this exercises is the stacking, the chrome, the dragging
//! and the resizing, with no wayland, no codec and no link in the picture at all.
//!
//! The shape is the real thing's, shrunk to one process: a window arrives at the compositor as a
//! bundle of channels (`CompositableWindow`), and the half that made it keeps answering its end —
//! the input it is typed on, the size it is asked for, the children it opens. A servant task per
//! window plays that half; a `C` or `W` key mints another window through the same path a real
//! program's window takes.
//!
//! `q` or `Esc` ends it. The set of images is `CAPYBARA_IMG_DIR`, as the demo always was.

use std::io::Read;
use std::sync::atomic::AtomicU32;
use std::sync::{Arc, Mutex};

use compositor::{CompositableWindow, Compositor};
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use ratatui::crossterm::execute;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::Widget;
use termina::event::{KeyCode, KeyEventKind};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _tty = Raw::install()?;
    let mut terminal = ratatui::try_init()?;
    // Mouse reporting is a mode of the terminal device rather than of the drawing, and it is
    // off until something asks for it: without this the terminal sends no mouse events at all, so
    // the chrome is pressable in principle and unreachable in practice.
    execute!(std::io::stdout(), EnableMouseCapture)?;

    let winsize = rustix::termios::tcgetwinsize(std::io::stdin())?;
    let root = spawn_window(pane_pixels(&winsize));
    let mut compositor = Compositor::new(root);

    // A thread that does nothing but read stdin and forward the bytes: the async side reads from
    // the channel, so a keystroke and a window change are two answers to the same `select!`.
    let (bytes_tx, bytes_rx) = flume::unbounded::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut bytes = [0u8; 256];
        loop {
            match stdin.read(&mut bytes) {
                Ok(0) => break,
                Ok(read) => {
                    if bytes_tx.send(bytes[..read].to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    let mut parser = termina::Parser::default();

    terminal.draw(|frame| {
        let area = frame.area();
        Widget::render(&mut compositor, area, frame.buffer_mut());
    })?;

    loop {
        let winsize = rustix::termios::tcgetwinsize(std::io::stdin())?;
        let quit = tokio::select! {
            bytes = bytes_rx.recv_async() => {
                match bytes {
                    Ok(bytes) => {
                        parser.parse(&bytes, true);
                        let mut quit = false;
                        while let Some(event) = parser.pop() {
                            if is_quit(&event) {
                                quit = true;
                                break;
                            }
                            compositor.handle_event(event, winsize)?;
                        }
                        quit
                    }
                    // The reader thread ended: stdin is closed, and so is the demo.
                    Err(_) => true,
                }
            }
            result = compositor.step(winsize) => {
                // A failed turn is a window that could not be placed, and the next one is worth
                // as much as this one: reporting and carrying on keeps the demo alive.
                if let Err(error) = result {
                    log::warn!("a window could not be placed: {error}");
                }
                false
            }
        };
        if quit {
            break;
        }
        terminal.draw(|frame| {
            let area = frame.area();
            Widget::render(&mut compositor, area, frame.buffer_mut());
        })?;
    }
    // Before the screen is restored: a terminal left reporting mouse events is one that keeps
    // printing them into the shell that comes back.
    execute!(std::io::stdout(), DisableMouseCapture)?;
    ratatui::restore();
    Ok(())
}

/// Whether the event asks the example to stop.
fn is_quit(event: &termina::Event) -> bool {
    let termina::Event::Key(key) = event else {
        return false;
    };
    key.kind == KeyEventKind::Press && matches!(key.code, KeyCode::Char('q') | KeyCode::Escape)
}

/// The pane in pixels, for a terminal that reports them; one pixel a cell for one that does not,
/// which is the same fallback the real client assumes.
fn pane_pixels(winsize: &rustix::termios::Winsize) -> (u32, u32) {
    match (winsize.ws_xpixel, winsize.ws_ypixel) {
        (0, _) | (_, 0) => (u32::from(winsize.ws_col), u32::from(winsize.ws_row)),
        (width, height) => (u32::from(width), u32::from(height)),
    }
}

/// A window of the demo, wired to the channels the compositor expects.
///
/// The channels go in both directions and each half lives on its own side: the compositor holds
/// the senders it writes to and the receivers it waits on, this half holds the rest. The two
/// ends of each channel are made here, once, so a window is never half-wired.
fn spawn_window(size: (u32, u32)) -> CompositableWindow {
    let (set_size, set_size_rx) = flume::unbounded();
    let (input, input_rx) = flume::unbounded();
    let (children, children_rx) = flume::unbounded();
    let (popups, popups_rx) = flume::unbounded();
    let (close, close_rx) = flume::unbounded();

    let committed = Arc::new(Mutex::new(size));
    let title = Arc::new(Mutex::new(String::from("capybara")));
    // The image this window paints, reduced to cells on first draw and kept per width: cells made
    // for a narrower box fill only the left of a wider one, so they are made again on a resize.
    let painted: Arc<Mutex<(u16, Option<(String, Cells)>)>> = Arc::new(Mutex::new((0, None)));

    let render_painted = Arc::clone(&painted);
    let render_title = Arc::clone(&title);
    let render = Box::new(
        move |area: Rect, buf: &mut Buffer, _state: &mut compositor::widgets::WindowState| {
            buf.set_style(area, Style::new().bg(Color::Rgb(0x00, 0x00, 0x00)));

            let mut painted = render_painted.lock().unwrap_or_else(|e| e.into_inner());
            if painted.0 != area.width {
                *painted = (area.width, cells(area.width, area.height));
                if let Some((name, _)) = &painted.1 {
                    *render_title.lock().unwrap_or_else(|e| e.into_inner()) = name.clone();
                }
            }
            let Some((_, cells)) = &painted.1 else {
                return;
            };
            for (index, (glyph, colour)) in cells.glyphs.iter().enumerate() {
                let row = (index as u32 / cells.width.max(1)) as u16;
                if row >= area.height {
                    break;
                }
                let place = (
                    area.x + (index as u32 % cells.width.max(1)) as u16,
                    area.y + row,
                );
                if let Some(slot) = buf.cell_mut(place) {
                    slot.set_char(*glyph);
                    let [r, g, b] = *colour;
                    slot.set_fg(Color::Rgb(r, g, b));
                }
            }
        },
    );

    tokio::spawn(serve_window(
        Arc::clone(&committed),
        set_size_rx,
        input_rx,
        close_rx,
        children,
        popups,
    ));

    CompositableWindow {
        set_size,
        size: committed,
        title,
        is_popup: false,
        offset_px: None,
        input,
        new_children: children_rx,
        new_popups: popups_rx,
        close,
        rank: Arc::new(AtomicU32::new(0)),
        render,
    }
}

/// The half of a window this demo keeps: answer the compositor until the window is over.
///
/// The channels held here are what tell the compositor the window is over: when this task ends,
/// they drop, and the wait the compositor has on them answers with an error, which is the
/// window being taken out of the stack. `popups` is held only so it lasts as long as the window
/// does — the demo opens none.
async fn serve_window(
    committed: Arc<Mutex<(u32, u32)>>,
    set_size: flume::Receiver<(u32, u32)>,
    input: flume::Receiver<termina::Event>,
    close: flume::Receiver<()>,
    children: flume::Sender<CompositableWindow>,
    _popups: flume::Sender<CompositableWindow>,
) {
    loop {
        tokio::select! {
            size = set_size.recv_async() => {
                let Ok(size) = size else { break };
                *committed.lock().unwrap_or_else(|e| e.into_inner()) = size;
            }
            event = input.recv_async() => {
                let Ok(event) = event else { break };
                let termina::Event::Key(key) = event else { continue };
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                let KeyCode::Char(c) = key.code else { continue };
                if !matches!(c, 'c' | 'C' | 'w' | 'W') {
                    continue;
                }
                // As big as the window that opened it: a demo has no program to ask, and a child the
                // size of its parent is the one answer that needs no invented number.
                let size = *committed.lock().unwrap_or_else(|e| e.into_inner());
                let _ = children.send(spawn_window(size));
            }
            closed = close.recv_async() => {
                let _ = closed;
                break;
            }
        }
    }
}

/// One capybara of the demo set, reduced to cells, as the window's picture.
struct Cells {
    /// How many cells a row holds.
    width: u32,
    /// The whole picture, row after row: a glyph and the colour it is drawn in.
    glyphs: Vec<(char, [u8; 3])>,
}

/// The shades a cell is drawn in, darkest first. A cell's brightness picks one, which is what
/// turns a photograph into something a terminal can show.
const SHADES: [char; 10] = [' ', '·', ':', '-', '=', '+', '*', '#', '%', '@'];

/// One image from the demo set, reduced to cells `width` by `height`, with the name of the file
/// it came from.
///
/// The set is walked from a random start, and one image after another is tried until one reads:
/// a bad file is skipped, and only a set where none works leaves the window its black
/// background. Each failure is reported, since a skipped image is otherwise invisible.
///
/// Reduced rather than halftoned: a halftone would need a library, and what the dummy window is
/// for is the compositor around it. Each cell averages the pixels it covers, and the average's
/// brightness picks a glyph from [`SHADES`] while its colour is that average.
fn cells(width: u16, height: u16) -> Option<(String, Cells)> {
    let dir = std::env::var_os("CAPYBARA_IMG_DIR")?;
    let files: Vec<_> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .collect();
    if files.is_empty() {
        return None;
    }

    use std::hash::{BuildHasher, Hash, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    files.hash(&mut hasher);
    let start = (hasher.finish() % files.len() as u64) as usize;

    (0..files.len()).find_map(|offset| {
        let file = &files[(start + offset) % files.len()];
        let bytes = match std::fs::read(file) {
            Ok(bytes) => bytes,
            Err(e) => {
                log::warn!("could not read {}: {e}", file.display());
                return None;
            }
        };
        match image::load_from_memory(&bytes) {
            Ok(picture) => {
                let name = file
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| file.display().to_string());
                let reduced = reduce(&picture, width.max(1), height.max(1));
                Some((name, reduced))
            }
            Err(e) => {
                log::warn!("could not read {}: {e}", file.display());
                None
            }
        }
    })
}

/// The picture as `width` by `height` cells, each one the average of the pixels it covers.
fn reduce(picture: &image::DynamicImage, width: u16, height: u16) -> Cells {
    let (across, down) = (u32::from(width), u32::from(height));
    let small = picture
        .resize_exact(across, down, image::imageops::FilterType::Triangle)
        .to_rgb8();
    let glyphs = small
        .pixels()
        .map(|pixel| {
            let [r, g, b] = pixel.0;
            // The eye weighs the channels unequally, and a plain mean would show a red field as
            // bright as a green one. The weights are the usual ones.
            let level = (0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b)) / 255.0;
            let index = ((level * SHADES.len() as f32) as usize).min(SHADES.len() - 1);
            (SHADES[index], [r, g, b])
        })
        .collect();
    Cells {
        width: across,
        glyphs,
    }
}

/// Holds this terminal in raw mode, and restores the old settings when dropped.
///
/// Raw mode is what makes a key arrive as a byte rather than as a line, and it is the
/// example's own business because the example owns the terminal it draws on.
struct Raw {
    original: rustix::termios::Termios,
}

impl Raw {
    fn install() -> std::io::Result<Self> {
        let stdin = std::io::stdin();
        let original = rustix::termios::tcgetattr(&stdin).map_err(std::io::Error::from)?;
        let mut raw = original.clone();
        raw.make_raw();
        // `make_raw` clears `ISIG`, which turns `^C` from a signal into a byte. It
        // is put back so `^C` still stops this: an example with no other way out
        // is worse than one that cannot see the key.
        raw.local_modes.insert(rustix::termios::LocalModes::ISIG);
        rustix::termios::tcsetattr(&stdin, rustix::termios::OptionalActions::Now, &raw)
            .map_err(std::io::Error::from)?;
        Ok(Self { original })
    }
}

impl Drop for Raw {
    fn drop(&mut self) {
        let _ = rustix::termios::tcsetattr(
            &std::io::stdin(),
            rustix::termios::OptionalActions::Now,
            &self.original,
        );
    }
}
