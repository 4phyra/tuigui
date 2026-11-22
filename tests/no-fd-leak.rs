//! `--dbg-no-accept`: the client never answers the server's dial, so nothing is ever
//! accepted and the pairing screen stays up — the exact state in which re-announcing the
//! barcode once per output chunk parks one socket per chunk on the server.
//!
//! The runnable draws something constantly (`foot` running a tight echo loop) so the
//! client keeps repainting, the scanner keeps finding the barcode, and the server keeps
//! re-dialing. The session's fd count is then a direct readout of whether announcements
//! are being dialed per chunk (the leak) or once per distinct barcode.
//!
//! This test is the smoke check for the pairing-phase leak: it measures the server's
//! descriptor table over a few seconds and requires it to stay put. It is not a proof —
//! it only sees what the barcode window looks like while it is drawn, and a quiet client
//! redraws too rarely to grow anything — but it is the failure mode as it was observed.

#![cfg(target_os = "linux")]

mod common;

use std::fs;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use termlens::{Graphics, Terminal};

const EVERY: Duration = Duration::from_millis(300);
/// How many descriptors the server may gain over the sampling window. Threads and the
/// first repaints cost a handful; one dial per repainted chunk costs far more than this.
const MAX_GROWTH: usize = 8;

/// Descriptors the server currently holds, or `None` if it is gone (which is also a failure
/// mode worth distinguishing in the message).
fn fd_count(pid: u32) -> Option<usize> {
    fs::read_dir(format!("/proc/{pid}/fd")).ok().map(|d| d.count())
}

/// Every descendant of `pid`, children before parents, so that killing in order works.
fn descendants(pid: u32, out: &mut Vec<u32>) {
    if let Ok(tasks) = fs::read_dir(format!("/proc/{pid}/task")) {
        for task in tasks.flatten() {
            if let Ok(children) = fs::read_to_string(task.path().join("children")) {
                for child in children.split_whitespace().filter_map(|c| c.parse::<u32>().ok()) {
                    out.push(child);
                    descendants(child, out);
                }
            }
        }
    }
}

/// The whole tree, dead. The client is, by construction of `--dbg-no-accept`, unable to
/// exit on its own, and `foot` outlives a server it never hears back from — dropping the
/// `Terminal` only reaps the direct child, which leaks the rest. This test's whole job is
/// to leave nothing behind, whatever the assertion says.
fn kill_tree(pid: u32) {
    let mut pids = Vec::new();
    descendants(pid, &mut pids);
    pids.reverse();
    pids.push(pid);
    // Best effort, one call: a process that already exited turns its argument into a
    // harmless error, and there is no dependency here for a library `kill(2)`.
    let mut kill = std::process::Command::new("kill");
    kill.arg("-9");
    for pid in &pids {
        kill.arg(pid.to_string());
    }
    let _ = kill.status();
}

/// The server: the `tuigui` process whose argv names the runnable but is not the `--client`
/// re-entry of itself.
fn server_pid(runnable: &Path) -> Option<u32> {
    let needle = runnable.as_os_str().as_encoded_bytes();
    for entry in fs::read_dir("/proc").ok()?.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Ok(pid) = name.parse::<u32>() else { continue };
        let Ok(cmdline) = fs::read(format!("/proc/{pid}/cmdline")) else {
            continue;
        };
        let has = |part: &[u8]| cmdline.windows(part.len()).any(|w| w == part);
        let argv0 = cmdline.split(|b| *b == 0).next().unwrap_or(&[]);
        if has(needle) && has(b"tuigui") && !has(b"--client") && argv0.ends_with(b"tuigui") {
            return Some(pid);
        }
    }
    None
}

#[test]
fn fds_do_not_grow_while_the_peer_never_accepts() -> termlens::Result<()> {
    common::require_foot();
    let runtime = common::runtime_dir("no-fd-leak");
    // Something that draws without ever stopping, so the client keeps repainting the pane
    // and the scanner keeps meeting the barcode.
    let runnable = common::script(
        "no-fd-leak",
        "exec foot -- sh -c 'while true; do echo tick; sleep 0.05; done'",
    );

    let session = Terminal::builder()
        .size(100, 40)
        .graphics(Graphics::Kitty)
        .cell_size(8, 16)
        .env("XDG_RUNTIME_DIR", &runtime)
        .timeout(Duration::from_secs(15))
        .arg("--dbg-no-accept")
        .arg(&runnable)
        .spawn(env!("CARGO_BIN_EXE_tuigui"))?;
    eprintln!("[no-fd-leak] spawned");

    let pid = server_pid(&runnable).expect("the tuigui server is running");
    let first = fd_count(pid).expect("the server's descriptors are readable");

    // No waiting on any external signal: the leak is visible the moment the client paints,
    // so poll from spawn and fail the instant the count moves. A client that never paints
    // leaves the count flat and the test green — documented limitation; foot ticks 20 times
    // a second, and the leak is one dial per scanned chunk, so silence means no repaint,
    // not the bug being absent.
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut last = first;
    let mut leaked_at = None;
    eprintln!("[no-fd-leak] baseline {first} fds");
    while Instant::now() < deadline {
        thread::sleep(EVERY);
        last = fd_count(pid).expect("the server's descriptors are readable");
        eprintln!("[no-fd-leak] {last} fds");
        if last > first + MAX_GROWTH {
            leaked_at = Some(last);
            break;
        }
    }

    // Tear the tree down before judging: a failing assertion must not leave a
    // repainting client and foot behind.
    kill_tree(pid);
    drop(session);
    assert!(
        leaked_at.is_none(),
        "the server's descriptors grew from {first} to {} while nothing ever accepted — \
         announcements are being dialed faster than they are dropped",
        leaked_at.unwrap_or(last),
    );
    Ok(())
}
