//! A window whose pane herdr stops showing must take its picture with it.
//!
//! The harness runs `tui herdr` — the outer terminal is this test's terminal — and inside
//! herdr a pane runs `gui foot`, which is what a user running a GUI inside herdr gets. The
//! picture arrives the same route it always does: `tui` scans herdr's output for the window's
//! anchor cells and places the image behind them, so the anchor cells going away is the only
//! signal it has that the window is gone from sight.
//!
//! That signal has to be acted on even when the window draws nothing more. A static window
//! sends no frames — the client's wire traffic is "pixels, a rank, or a box", and silence means
//! none of the three changed — so a hide path that is only reached *by a frame* never runs for
//! the exact case that needs it. Both tests below hold the window static and then take it away
//! from the screen:
//!
//! * switching to another tab, which leaves the window up but unseen, and
//! * closing foot, which is the window going away for good.
//!
//! Both assert on the wire rather than on pixels: a `delete` naming the image is the one
//! escape a terminal cannot misread, and it is what the picture's owner has to emit. The image
//! id comes from the payloads seen before the action, so the assert names exactly the picture
//! that was placed.
//!
//! The herdr server is private to this test: a config and state directory under the test's
//! runtime directory, and the `HERDR_*` variables of the caller scrubbed, so a stray `herdr`
//! invocation from here cannot reach a server that is not ours.

#![cfg(target_os = "linux")]

mod common;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use termlens::{Graphics, GraphicsAction, Key, Screen, Terminal};

/// The pane the whole session is measured in.
const SIZE: (u16, u16) = (110, 36);

/// How long a delete, once required, is given to reach the terminal.
///
/// The delete is written on the frame after the screen changes, or straight from the teardown
/// path — a write, not a wait. A session that never sends one is never going to.
const DELETE: Duration = Duration::from_secs(10);

/// A `herdr` reachable from the pane the tests are run in, or a failure that says so.
fn require_herdr() {
    let version = Command::new("herdr").arg("--version").output();
    match version {
        Ok(output) if output.status.success() => {}
        _ => panic!("herdr is not runnable here; run these tests inside `nix develop`"),
    }
}

/// A binary of this workspace, beside the `tuigui` this test was built with.
///
/// `CARGO_BIN_EXE_*` is only defined for the package under test, and the other two belong to
/// other packages, so the path is derived rather than handed over. A missing one is a build
/// that has not produced it, which is said here instead of as a session that cannot start.
fn sibling(name: &str) -> PathBuf {
    let path = Path::new(env!("CARGO_BIN_EXE_tuigui")).with_file_name(name);
    assert!(
        path.exists(),
        "{} is not beside tuigui; build the binaries first",
        path.display()
    );
    path
}

/// The herdr configuration for the test's own server.
///
/// No onboarding, no name prompt, no sounds, no update checks: anything that makes the first
/// run ask a question is a question the pane will wait on forever.
const CONFIG: &str = "\
onboarding = false

[terminal]

[ui]
sidebar_start_collapsed = true
prompt_new_tab_name = false
confirm_close = false

[ui.sound]
enabled = false

[update]
version_check = false
manifest_check = false
";

/// The two path roots herdr is given, both inside the test's runtime directory.
fn xdg(runtime: &Path) -> (PathBuf, PathBuf) {
    (runtime.join("config"), runtime.join("state"))
}

/// Run the `herdr` CLI against the test's own server.
fn herdr(runtime: &Path, args: &[&str]) -> std::process::Output {
    let (config_home, state_home) = xdg(runtime);
    Command::new("herdr")
        .args(args)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", runtime)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("XDG_STATE_HOME", &state_home)
        .env("HERDR_CONFIG_PATH", runtime.join("herdr.toml"))
        .output()
        .unwrap_or_else(|e| panic!("herdr {args:?} could not be run: {e}"))
}

/// Stop the test's server, so it does not outlive the test.
///
/// Best effort: a server that never came up has nothing to stop, and that is not a failure of
/// the test.
fn stop_server(runtime: &Path) {
    let _ = herdr(runtime, &["server", "stop"]);
}

/// Start `tui herdr` with an isolated config, and leave foot's window up in its first pane.
///
/// The pane is a plain shell, and the command it runs is typed into it, the way a user would:
/// the picture being tested is the one that comes up behind a typed command, not one wired up
/// any other way.
fn start_foot(runtime: &Path) -> termlens::Result<Terminal> {
    let (config_home, state_home) = xdg(runtime);
    std::fs::create_dir_all(&config_home).expect("a config dir for herdr");
    std::fs::create_dir_all(&state_home).expect("a state dir for herdr");
    std::fs::write(runtime.join("herdr.toml"), CONFIG).expect("herdr's config");
    let path = std::env::var_os("PATH").unwrap_or_default();

    let mut session = Terminal::builder()
        .size(SIZE.0, SIZE.1)
        .graphics(Graphics::Kitty)
        .cell_size(8, 16)
        .env_clear()
        .env("PATH", &path)
        .env("HOME", runtime)
        .env("XDG_RUNTIME_DIR", runtime)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("XDG_STATE_HOME", &state_home)
        .env("HERDR_CONFIG_PATH", runtime.join("herdr.toml"))
        .env("LANG", "C.UTF-8")
        .timeout(Duration::from_secs(90))
        .arg("herdr")
        .spawn(sibling("tui"))?;

    // The bytes typed before the pane's shell reads them wait in the pty's input queue, but
    // typing into a blank screen races herdr's first draw, so wait for the pane to show
    // something before typing.
    session.wait_until(|screen| !screen.full_text().trim().is_empty())?;
    let gui = sibling("gui");
    session.send_str(&format!("{} --channel unix --jpeg foot\r", gui.display()))?;
    // The picture is up when the terminal has been told to transmit one.
    session.wait_until(|screen| !image_ids(screen).is_empty())?;
    Ok(session)
}

