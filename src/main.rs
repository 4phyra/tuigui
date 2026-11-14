//! `tuigui` — start the `tui` server and the `gui` client as one unit.

use clap::Parser as _;

// `worker_threads` has to be a literal, so the count is a constant rather than half the cores
// computed at startup. Eight: a decoded frame and a transmit are a few milliseconds each with a
// wait between them, so the threads beyond what those waits can occupy would spend their lives
// parked. Both halves get the same count, and both binaries, because a session started from
// either one should schedule its work the same way.
#[tokio::main(worker_threads = 8)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Internal re-entry: this process *is* the client, started by the server
    // inside its pty. Checked first so the runnable's own path is never parsed as
    // this command's.
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() == Some(tuigui::INTERNAL_CLIENT_FLAG) {
        #[cfg(feature = "tracy")]
        tuigui::name_process("gui (client)");
        // A program name has to be put back before clap sees the rest. `parse_from`
        // treats its first element as `argv[0]` and skips it, so passing the runnable
        // directly made clap read the runnable's path as the program name and then
        // report that its required `<PATH>` was missing.
        let gui = gui::Gui::parse_from(std::iter::once("gui".to_string()).chain(args));
        // tokio's stdin, not `std::io`'s: the client reads this terminal as a stream, and the async
        // handle is the same descriptor with a future attached, so the draw loop can wait on a key
        // and on the program in one `select!`.
        gui.run(tokio::io::stdin(), std::io::stdout()).await?;
        return Ok(());
    }

    #[cfg(feature = "tracy")]
    tuigui::name_process("tui (server)");
    let config = tuigui::TuiGui::parse();
    config.run().await
}
