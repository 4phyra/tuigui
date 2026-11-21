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
async fn main() {
    // Printed here rather than returned from `main`. A returned error is drawn by the runtime
    // with `Error: ` in front of it, which puts a second frame around a report that miette has
    // already drawn: the inner `×` and `╰─▶` end up nested inside the outer one and the picture
    // stops being readable. `{:?}` on a `Report` is the drawn form, and it is the whole output.
    if let Err(e) = Gui::parse()
        .run(tokio::io::stdin(), std::io::stdout())
        .await
    {
        // A newline first, because the report starts with miette's own `×` and no leading space:
        // without one it lands on the row the session was drawing on, and the header of the
        // diagram runs into whatever the pane left there.
        println!();
        println!("{:?}", miette::Report::new(e));
        std::process::exit(1);
    }
    // The exit is `std::process::exit`, not a return, because the runtime cannot be dropped:
    // tokio's stdin read is a blocking task, and the one in flight when the session ends is
    // parked on this pty, whose master the server still holds on the other side. It never
    // returns input or end-of-file, and dropping the runtime waits for it — the process would
    // hang here after the session is over. The terminal is already back: the guard that held it
    // dropped as `run` returned.
    std::process::exit(0);
}
