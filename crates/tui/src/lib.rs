//! `tui` — run a command in a pty we own and forward its bytes to this terminal.
//!
//! ```text
//!   this terminal  <── Pty ──▶  the command's pty  ──▶  the command
//!                                    │
//!                                    └── a view of the output ──▶ Server: dials a client
//! ```
//!
//! The command is a normal terminal program: it is given a pty, and everything it
//! reads and writes is forwarded unchanged. A client that wants to draw into this
//! pane cannot be reached before it says how, so it announces a channel of its own
//! in that output, and the server — which is already watching every byte — dials
//! it.
//!
//! The pty hands out a **view** of its output rather than being given a place to
//! send it: the producer does not know about its consumers, so adding one is not a
//! change to the pty.
//!
//! Both channels write the screen through one [`SharedStdout`], which is what stops a
//! frame from landing in the middle of the command's output.

mod bytes;
mod error;
mod proxy;
mod pty;
mod raw;
mod server;
mod session;

use std::io::{self, Read, Write};

use pty::Pty;
use server::Server;

pub use bytes::Bytes;
pub use error::{Error, Result};
pub use proxy::Scanner as CellScanner;
pub use server::Server as ImageServer;

// A session, as the command line describes it. `run` is where it starts.
//
// `command` is trailing and required: `tui` with nothing to run has no meaning,
// and clap rejects it rather than starting a server around an empty command.
// `allow_hyphen_values` is what lets the wrapped command's own flags (`-l`,
// `--color`) through instead of being read as `tui`'s.
#[derive(Debug, Clone, clap::Parser)]
#[command(
    name = "tui",
    about = "Run a command in a pty and forward it to this terminal"
)]
pub struct Tui {
    /// The command to run, with its arguments.
    #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
    pub command: Vec<String>,
    /// The diagnostic switches, shared with the client.
    #[command(flatten)]
    pub debug: DebugOptions,
}

/// The switches that exist to diagnose a session, not to use one.
///
/// Every one of these makes the session worse in some way, and every one exists
/// because a failure in this pipeline is invisible: an image that is never sent, a
/// placement nothing resolves against, a medium the terminal cannot read, and a
/// blank pane all look the same from outside. Each switch removes one layer, so
/// what remains on screen is attributable to the layers still in place.
///
/// They are all named `--dbg-`, and they are all the caller's to pass: a server
/// does not decide to diagnose itself.
#[derive(Debug, Clone, Default, clap::Args)]
// A group id of its own: `tuigui` flattens this beside `gui`'s own DebugOptions,
// and clap names a flattened group after the struct — two of them called the
// same thing is a command that refuses to build at all.
#[group(id = "tui-debug")]
pub struct DebugOptions {
    /// Draw a fixed blue image instead of the frames that arrive.
    ///
    /// Removes the wire codec and the decode, so what is on screen is attributable
    /// to the encode, the transmit and the placement. Blue means those are sound and
    /// the fault is upstream; a blank pane means it is here.
    #[arg(long = "dbg-blue-pixel")]
    pub blue_pixel: bool,
    /// Place the image for real instead of creating a virtual placement.
    ///
    /// A virtual placement (`U=1`) has no position of its own, so it draws nothing
    /// until placeholder cells resolve against it — correct, and silent when they do
    /// not. A real placement draws at the cursor and needs nothing from the client's
    /// text, which is what separates "no image" from "no cells".
    #[arg(long = "dbg-no-vplacement")]
    pub no_vplacement: bool,
    /// Send the pixels inside the escape instead of through shared memory.
    ///
    /// The shared memory medium (`t=s`) names an object the terminal maps, which is
    /// the fast path and the reason this pipeline is affordable. The inline path
    /// puts a base64 PNG in the escape, so it is slower and larger — and it cannot
    /// fail on a terminal that will not read the object, which is what makes it the
    /// control for "the image arrived but nothing was drawn".
    #[arg(long = "dbg-no-shm")]
    pub no_shm: bool,
    /// Mark the cells the surface mask fires on, instead of blanking them.
    ///
    /// The mask exists to blank the cells that carry a window's signature, so the
    /// picture drawn behind them is visible. When nothing appears, the two possible
    /// faults are "the mask never matched" and "the image never arrived", and both look
    /// like an empty pane. A pink `*` on a default background is the same edit made
    /// visible: pink cells mean the mask works and the picture is the problem.
    #[arg(long = "dbg-pink-placeholders")]
    pub pink_placeholders: bool,
}

