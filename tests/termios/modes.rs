//! One session, with the pty's termios read on either side of it.
//!
//! The modes are written to a file in the session's runtime directory rather
//! than read off the screen: the client draws its dock over the rows the shell
//! printed on, and an overwritten row can split the value across a wrap. A file
//! is not the pane, so the dock cannot reach it.

use std::path::Path;
use std::time::Duration;

use termlens::{Graphics, Terminal};

/// The pane the session runs in.
///
/// Its size does not reach the assertions — the report is a file — but the pane
/// is still a real one, because the client takes the terminal's dimensions as
/// the buffer it streams.
const SIZE: (u16, u16) = (100, 40);

/// The pty's modes, as `stty -g` reports them, around one session.
pub struct Report {
    /// The modes before `tuigui` ran.
    pub before: String,
    /// The modes after it exited.
    pub after: String,
    /// What `tuigui` exited with, as the shell saw it.
    pub code: String,
}

/// Run `runnable` under `tuigui` in a pty, and read the modes either side of it.
///
/// The shell writes its report into the runtime directory and `wait_exit` is
/// what says the report is complete; nothing is read from the pane, because the
/// pane's last rows belong to the client and to the dock it draws over them.
pub fn run(name: &str, runnable: &Path) -> termlens::Result<Report> {
    let runtime = crate::common::runtime_dir(name);
    let report = runtime.join("modes");
    let report_path = word(&report);
    let tuigui = word(Path::new(env!("CARGO_BIN_EXE_tuigui")));
    let runnable = word(runnable);
    let script = format!(
        "before=$(stty -g)\n\
         {tuigui} {runnable}\n\
         code=$?\n\
         after=$(stty -g)\n\
         printf 'MODE-BEFORE %s\\nMODE-AFTER %s\\nTUIGUI-EXIT %s\\n' \
             \"$before\" \"$after\" \"$code\" > {report_path}\n"
    );

    let mut pty = Terminal::builder()
        .size(SIZE.0, SIZE.1)
        // A terminal that speaks the protocol, so the session's first transmit
        // completes instead of waiting for an answer that never comes.
        .graphics(Graphics::Kitty)
        .cell_size(8, 16)
        .env("XDG_RUNTIME_DIR", &runtime)
        .timeout(Duration::from_secs(60))
        .args(["-c", script.as_str()])
        .spawn("/bin/sh")?;

    pty.wait_exit()?;
    let text = read(&report);

    Ok(Report {
        before: field(&text, "MODE-BEFORE "),
        after: field(&text, "MODE-AFTER "),
        code: field(&text, "TUIGUI-EXIT "),
    })
}

/// One shell word holding `path`.
///
/// Quoted rather than interpolated bare: the paths are this test's own temporary
/// directory today, but a quoting rule that holds only for today's path is a rule
/// that breaks the day the directory is named differently.
fn word(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

/// The report the shell wrote, or a failure naming where it should have been.
fn read(path: &Path) -> String {
    match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => panic!(
            "the session did not report its modes to {}: {error}",
            path.display()
        ),
    }
}

/// The value on the line beginning with `prefix`.
///
/// Whole lines, not a run of hex characters: the report is a file the shell
/// wrote, so a line is a line and the value is everything after the space.
fn field(text: &str, prefix: &str) -> String {
    for line in text.lines() {
        if let Some(value) = line.strip_prefix(prefix) {
            return value.to_owned();
        }
    }
    panic!("the report has no {prefix:?} line: {text}")
}
