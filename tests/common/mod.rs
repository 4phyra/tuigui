//! What the pty tests share: a runtime directory of their own, and a runnable.
//!
//! Not every test binary uses every helper, and that is the point of a shared
//! module: a warning about the one a binary does not use would be noise.

#![allow(dead_code)]

use std::path::PathBuf;

/// A `$XDG_RUNTIME_DIR` for one test.
///
/// Per test, not shared: the socket `gui` binds is `$XDG_RUNTIME_DIR/tuigui.sock`,
/// so two sessions in one directory are two sessions at one path. Nothing breaks
/// while both run — neither dials here — but a suite that quietly contends is a
/// suite that fails the day one of them does.
pub fn runtime_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tuigui-e2e-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a runtime dir to bind under");
    dir
}

/// A program for `tuigui` to run.
///
/// `tuigui` takes the runnable as a bare path and gives it no arguments, so a
/// program that needs a command line (`foot` does) needs a wrapper. The script
/// is written into the test's own runtime directory and made executable; `exec`
/// in the body hands the process over, so the process the compositor watches is
/// the one that draws.
pub fn script(name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt as _;

    let path = runtime_dir(&format!("script-{name}")).join("run");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("a runnable");
    let mut permissions = std::fs::metadata(&path)
        .expect("the script that was just written")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&path, permissions).expect("the script is executable");
    path
}

/// The `foot` these tests stream, from the dev shell's `PATH`.
///
/// Checked rather than assumed: a missing `foot` fails here, naming what is
/// missing, instead of as a session that never draws a picture.
pub fn require_foot() {
    let version = std::process::Command::new("foot")
        .arg("--version")
        .output();
    match version {
        Ok(output) if output.status.success() => {}
        _ => panic!("foot is not runnable here; run these tests inside `nix develop`"),
    }
}