impl Tui {
    pub async fn run(
        self,
        stdin: impl Read + Send + 'static,
        stdout: impl Write + Send + Sync + Unpin + 'static,
    ) -> io::Result<()> {
        // Before anything else can fail. The pane is this process's screen, so a
        // diagnostic written to stdout would land in the middle of it; a file is the only
        // place left. Installed unconditionally, because the failures this exists for are
        // the ones nobody asked to reproduce.
        install_logger();
        // Unconditional in a build that carries the profiler: a switch would have to be set
        // before the interesting session started, and the session worth profiling is the one
        // that is not already being profiled. Without the feature this is not compiled at all,
        // so a plain build carries no client and no way to ask for one.
        #[cfg(feature = "tracy")]
        install_tracing()?;
        let _raw = Raw::install()?;

        // One set of switches, shared: the server and every session read them, and a copy
        // per reader would be several answers to one question.
        let debug = std::sync::Arc::new(self.debug);

        // The keyboard, split on its way in. The terminal's answers to this side's graphics
        // escapes arrive on the same descriptor as the user's keystrokes, and only the library can
        // tell them apart, so it does the split and hands back two pipes: the input the command
        // reads, and the escapes the screen is drawn with. Without this the command would be typed
        // the terminal's own acknowledgements.
        let (controller, input) = easytgp::Controller::init_with_pipe(stdin)?;

        // Every byte this side puts on the screen goes through easytgp's writer: the rewrite of
        // the command's output and a session's placement both. One writer and one lock between
        // them is what stops a picture from being placed in the middle of a line the command
        // printed. Global because the escapes are global: one terminal, one destination.
        if !easytgp::set_sink(Box::new(stdout)) {
            return Err(io::Error::other("the screen's writer was already set"));
        }

        let (pty, master) = Pty::new(&self.command, input)?;

        // Where every window's cells are. Written by the proxy, which is the side that reads the
        // command's output, and read by the rewrite (to blank them) and by a session (to place a
        // frame there). Shared rather than borrowed because a session is a `'static` future in a
        // `FuturesUnordered`, and a borrowed one could not be.
        let anchors = std::sync::Arc::new(arc_swap::ArcSwap::from_pointee(
            std::collections::HashMap::new(),
        ));
        // A client that drew its address. One channel for the whole session: an announcement is a
        // channel of its own, so several clients are several messages through this one.
        let (barcodes, announcements) = tokio::sync::mpsc::channel(ANNOUNCEMENTS);

        let forwarding = proxy::run(
            futures_util::StreamExt::map(
                tokio_util::io::ReaderStream::new(master),
                |chunk| chunk.map(|bytes| bytes.to_vec()),
            ),
            anchors.clone(),
            barcodes,
        );

        // The medium is chosen here and nowhere else: a flag read at every frame is the branch the
        // type parameter exists to remove, but the flag still has to be read somewhere, and this is
        // the one place that knows what the user asked for. The two arms are different types, so
        // each is written out where it is built.
        let serving = async {
            if debug.no_shm {
                Server::new(debug.clone(), controller, anchors)
                    .run::<easytgp::Png>(announcements)
                    .await
            } else {
                Server::new(debug, controller, anchors)
                    .run::<easytgp::Shm>(announcements)
                    .await
            }
        };

        tokio::select! {
            // The command's half: it waits on the child and keeps the pty's grid in step with
            // this terminal.
            session = pty.run() => session,
            // The command's output: rewritten onto the screen, and scanned for what a client drew.
            forwarded = forwarding => forwarded,
            // The screen: dial whoever announced itself, and let every client take its turn.
            served = serving => served,
        }
    }

}

/// How many announcements can be waiting a dial.
///
/// One per pane that can be on the screen at once, with room to spare: an announcement is a
/// client that drew its address and is sitting there waiting to be dialed, so the queue is as
/// long as the panes a user could open rather than as long as the traffic.
const ANNOUNCEMENTS: usize = 16;

