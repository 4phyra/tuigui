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
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let gui = Gui::parse();
    // tokio's stdin, not `std::io`'s: the client reads this terminal as a stream, and the async
    // handle is the same descriptor with a future attached.
    gui.run(tokio::io::stdin(), std::io::stdout()).await?;
    Ok(())
}
