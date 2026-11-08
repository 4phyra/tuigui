//! The pane drawn from a session with nothing behind it.
//!
//! A session with no program behind it. Every window draws itself as a pane
//! showing a capybara, and `C` or `W` spawns another under the one the key
//! reached. The compositor cannot tell a dummy window from a real one, which is
//! the whole point: what this exercises is the stacking, the chrome, the dragging
//! and the resizing, with no wayland, no codec and no link in the picture at all.
//!
//! The shape is the wayland session's, deliberately: a server that owns its
//! windows, and a handle per window that the compositor keeps. The server mints a
//! fresh handle on every ask, so it holds no second copy of what it already owns;
//! a handle holds the window weakly, so a window that is over is one that fails to
//! upgrade rather than something to prune.
//!
//! `q` or `Esc` ends it. The set of images is `CAPYBARA_IMG_DIR`, as the demo
//! always was.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock, Weak};

use compositor::{Compositable, Compositor, Error, WindowingServer};
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use ratatui::crossterm::execute;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _tty = Raw::install()?;
    let mut terminal = ratatui::try_init()?;
    // Mouse reporting is a mode of the terminal device rather than of the drawing, and it is
    // off until something asks for it: without this the terminal sends no mouse events at
    // all, so the chrome is pressable in principle and unreachable in practice.
    execute!(std::io::stdout(), EnableMouseCapture)?;
    let mut compositor = Compositor::new(DummySession::default());
    // How long a quiet step is paused, which is what keeps an idle loop cheap: a
    // frame with nothing to do would otherwise be drawn as fast as the terminal
    // takes it.
    let quiet = std::time::Duration::from_millis(16);

    loop {
        let winsize = rustix::termios::tcgetwinsize(std::io::stdin())?;
        for event in read_events()? {
            if is_quit(&event) {
                // Before the screen is restored: a terminal left reporting mouse events is
                // one that keeps printing them into the shell that comes back.
                execute!(std::io::stdout(), DisableMouseCapture)?;
                ratatui::restore();
                return Ok(());
            }
            compositor.input(&event, winsize)?;
        }

        compositor
            .frame(quiet, winsize)
            .expect("a frame could not be taken");
        terminal.draw(|frame| {
            let area = frame.area();
            ratatui::widgets::Widget::render(&mut compositor, area, frame.buffer_mut());
        })?;
    }
}

/// Whether the event asks the example to stop.
fn is_quit(event: &termina::Event) -> bool {
    use termina::event::KeyCode;
    let termina::Event::Key(key) = event else {
        return false;
    };
    matches!(key.code, KeyCode::Char('q') | KeyCode::Escape)
}

/// Every event this terminal has waiting, parsed.
///
/// A draw loop cannot sit in a blocking read: a frame that waited for a key is a
/// frame the pane never showed. Reading is non-blocking, so no events is the
/// normal answer, and the caller draws.
fn read_events() -> std::io::Result<Vec<termina::Event>> {
    use std::io::Read as _;
    let mut bytes = [0u8; 256];
    let mut stdin = std::io::stdin();
    let mut parser = termina::Parser::default();
    let mut out = Vec::new();
    // One read per call at most: more would be a loop that waits for input to stop
    // arriving, and a key held down never does.
    let read = std::io::BufReader::new(&mut stdin).read(&mut bytes)?;
    parser.parse(&bytes[..read], true);
    while let Some(event) = parser.pop() {
        out.push(event);
    }
    Ok(out)
}

/// The dummy session: the windows it owns, and the size it last measured itself at.
#[derive(Default)]
struct DummySession {
    /// The session's root window, minted at the first ask.
    ///
    /// `Arc` because the session owns its windows and hands out weak handles to them; a handle
    /// that kept the `Arc` would be a second owner, and a window would then outlive the pane
    /// that is the only thing showing it.
    root: RwLock<Option<Arc<DummyWindow>>>,
    /// The pane, in pixels, as the compositor last reported it.
    ///
    /// What the root is minted at: a demo window is as big as the pane it was started into,
    /// rather than at a number invented for it.
    pane: (u32, u32),
}

