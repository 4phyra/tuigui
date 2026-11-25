//! `tui` — run a command in a pty we own and forward its bytes to this terminal.
//!
//! All the work lives in the `tui` library; this is only the command-line
//! wrapper, so the same session can also be started in-process by `tuigui`.

use clap::Parser as _;

use tui::Tui;

// Eight: a decoded frame and a transmit are a few milliseconds each with a wait between them, so
// the threads beyond what those waits can occupy would spend their lives parked. The same count
// as `tuigui`, because a session started from either one should schedule its work the same way.
#[tokio::main(worker_threads = 8)]
#[tracing::instrument(skip_all)]
async fn main() {
    // `parse`, not `try_parse_from`: clap needs `argv[0]` to name itself in a
    // usage message, and skipping it made `--help` come out as the program name.
    let session = Tui::parse();
    // Handled here rather than returned from `main`. A returned error is printed to stderr by
    // the runtime, and this process owns the terminal: its stderr *is* the pane, so the report
    // would be drawn into (and scroll) the interface it is about. The log the session already
    // writes to is where a failure belongs.
    if let Err(e) = session.run(std::io::stdin(), std::io::stdout()).await {
        log::error!("the session failed: {e}");
        std::process::exit(1);
    }
}
