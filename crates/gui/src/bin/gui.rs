//! `gui` — send frames to a running `tui` server and draw the pane that shows
//! them. All the work lives in the `gui` library; this is only the command-line
//! wrapper, so the same client can also be started in-process by `tuigui`.

use clap::Parser as _;

use gui::Gui;

// Eight: a frame's encode and its transmit are a few milliseconds each with a wait between them,
// so the threads beyond what those waits can occupy would spend their lives parked. The same
// count as `tui`, because a session started from either side should schedule its work the same
// way.
#[tokio::main(worker_threads = 8)]
async fn main() -> miette::Result<()> {
    Gui::parse()
        .run(tokio::io::stdin(), std::io::stdout())
        .await?;
    // The exit is `std::process::exit`, not a return, because the runtime cannot be dropped:
    // tokio's stdin read is a blocking task, and the one in flight when the session ends is
    // parked on this pty, whose master the server still holds on the other side. It never
    // returns input or end-of-file, and dropping the runtime waits for it — the process would
    // hang here after the session is over. The terminal is already back: the guard that held it
    // dropped as `run` returned.
    std::process::exit(0);
}