/// The ids of every image the peer has placed, named by the payload that placed it.
fn image_ids(screen: &Screen) -> HashSet<u32> {
    screen
        .graphics()
        .payloads()
        .iter()
        .filter(|payload| {
            matches!(
                payload.action(),
                GraphicsAction::Place | GraphicsAction::Transmit | GraphicsAction::TransmitAndPlace
            )
        })
        .filter_map(|payload| payload.id())
        .collect()
}

/// How many placements of the given ids the screen has seen so far.
///
/// The count, not the set: the same id is placed once per frame the window is redrawn, and what
/// the switch-back waits for is one more placement after the switch, whatever the count was at.
fn placements(screen: &Screen, ids: &HashSet<u32>) -> usize {
    screen
        .graphics()
        .payloads()
        .iter()
        .filter(|payload| {
            matches!(
                payload.action(),
                GraphicsAction::Place | GraphicsAction::TransmitAndPlace
            )
        })
        .filter(|payload| payload.id().is_some_and(|id| ids.contains(&id)))
        .count()
}

/// The ids a delete has named so far.
fn deleted_ids(screen: &Screen) -> HashSet<u32> {
    screen
        .graphics()
        .payloads()
        .iter()
        .filter(|payload| payload.action() == GraphicsAction::Delete)
        .filter_map(|payload| payload.id())
        .collect()
}

/// Every graphics payload, for the failure message.
fn graphics_log(screen: &Screen) -> String {
    screen
        .graphics()
        .payloads()
        .iter()
        .map(|payload| format!("{:?} id={:?}", payload.action(), payload.id()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The assertion both tests end on: every id that was placed has been deleted.
fn assert_all_deleted(session: &mut Terminal, ids: &HashSet<u32>, what: &str) {
    let gone = session.wait_until_for(
        |screen| {
            let deleted = deleted_ids(screen);
            ids.iter().all(|id| deleted.contains(id))
        },
        DELETE,
    );
    let screen = session.screen();
    assert!(
        gone.is_ok(),
        "the picture is still on the terminal {what}: no delete was sent for {ids:?}\n\
         graphics traffic: {}\nscreen:\n{}",
        graphics_log(&screen),
        screen.full_text(),
    );
}

#[test]
fn switching_to_another_tab_removes_the_picture() -> termlens::Result<()> {
    require_herdr();
    common::require_foot();
    let runtime = common::runtime_dir("herdr-image-tab-switch");
    let mut session = start_foot(&runtime)?;
    let ids = image_ids(&session.screen());
    assert!(!ids.is_empty(), "foot's picture was placed before switching");

    // A second tab, focused: the foot pane stays up but is no longer shown.
    let created = herdr(&runtime, &["tab", "create", "--focus"]);
    assert!(
        created.status.success(),
        "herdr tab create failed: {}",
        String::from_utf8_lossy(&created.stderr)
    );

    assert_all_deleted(&mut session, &ids, "after switching tabs");

    // Back to the first tab: herdr repaints the pane, and the picture has to come back with it.
    // foot draws nothing on its own, so a picture that does not return is gone for good.
    let placed = placements(&session.screen(), &ids);
    session.send(Key::Ctrl('b'))?;
    session.send(Key::Char('p'))?;
    let restored = session.wait_until_for(
        |screen| placements(screen, &ids) > placed,
        DELETE,
    );
    let screen = session.screen();
    assert!(
        restored.is_ok(),
        "the picture was not placed again after switching back to its tab\n\
         graphics traffic: {}",
        graphics_log(&screen),
    );

    stop_server(&runtime);
    Ok(())
}

#[test]
fn closing_the_window_removes_the_picture() -> termlens::Result<()> {
    require_herdr();
    common::require_foot();
    let runtime = common::runtime_dir("herdr-image-close");
    let mut session = start_foot(&runtime)?;
    let ids = image_ids(&session.screen());
    assert!(!ids.is_empty(), "foot's picture was placed before closing");

    // `exit` in foot's shell: foot exits, the pane's command is over, and the window the
    // picture belonged to goes with it — the same teardown as the pane's close button.
    session.send_str("exit\r")?;

    assert_all_deleted(&mut session, &ids, "after foot exited");

    stop_server(&runtime);
    Ok(())
}