/// One window of the dummy session: its state, the windows it opened after it, and the picture
/// it draws.
struct DummyWindow {
    /// The windows this one's program opened after it, owned: the tree is what keeps them alive.
    children: RwLock<Vec<Arc<DummyWindow>>>,
    /// The capybara this window shows, as cells, and the width it was reduced for: cells made for
    /// a narrower window fill only its left, so the width is kept with them and they are made
    /// again when the window is drawn wider. The image's name is kept beside them, which is what
    /// the bar shows.
    capy: RwLock<(u16, Option<(String, Cells)>)>,
    /// The size the compositor last asked for, in pixels.
    ///
    /// A window with no program has no picture to measure, so the size it was given is the only
    /// size it has — which is what it answers with, rather than measuring nothing.
    size: Mutex<(u32, u32)>,
    /// Set by `terminate`. Nothing runs behind a dummy window, so there is no program to ask to
    /// close: being asked is the window being over, and its parent stops reporting it.
    gone: AtomicBool,
}

impl DummyWindow {
    fn new(size: (u32, u32)) -> Self {
        Self {
            children: RwLock::new(Vec::new()),
            capy: RwLock::new((0, None)),
            size: Mutex::new(size),
            gone: AtomicBool::new(false),
        }
    }
}

/// One window of the session, as the compositor holds it.
///
/// Minted afresh on every ask, and holding the window weakly: the session is what owns its
/// windows, so a handle whose window is gone is one that fails to upgrade, which is the window
/// being over rather than a failure to report.
struct DummyHandle {
    inner: Weak<DummyWindow>,
    /// How many of this window's children have already been handed over.
    ///
    /// A count rather than a queue: the children are already a list on the window, and a second
    /// list of the same windows is a second truth. Reading is what advances this, which is why
    /// the trait asks for `&mut`.
    reported: usize,
}

impl DummyHandle {
    fn window(&self) -> Result<Arc<DummyWindow>, Error> {
        self.inner.upgrade().ok_or(Error::Gone)
    }
}

impl WindowingServer for DummySession {
    /// The root window, wrapped afresh on every call.
    ///
    /// Not kept here: the session is what holds its windows, so asking is a lookup rather than a
    /// slot this side has to keep in step with it.
    fn root_window(&mut self) -> Option<Box<dyn Compositable>> {
        let mut root = self.root.write().unwrap();
        if root.is_none() {
            *root = Some(Arc::new(DummyWindow::new(self.pane)));
        }
        Some(Box::new(DummyHandle {
            inner: Arc::downgrade(root.as_ref()?),
            reported: 0,
        }))
    }

    fn is_ready(&self) -> bool {
        true
    }

    /// The session is the pane: measuring it is what the root is minted at.
    fn set_size(&mut self, size: (u32, u32)) {
        self.pane = size;
    }
}

impl Compositable for DummyHandle {
    /// Whether the session still holds the window this stands for.
    ///
    /// A window asked to close is one whose program is over, which is the same answer as one the
    /// session has let go: either way the compositor has nothing left to draw.
    fn is_alive(&self) -> bool {
        self.inner.strong_count() > 0
            && self
                .inner
                .upgrade()
                .is_some_and(|window| !window.gone.load(Ordering::Acquire))
    }

    /// A key is what grows the tree: `C` or `W` spawns a window this one opened after it.
    fn key_input(&self, event: ui_events::keyboard::KeyboardEvent) -> Result<(), Error> {
        use ui_events::keyboard::Key;
        if !event.state.is_down() {
            return Ok(());
        }
        let Key::Character(typed) = &event.key else {
            return Ok(());
        };
        if !matches!(typed.as_str(), "c" | "C" | "w" | "W") {
            return Ok(());
        }
        let window = self.window()?;
        // As big as the window that opened it: a demo has no program to ask, and a child the
        // size of its parent is the one answer that needs no invented number.
        let size = *window.size.lock().unwrap_or_else(|e| e.into_inner());
        window
            .children
            .write()
            .unwrap()
            .push(Arc::new(DummyWindow::new(size)));
        Ok(())
    }

