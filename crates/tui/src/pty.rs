//! The command on a pty we own, and the handles out of it.
//!
//! ```text
//!   Pty::new ──▶ master (read)  ── the command's output
//!            ├──▶ master (write) ── its input
//!            ├──▶ master (size)  ── the ioctl a resize needs
//!            └──▶ Child          ── so its exit can be waited on
//! ```
//!
//! The pty is what makes the command behave like a terminal program rather than a pipeline:
//! it gets a controlling terminal, a window size, and job control. What it writes comes back
//! out of the master, and what is written to the master arrives on its stdin.
//!
//! Nothing here pumps. The master handles are handed to the caller, which is the only place
//! that knows where output goes and where input comes from: a keyboard that has to be
//! filtered first, a filter that rewrites cells, a screen model that has to see both.

use std::ffi::OsStr;
use std::io;
use std::os::unix::ffi::OsStrExt as _;
use std::path::Path;
use std::process::Stdio;

use tokio::process::{Child, Command};

use rustix::pty::{grantpt, openpt, ptsname, unlockpt, OpenptFlags};

/// A pty we own, with the wrapped command attached to its slave.
pub struct Pty {
    /// The command, so its exit can be waited on.
    child: Child,
    /// One handle to the master, for setting the grid on a resize.
    ///
    /// The fd `TIOCSWINSZ` needs. The other two directions are moved out to their consumers —
    /// the output to the reader, the input to the thread that pumps the keyboard into it.
    resizable: std::fs::File,
    /// The grid the pty was created with, in cells.
    ///
    /// Read from this terminal rather than inherited by the command, and kept here so a
    /// client can be told the size it is drawing into: a muxer's pane does not pass its own
    /// geometry down through the environment.
    cols: u32,
    rows: u32,
}

impl Pty {    /// Start `command` on a fresh pty of this terminal's size.
    ///
    /// Everything the caller needs comes back out of here: the direction of the master the
    /// output is read from, and the child itself. The **input** direction stays in here,
    /// because it is this side's to pump: `stdin` is the keyboard the reply router already
    /// stripped of this side's own answers, and forwarding it into the master is the one thing
    /// the command cannot do for itself.
    pub fn new(
        command: &[String],
        mut input: std::io::PipeReader,
    ) -> io::Result<(Self, tokio::fs::File)> {
        // The window size is a property of the terminal device rather than of this process, so it
        // is read from stdin — the terminal the user is looking at — and handed to the pty the
        // command runs on.
        let ws = shared_utils::resize::size()?;

        let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).map_err(io::Error::from)?;
        grantpt(&master).map_err(io::Error::from)?;
        unlockpt(&master).map_err(io::Error::from)?;
        let name = ptsname(&master, Vec::new()).map_err(io::Error::from)?;
        let slave = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(Path::new(OsStr::from_bytes(name.as_bytes())))?;
        rustix::termios::tcsetwinsize(&master, ws).map_err(io::Error::from)?;
        let master = std::fs::File::from(master);

        let child = exec(command, &slave)?;

        // The keyboard, pumped into the master: the one edge the command cannot make for itself.
        let mut writing = master.try_clone()?;
        std::thread::spawn(move || io::copy(&mut input, &mut writing));

        let pty = Self {
            child,
            resizable: master.try_clone()?,
            cols: u32::from(ws.ws_col),
            rows: u32::from(ws.ws_row),
        };
        Ok((pty, tokio::fs::File::from_std(master)))
    }

    /// Wait until the command exits, keeping the pty's size in step with this terminal. Blocks.
    ///
    /// Two things are waited on: the command exiting, which is the end of the session; and a
    /// resize of the real terminal, which does not end anything but has to be acted on here
    /// because this is where the pty's fd is.
    ///
    /// Dropping the child on the way out is the kill: it is `kill_on_drop`, so a session that
    /// ended for any reason leaves no command behind.
    pub async fn run(mut self) -> io::Result<()> {
        loop {
            tokio::select! {
                status = self.child.wait() => {
                    // The command's own outcome, reported rather than only logged: a session that
                    // ends because the wrapped program failed must not look like one that ended
                    // because the program finished, or the failure has nowhere to surface.
                    let status = status?;
                    return if status.success() {
                        Ok(())
                    } else {
                        Err(io::Error::other(format!("the command exited with {status}")))
                    };
                }
                // The terminal was resized. The child's pty still has the old grid, so the command
                // keeps drawing for a screen that is no longer the right shape until it is told —
                // which for a full-screen program means it lays itself out against the wrong size.
                resized = shared_utils::resize::resized() => {
                    // A resize nobody could read leaves the pty at its old size, and the next one
                    // arrives anyway: the command is laid out for a screen one size stale rather
                    // than for no screen at all.
                    let ws = match resized {
                        Ok(ws) => ws,
                        Err(e) => {
                            tracing::warn!("could not read the new size: {e}");
                            continue;
                        }
                    };
                    if let Err(e) = rustix::termios::tcsetwinsize(&self.resizable, ws) {
                        tracing::warn!("could not tell the command its new size: {e}");
                    }
                    self.cols = u32::from(ws.ws_col);
                    self.rows = u32::from(ws.ws_row);
                    self.signal();
                }
            }
        }
    }

    /// Tell the command its grid moved.
    ///
    /// The size is in the kernel now, but a full-screen program only notices by being
    /// signalled — the same `SIGWINCH` the terminal sent us is what it does not receive,
    /// because it is on a pty of its own.
    fn signal(&self) {
        // A pid is always positive and fits an `i32`, which is what `from_raw` takes;
        // `and_then` keeps the failure visible instead of inventing a pid that would signal
        // an unrelated process.
        let pid = self
            .child
            .id()
            .and_then(|id| i32::try_from(id).ok())
            .and_then(rustix::process::Pid::from_raw);
        match pid {
            Some(pid) => {
                if let Err(e) = rustix::process::kill_process(pid, rustix::process::Signal::WINCH) {
                    tracing::warn!("could not signal the command: {e}");
                }
            }
            // Already reaped, or a pid that cannot exist: nothing to resize.
            None => {}
        }
    }
}

/// Start `cmd` on `slave`, with the pty as its controlling terminal.
fn exec(cmd: &[String], slave: &std::fs::File) -> io::Result<Child> {
    let mut c = Command::new(&cmd[0]);
    c.args(&cmd[1..]);
    c.kill_on_drop(true);
    c.stdin(Stdio::from(slave.try_clone()?));
    c.stdout(Stdio::from(slave.try_clone()?));
    c.stderr(Stdio::from(slave.try_clone()?));
    unsafe {
        c.pre_exec(|| {
            // New session, then adopt the pts as the controlling terminal so job control and
            // job-control signals behave.
            rustix::process::setsid().map_err(io::Error::from)?;
            rustix::process::ioctl_tiocsctty(std::io::stdin()).map_err(io::Error::from)?;
            Ok(())
        });
    }
    c.spawn()
}