/// Puts this terminal in raw mode for the session; restores it on drop.
///
/// A re-export so the call site reads `Raw::install()` without a module path,
/// while the explanation stays next to the termios code. See [`raw::Raw`].
pub use raw::Raw;

/// Send `tracing`'s spans to the Tracy profiler, so a session can be looked at
/// rather than guessed at.
///
/// The server is the right half to hold it: this is the process that decodes a
/// frame, transmits it, and waits on the terminal's acknowledgement — three
/// different waits that look identical from outside, which is what the profiler
/// is for.
///
/// Cheap enough to be unconditional. Tracy's client records into a lock-free
/// ring and a background thread drains it to the profiler, so a span costs a few
/// nanoseconds and nothing at all when no profiler is attached. A switch would
/// have to be set before the interesting session started, which is the one that
/// is not already being profiled.
///
/// `tuigui` runs this half in its own process, so installing it here covers both
/// commands; the client half installs its own, because it is a separate process
/// inside the pty.
/// Send `tracing`'s spans to the Tracy profiler, so a session can be looked at
/// rather than guessed at.
/// Send every `log` record to `/tmp/tuigui.log`.
///
/// The path is fixed rather than per process, so the log of the run that just failed is
/// the one that is read. A single line per record: this is a file to grep, not to read.
fn install_logger() {
    struct File(log::LevelFilter);

    impl log::Log for File {
        fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
            metadata.level() <= self.0
        }

        fn log(&self, record: &log::Record<'_>) {
            if !self.enabled(record.metadata()) {
                return;
            }
            // Opened per record and appended: a session is long and a file held open
            // across it would be one more thing to get wrong on the way out. The cost is
            // one open per line, which these diagnostics can afford.
            use std::io::Write as _;
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("/tmp/tuigui.log")
            {
                let _ = writeln!(
                    file,
                    "[{}] {}: {}",
                    record.level(),
                    record.target(),
                    record.args()
                );
            }
        }

        fn flush(&self) {}
    }

    // A logger already installed wins: one capturer per process, and the second call is
    // a caller that has nothing to say.
    let _ = log::set_boxed_logger(Box::new(File(log::LevelFilter::Debug)));
    log::set_max_level(log::LevelFilter::Debug);
}

///
/// The server is the right half to hold it: this is the process that decodes a
/// frame, transmits it, and waits on the terminal's acknowledgement — three
/// different waits that look identical from outside, which is what the profiler
/// is for.
#[cfg(feature = "tracy")]
fn install_tracing() -> io::Result<()> {
    use tracing::Level;
    use tracing_subscriber::filter::Targets;
    use tracing_subscriber::layer::SubscriberExt as _;

    // This workspace's crates, and nothing else. A dependency's spans are a wall
    // of zones that answers none of the questions a frame is profiled for, and
    // there are far more of them than there are of ours. Filtered here rather
    // than hidden in the profiler's own UI, so a trace is clean wherever it is
    // opened and however many processes are writing to it.
    let ours = Targets::new()
        .with_target("compositor", Level::TRACE)
        .with_target("gui", Level::TRACE)
        .with_target("tui", Level::TRACE)
        .with_target("tuigui", Level::TRACE)
        .with_target("wire", Level::TRACE)
        .with_target("termputs", Level::TRACE);

    let layer = tracing_tracy::TracyLayer::default();

    // Reported, and not dropped. The client is built by `TracyLayer::default()`,
    // so it starts listening *before* this returns: a profiler connects and then
    // shows an empty timeline whenever no subscriber ever reached the spans.
    //
    // `set_global_default` and **not** `try_init`: `try_init` also installs a
    // bridge that takes over the `log` facade, and a caller may already have
    // given `log` its own logger. The bridge refuses to replace it, `try_init`
    // reports that as its own failure, and the tracing subscriber is never
    // installed at all — the whole shape of "the profiler connects and there is
    // nothing there".
    //
    // A second call fails here too, which is right: one subscriber per process.
    tracing::subscriber::set_global_default(tracing_subscriber::registry().with(ours).with(layer))
        .map_err(|e| io::Error::other(format!("could not install the profiler: {e}")))?;

    // One span, guaranteed, before anything else is instrumented. It answers "is
    // anything reaching the profiler at all", which is the first question when a
    // timeline comes back empty.
    tracing::info!(target: "tui", "profiler installed");
    Ok(())
}