    /// A dummy window has no program to send a pointer event to: the keys are the whole of what
    /// it understands.
    fn pointer_input(&self, _event: ui_events::pointer::PointerEvent) -> Result<(), Error> {
        Ok(())
    }

    /// A dummy window never draws a picture: nothing is running behind it to take, so there is
    /// no frame to send anywhere.
    fn frame(&self, _rank: u32, _cells: (u16, u16)) -> Result<(), Error> {
        Ok(())
    }

    /// The size the compositor last asked for, which is the whole of what this window is.
    fn size_px(&self) -> Result<(u32, u32), Error> {
        let window = self.window()?;
        let size = *window.size.lock().unwrap_or_else(|e| e.into_inner());
        Ok(size)
    }

    /// A dummy window is placed by the compositor like any other window, never by a program.
    fn is_popup(&self) -> bool {
        false
    }

    /// A dummy window has no program to configure, so being asked is the size being recorded.
    fn set_size(&self, size: (u32, u32)) -> Result<(), Error> {
        let window = self.window()?;
        *window.size.lock().unwrap_or_else(|e| e.into_inner()) = size;
        Ok(())
    }

    /// The windows this one opened since the last read: the ones past the count already handed
    /// over, so a window is announced once and never re-placed.
    fn get_new_children(&mut self) -> Vec<Box<dyn Compositable>> {
        let Some(window) = self.inner.upgrade() else {
            return Vec::new();
        };
        let mut children = window.children.write().unwrap();
        // A child asked to close is dropped here rather than merely filtered: it is this list
        // that keeps it alive, and a window nobody can see must not stay alive.
        children.retain(|child| !child.gone.load(Ordering::Acquire));
        let fresh = children
            .iter()
            .skip(self.reported)
            .map(|child| {
                Box::new(DummyHandle {
                    inner: Arc::downgrade(child),
                    reported: 0,
                }) as Box<dyn Compositable>
            })
            .collect();
        self.reported = children.len();
        fresh
    }

    /// A dummy window is named after the image it draws, which is the only thing there is to
    /// call it.
    fn title(&self) -> Result<String, Error> {
        let window = self.window()?;
        let name = window
            .capy
            .read()
            .unwrap()
            .1
            .as_ref()
            .map(|(name, _)| name.clone());
        Ok(name.unwrap_or_else(|| "capybara".to_string()))
    }

    /// Nothing runs behind a dummy window, so there is no program to ask: being asked to close
    /// is the window being over, and its parent stops reporting it.
    fn terminate(&self) -> Result<(), Error> {
        self.window()?.gone.store(true, Ordering::Release);
        Ok(())
    }
}

/// One capybara of the demo set, reduced to cells, as the window's background.
impl ratatui::widgets::StatefulWidgetRef for DummyHandle {
    type State = compositor::widgets::WindowState;

    fn render_ref(&self, area: Rect, buf: &mut Buffer, _state: &mut Self::State) {
        let Some(window) = self.inner.upgrade() else {
            return;
        };
        buf.set_style(area, Style::new().bg(Color::Rgb(0x00, 0x00, 0x00)));

        let mut capy = window.capy.write().unwrap();
        if capy.0 != area.width {
            *capy = (area.width, cells(area.width, area.height));
        }
        let Some((_, cells)) = &capy.1 else {
            return;
        };
        for (index, (glyph, colour)) in cells.glyphs.iter().enumerate() {
            let row = (index as u32 / cells.width.max(1)) as u16;
            if row >= area.height {
                break;
            }
            let place = (area.x + (index as u32 % cells.width.max(1)) as u16, area.y + row);
            if let Some(slot) = buf.cell_mut(place) {
                slot.set_char(*glyph);
                let [r, g, b] = *colour;
                slot.set_fg(Color::Rgb(r, g, b));
            }
        }
    }
}

/// The demo image as cells: one glyph and one colour per cell, `width` of them to a row.
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
/// Raw mode is what makes a key arrive as a byte rather than as a line, and it is
/// the example's own business because the example owns the terminal it draws on.
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
