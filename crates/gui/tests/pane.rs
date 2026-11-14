//! The client as a program: started in a real pty, read off its grid.
//!
//! What a widget test cannot reach is the half of this client that is the terminal itself: the
//! line discipline it takes, the inline viewport it draws the dock into, and the order those two
//! happen in. Both are invisible from inside the process, and that order is what decides whether
//! the dock sits under the banner or scrolls into it. So the binary is spawned and asked what is
//! on its screen.
//!
//! # Why every wait names `pairing`
//!
//! The dock's light breathes from the wall clock, so its cell changes colour on every frame and
//! the pane never holds still. A `snapshot_after` or `wait_stable` therefore cannot settle here —
//! both wait for a picture that is not going to stop moving — so the waits below are predicates.
//!
//! The word `pairing` is painted by the light, which is drawn after the record and after the
//! buttons, so a screen showing it has that whole dock in it. That is the harness's second rule —
//! wait on the last thing the application paints — and it is what lets the snapshot taken next be
//! read as one frame rather than two.

use termlens::Terminal;

/// The pane the client draws into: wide enough for both buttons and a record.
const SIZE: (u16, u16) = (80, 24);

/// The client, under a runtime dir of its own.
///
/// Per test, not shared: the socket it binds is `$XDG_RUNTIME_DIR/tuigui.sock`, so two clients in
/// one directory would be two clients at one path and the second would unlink the first's. They
/// never dial here, so nothing breaks — but a suite whose tests quietly contend is a suite that
/// fails the day one of them does.
fn client(name: &str) -> termlens::Result<Terminal> {
    let runtime = std::env::temp_dir().join(format!("tuigui-pane-{name}"));
    std::fs::create_dir_all(&runtime).expect("a runtime dir to bind under");
    Terminal::builder()
        .size(SIZE.0, SIZE.1)
        .env("XDG_RUNTIME_DIR", &runtime)
        .args([
            // The pane's own socket, named rather than left to the client's default: the
            // default is the remote-capable p2p transport, and this test is about the address
            // a local server dials.
            "--channel",
            "unix",
            // A program that says nothing. It never starts anyway — `gui` spawns the program
            // only once the other side has dialed — so this is naming one, not running one.
            "true",
            "--scale",
            "1",
        ])
        .timeout(std::time::Duration::from_secs(20))
        .spawn(env!("CARGO_BIN_EXE_gui"))
}

/// The dock is drawn, and it says it is waiting to be dialed.
///
/// The dock is three rows — a border, the row its content sits on, and a border. It is found by
/// the word on its top border rather than by index, because where it sits is the next test's
/// question and this one should not fail for that reason.
#[test]
fn the_dock_says_it_is_waiting_to_be_dialed() -> termlens::Result<()> {
    let mut client = client("waiting")?;
    client.wait_until(|s| s.contains("pairing"))?;

    let screen = client.screen();
    let at = screen
        .find("pairing")
        .expect("the dock says which stage it is in");

    // The two screens it offers, and one of them marked. The mark is a colour, so it is asserted
    // on the cell rather than on the text: the word is there either way, and a button that lost
    // its highlight is a button a user cannot tell is up.
    assert!(
        screen.contains("Logs") && screen.contains("Windows"),
        "both screens are offered: {screen}"
    );
    let (row, col) = screen.find("Logs").expect("the logs button");
    assert_eq!(
        row,
        at.0 + 1,
        "the buttons are on the dock's content row, one below the light: {screen}"
    );
    let marked = screen.cell(row, col).expect("the button's own cell");
    assert_ne!(
        marked.style().bg,
        termlens::Color::Default,
        "the screen that is up is the one marked: {screen}"
    );
    Ok(())
}

/// The record on the dock's row is an announcement a server could dial.
///
/// The client says where it can be reached by drawing its address in braille — the one channel
/// that survives a multiplexer, because it becomes cells and cells are what a multiplexer draws.
/// Read back here the way a server reads it: the run of braille on the row, decoded, then split on
/// the separator the transports define. Both halves of this test are the two halves of the
/// channel: the cells are the announcement, and the text they decode to is what `from_magic` reads.
#[test]
fn the_dock_draws_an_address_a_server_can_dial() -> termlens::Result<()> {
    let mut client = client("announcing")?;
    client.wait_until(|s| s.contains("pairing"))?;

    let screen = client.screen();
    // The row inside the dock's border, which is where the record is drawn: one below the light.
    let row = screen.find("pairing").expect("the dock").0 + 1;

    // The run of braille, as the cells a scanner would hand to `cellcode::decode`: from the first
    // braille cell to the first one that is not, which is the same rule the server's own scan
    // uses. The dock's border and its padding are before it, so the run is found rather than
    // assumed to start at column zero.
    let record: Vec<cellcode::Cell> = (0..SIZE.0)
        .filter_map(|col| screen.cell(row, col))
        .map(|cell| {
            let symbol = cell.contents().chars().next()?;
            let termlens::Color::Rgb(r, g, b) = cell.style().fg else {
                return None;
            };
            let cell = cellcode::Cell {
                symbol,
                fg: cellcode::Rgb(r, g, b),
            };
            cell.bits().map(|_| cell)
        })
        .skip_while(Option::is_none)
        .map_while(std::convert::identity)
        .collect();
    assert!(
        record.len() >= 2,
        "the dock carries a record, not a scattered mark: {:?}",
        screen.row_text(row)
    );

    let announced = cellcode::decode(record.into_iter()).expect("the record is a record");
    let (name, address) = announced
        .split_once(wire::transport::ANNOUNCE_SEPARATOR)
        .expect("an announcement is a name, the separator, and an address");
    assert_eq!(
        name,
        wire::unix::NAME,
        "the record names a transport this build can dial: {announced:?}"
    );
    assert!(
        address.ends_with("tuigui.sock"),
        "the address is this pane's own socket under the runtime dir it was given: {address:?}"
    );

    Ok(())
}

/// The banner is written before the dock, so the dock is what sits last on the screen.
///
/// The client prints its name and only then builds the terminals, and the inline viewport anchors
/// at the cursor. Getting that order wrong puts the banner in the pane instead of above it, or
/// leaves it under the dock's border — neither of which is visible from inside the process, since
/// both are the terminal's own cursor answering.
#[test]
fn the_banner_is_written_above_the_dock() -> termlens::Result<()> {
    let mut client = client("banner")?;
    client.wait_until(|s| s.contains("pairing"))?;

    let screen = client.screen();
    let dock = screen.find("pairing").expect("the dock").0;

    // The banner's own shape, four rows of block letters. Its last row is the one above the dock,
    // and the row above that is the first: a gap means something else drew between them.
    assert!(
        screen.row_text(dock - 1).contains('\u{2584}'),
        "the banner's last row is directly above the dock: {screen}"
    );

    Ok(())
}
