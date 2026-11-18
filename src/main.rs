//! `tuigui` — start the `tui` server and the `gui` client as one unit.

use clap::Parser as _;

// `worker_threads` has to be a literal, so the count is a constant rather than half the cores
// computed at startup. Eight: a decoded frame and a transmit are a few milliseconds each with a
// wait between them, so the threads beyond what those waits can occupy would spend their lives
// parked. Both halves get the same count, and both binaries, because a session started from
// either one should schedule its work the same way.
#[tokio::main(worker_threads = 8)]
async fn main() -> miette::Result<()> {
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
        //
        // Both outcomes exit here, and neither returns. A return from this function — from the
        // success path or the failure one — drops the runtime, and `Runtime::drop` waits for the
        // blocking read parked on this pty: the master is held by the server on the other side, so
        // that read never ends and the client hangs instead of going away. That is a session that
        // never exits, and a server waiting for a child that is waiting for a reader.
        //
        // So the report is printed by hand rather than by std, and the exit is explicit.
        match gui.run(tokio::io::stdin(), std::io::stdout()).await {
            Ok(()) => std::process::exit(0),
            Err(e) => {
                println!("{:?}", miette::Report::new(e));
                std::process::exit(1);
            }
        }
    }

    #[cfg(feature = "tracy")]
    tuigui::name_process("tui (server)");
    let config = tuigui::TuiGui::parse();
    config.run().await?;
    Ok(())
}
