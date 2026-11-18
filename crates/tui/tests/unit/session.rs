use super::*;
use std::io::Write;
use std::sync::{Arc, Mutex};

use wire::transport::{Transport, Connection};
use wire::unix::UnixSocket;

/// The window the tests paint into: its surface is its identity, and that is what a
/// frame names.
const SURFACE: SurfaceId = SurfaceId {
    symbol: '❋',
    fg: (1, 2, 3),
    bg: (4, 5, 6),
};

/// A terminal that collects what it is sent and answers the liveness probes.
///
/// The library's reply path is a real conversation: a transfer asks the
/// terminal for an acknowledgement and waits for it, so a test that never
/// answers would hang rather than fail. This answers the way a terminal does —
/// echoing the marker it was asked about — which is what makes the awaited
/// transmit complete.
struct Terminal {
    /// Everything written to the terminal, escapes and all.
    written: Arc<Mutex<Vec<u8>>>,
    /// Where the acknowledgement goes: the input side of the library.
    keys: Mutex<std::io::PipeWriter>,
}

impl Write for Terminal {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.written.lock().unwrap().extend_from_slice(buf);

        // Anything the library waits on is answered by its `i=`: a liveness probe, a
        // transplace, a placement. The answer is the id it named, back with an `OK`, which
        // is the same conversation a real terminal has. Answering anything that names an id
        // is what makes this a terminal rather than a stub per command.
        let text = String::from_utf8_lossy(buf);
        for part in text.split('\x1b').filter(|p| p.starts_with("_G")) {
            let Some(marker) = part
                .split(',')
                .filter(|cd| cd.starts_with("i=") || cd.contains(",i="))
                .find_map(|cd| cd.get(cd.find("i=")? + 2..))
                .map(|v| v.trim_end_matches('\u{5c}'))
                .filter(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
            else {
                continue;
            };
            let ack = format!("\x1b_Gi={marker};OK\x1b\\");
            self.keys.lock().unwrap().write_all(ack.as_bytes())?;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A codec frame of one flat colour, which is all the painter looks at.
///
/// Encoded through the codec itself rather than anything simpler: a test that
/// feeds the decoder bytes another codec produced would pass by accident.
fn codec_frame(width: u32, height: u32) -> Vec<u8> {
    let mut encoder = codecs::h264::Encoder::new(width, height).expect("an encoder");
    let bgra = vec![20u8, 20, 20, 255].repeat((width * height) as usize);
    let stream = codecs::Encoder::encode(&mut encoder, &bgra, width, height, false)
        .expect("encode a frame");
    assert!(
        !stream.is_empty(),
        "the first frame of a stream carries its picture"
    );
    stream
}

/// A connection to throw away, so a session can be built.
///
/// The link is not what these tests are about, but a session cannot exist without one: it is the
/// thing a session is attached to. Both halves come back, because a session answers what it is
/// told — a peer dropped here would make every answer a broken pipe rather than the message.
async fn connection<T: Transport + 'static>() -> (Connection, Connection) {
    let host = T::new().await.expect("a transport hosts at its own address");
    let magic = host.to_magic().expect("a hosted transport announces itself");
    let (name, address) = magic
        .split_once(wire::transport::ANNOUNCE_SEPARATOR)
        .expect("an announcement is a name, a separator and an address");
    let dialer = T::from_magic(name, address).expect("our own announcement reads back");

    let (accepted, dialed) = tokio::join!(host.accept(), dialer.connect());
    (
        accepted.expect("the accepting side establishes"),
        dialed.expect("the dialing side establishes"),
    )
}

/// Where the one window is: the cells the scan would have found it on.
///
/// Written by hand because a client draws those cells: a real session gets this from a scan of
/// the screen, and a test has no screen to scan.
fn where_it_is() -> crate::proxy::Anchors {
    std::sync::Arc::new(arc_swap::ArcSwap::from_pointee(HashMap::from([(
        SURFACE,
        Some((3, 5)),
    )])))
}

/// Put everything the escape pipe has onto `terminal`, which is what a step does.
///
/// Every frame is transmitted under one id, and placed again.
///
/// The escape destination is global to the process and can only be set once, so both halves are
/// one test: two would race for it, and whichever lost would observe an empty terminal rather
/// than a wrong painting. The behaviour under test is a sequence anyway — first frame, then
/// next.
///
/// The re-placement is the point rather than a detail. A transmit under an id the terminal
/// already has **frees that image's placements**, so a session that places only its first frame
/// shows one picture and then a blank pane. What is asserted here is that the second frame
/// carries its own placement, under the same id.
#[tokio::test]
async fn every_frame_reuses_one_id_and_places_itself_again() {
    let written = Arc::new(Mutex::new(Vec::new()));

    // The wiring the program has: the library's escapes go through the sink installed here,
    // and that sink is also the terminal — it records the bytes and answers what it must, so
    // a paint can complete its wait for an acknowledgement.
    //
    // Both halves live in one sink because the destination is process-global: a test that
    // split recording from answering would have to route through a pipe this side would then
    // have to drain while the paint is still waiting.
    let (answers, replies) = std::io::pipe().expect("a pipe for the terminal's answers");
    let (controller, input) =
        easytgp::Controller::init_with_pipe(answers).expect("the controller's pipes");
    // Dropped: nothing here is typed, and a held writer would keep the router's forward alive.
    drop(input);

    let sink = Terminal {
        written: written.clone(),
        keys: Mutex::new(replies),
    };
    assert!(
        easytgp::set_sink(Box::new(sink)),
        "this test installs the only sink in the process"
    );

    let (mut session, _peer) = {
        let (accepted, peer) = connection::<UnixSocket>().await;
        (
            Session::<easytgp::Shm>::new(
                accepted,
                Arc::new(DebugOptions::default()),
                controller,
            ),
            peer,
        )
    };
    // The window, which brings its codec with it. A frame names only its surface, so a session
    // that has not been told about that window has nowhere to paint it.
    session
        .handle_inner(
            Message::OpenWindow {
                surface: SURFACE,
                codec: CodecId::H264,
            },
            &where_it_is(),
        )
        .await
        .expect("the window is opened");

    session
        .handle_inner(
            Message::Frame {
                surface: SURFACE,
                z: 0,
                cols: 8,
                rows: 8,
                bytes: Some(codec_frame(8, 8)),
            },
            &where_it_is(),
        )
        .await
        .expect("the first frame is painted");
    // The screen's thread writes the escapes into the same buffer, so this is where the paint
    // landed rather than a second thing that had to be asked for.
    let first = String::from_utf8(written.lock().unwrap().clone()).expect("escapes are text");
    // One command, `a=T`, that both replaces the pixels and makes the placement. The two used
    // to be separate, and the terminal frees an image's placements when its data is replaced —
    // so between them the picture was on screen with nothing drawing it, for as long as a round
    // trip takes. That gap is the flicker.
    assert!(first.contains("a=T"), "one transplace command: {first:?}");
    // The image is named by the window's own surface, so two windows are two pictures rather
    // than one replacing the other.
    let image = image_id(&SURFACE);
    assert!(first.contains(&format!("i={image}")), "{first:?}");
    assert!(first.contains("t=s"), "{first:?}");
    // The transmitted data's size, and the cell box it is fitted into. Both travel on one
    // command and are different keys.
    assert!(first.contains("s=8"), "{first:?}");
    assert!(first.contains("v=8"), "{first:?}");
    // The picture is placed where the cursor was moved to, which is the window's own top-left
    // cell: the scan reports (3, 5), and the cursor move is what puts it there. Not `x`/`y` —
    // those crop the source, which is why the position is a cursor move and not a key.
    assert!(first.contains("\u{1b}[6;4H"), "the cursor is moved: {first:?}");
    // Under the text *and* under the cells' own backgrounds, which the protocol puts strictly
    // below `INT32_MIN / 2` — a window's box is full of background colour, so a picture at or
    // above that boundary would be covered by the cells it exists to sit behind. Rank 0 is the
    // furthest back, so it is the floor of the range and not the boundary.
    assert!(first.contains(&format!("z={}", stacking(0))), "{first:?}");
    assert!(
        stacking(0) < i32::MIN / 2,
        "under the backgrounds: {}",
        stacking(0)
    );
    assert!(first.contains("p=98"), "{first:?}");
    // Without this the terminal moves the cursor by the placement's box once per frame, which
    // walks the picture down the screen.
    assert!(first.contains("C=1"), "{first:?}");

    let after_first = first.len();
    session
        .handle_inner(
            Message::Frame {
                surface: SURFACE,
                z: 3,
                cols: 8,
                rows: 8,
                bytes: Some(codec_frame(8, 8)),
            },
            &where_it_is(),
        )
        .await
        .expect("the second frame is painted");

    let all = written.lock().unwrap().clone();
    let second = String::from_utf8(all[after_first..].to_vec()).expect("escapes are text");
    // The same id, so the client's cells keep naming the image being updated, and its own
    // placement, under the same placement id — a repeat transmit replaces the placement that
    // was there.
    assert!(
        second.contains("a=T"),
        "a later frame transplaces: {second:?}"
    );
    assert!(
        second.contains(&format!("i={}", image_id(&SURFACE))),
        "the same id: {second:?}"
    );
    assert!(
        second.contains("p=98"),
        "under the same placement id: {second:?}"
    );
}

/// A frame with no new pixels sends the picture again rather than moving it.
///
/// A window that moves without its program drawing anything sends `bytes: None`, and the
/// obvious answer is `a=p`: place the image the terminal already holds. But the terminal is
/// entitled to have forgotten it — a delete removes the data, an off-screen window has its
/// placement drop — and then `a=p` answers `ENOENT`, which the session reports as a failure
/// and which has no recovery in it: there is nothing left to place from. The image keeps its
/// own pixels, so `a=T` makes them again and places them, and it is right whether or not the
/// terminal still has the old copy.
#[tokio::test]
async fn a_frame_with_no_new_pixels_sends_its_picture_again() {
    let written = Arc::new(Mutex::new(Vec::new()));
    let (answers, replies) = std::io::pipe().expect("a pipe for the terminal's answers");
    let (controller, input) =
        easytgp::Controller::init_with_pipe(answers).expect("the controller's pipes");
    drop(input);
    assert!(
        easytgp::set_sink(Box::new(Terminal {
            written: written.clone(),
            keys: Mutex::new(replies),
        })),
        "this test installs the only sink in the process"
    );

    let (mut session, _peer) = {
        let (accepted, peer) = connection::<UnixSocket>().await;
        (
            Session::<easytgp::Shm>::new(
                accepted,
                Arc::new(DebugOptions::default()),
                controller,
            ),
            peer,
        )
    };
    session
        .handle_inner(
            Message::OpenWindow {
                surface: SURFACE,
                codec: CodecId::H264,
            },
            &where_it_is(),
        )
        .await
        .expect("the window is opened");
    session
        .handle_inner(
            Message::Frame {
                surface: SURFACE,
                z: 0,
                cols: 8,
                rows: 8,
                bytes: Some(codec_frame(8, 8)),
            },
            &where_it_is(),
        )
        .await
        .expect("the first frame is painted");
    let after_first = written.lock().unwrap().len();

    session
        .handle_inner(
            Message::Frame {
                surface: SURFACE,
                z: 0,
                cols: 8,
                rows: 8,
                bytes: None,
            },
            &where_it_is(),
        )
        .await
        .expect("a frame with nothing new is not a failure");

    let all = written.lock().unwrap().clone();
    let moved = String::from_utf8(all[after_first..].to_vec()).expect("escapes are text");
    assert!(
        moved.contains("a=T"),
        "the picture travels with its placement: {moved:?}"
    );
    assert!(
        !moved.contains("a=p"),
        "a placement would name an image the terminal may have dropped: {moved:?}"
    );
    assert!(
        moved.contains(&format!("i={}", image_id(&SURFACE))),
        "under the same id, so the window's cells keep naming it: {moved:?}"
    );
    assert!(
        moved.contains("t=s"),
        "the pixels go again, through the medium the image was opened with: {moved:?}"
    );
}

/// A frame replaces the picture and does *not* remove it; a closed window does.
///
/// The terminal keeps an image until it is told to delete it, so something has to say so when a
/// window goes — otherwise the last frame stays on the screen for the rest of the session. It
/// must not be said per frame: every frame transmits under the same id, and a delete between two
/// of them is the picture removed and put back, which a viewer sees as a flicker.
#[tokio::test]
async fn a_frame_keeps_its_picture_and_a_closed_window_removes_it() {
    let written = Arc::new(Mutex::new(Vec::new()));
    let (answers, replies) = std::io::pipe().expect("a pipe for the terminal's answers");
    let (controller, input) =
        easytgp::Controller::init_with_pipe(answers).expect("the controller's pipes");
    drop(input);
    assert!(
        easytgp::set_sink(Box::new(Terminal {
            written: written.clone(),
            keys: Mutex::new(replies),
        })),
        "this test installs the only sink in the process"
    );

    let (mut session, _peer) = {
        let (accepted, peer) = connection::<UnixSocket>().await;
        (
            Session::<easytgp::Shm>::new(
                accepted,
                Arc::new(DebugOptions::default()),
                controller,
            ),
            peer,
        )
    };
    session
        .handle_inner(
            Message::OpenWindow {
                surface: SURFACE,
                codec: CodecId::H264,
            },
            &where_it_is(),
        )
        .await
        .expect("the window is opened");

    for _ in 0..2 {
        session
            .handle_inner(
                Message::Frame {
                    surface: SURFACE,
                    z: 0,
                    cols: 8,
                    rows: 8,
                    bytes: Some(codec_frame(8, 8)),
                },
                &where_it_is(),
            )
            .await
            .expect("the frame is painted");
    }

    let painted = String::from_utf8(written.lock().unwrap().clone()).expect("escapes are text");
    let delete = format!("a=d,i={}", image_id(&SURFACE));
    assert!(
        !painted.contains(&delete),
        "a frame does not delete the picture it just put up: {painted:?}"
    );

    session
        .handle_inner(
            Message::CloseWindow { surface: SURFACE },
            &where_it_is(),
        )
        .await
        .expect("the window is closed");

    let closed = String::from_utf8(written.lock().unwrap().clone()).expect("escapes are text");
    assert!(
        closed.contains(&delete),
        "the window's picture is removed with it: {closed:?}"
    );
}
