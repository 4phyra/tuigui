use super::*;

fn parse(s: &str) -> Option<Message> {
    Message::parse_next(&mut s.bytes().peekable())
}

fn cds(reply: &Message) -> Vec<(char, &str)> {
    reply.cds.iter().map(|cd| (cd.0, cd.1.as_str())).collect()
}

#[test]
fn single_control_data_with_empty_payload() {
    let r = parse("\x1b_Gi=31;\x1b\\").unwrap();
    assert_eq!(cds(&r), [('i', "31")]);
    assert_eq!(r.payload.as_deref(), Some(&[] as &[u8]));
}

#[test]
fn multiple_control_data_kept_in_order() {
    let r = parse("\x1b_Gi=31,q=1,m=0;\x1b\\").unwrap();
    assert_eq!(cds(&r), [('i', "31"), ('q', "1"), ('m', "0")]);
}

#[test]
fn payload_after_terminator_is_captured() {
    let r = parse("\x1b_Gi=31;AAEC\x1b\\").unwrap();
    assert_eq!(cds(&r), [('i', "31")]);
    assert_eq!(r.payload.as_deref(), Some("AAEC".as_bytes()));
}

#[test]
fn noise_before_the_intro_is_skipped() {
    let r = parse("junk\x1b[31m\x1b_Gi=31;\x1b\\").unwrap();
    assert_eq!(cds(&r), [('i', "31")]);
}

/// The object a `t=s` command names has to exist, hold exactly the pixels,
/// and be resolvable the way a terminal's `shm_open` resolves it.
#[test]
fn shm_frame_holds_the_pixels_under_the_name_it_reports() {
    let pixels: Vec<u8> = (0..=255u8).cycle().take(300).collect();
    let frame = ShmFrame::create(&pixels).expect("create shm object");

    // The protocol requires a leading slash and no other slash, and name
    // lengths must fit the platform limit (macOS: 31 bytes).
    assert!(frame.name().starts_with('/'), "{}", frame.name());
    assert_eq!(frame.name().matches('/').count(), 1, "{}", frame.name());
    assert!(frame.name().len() <= 31, "{}", frame.name());

    // A terminal resolves the name through shm_open, which on Linux is the
    // same file under /dev/shm.
    let path = std::path::Path::new(SHM_DIR).join(frame.name().trim_start_matches('/'));
    assert_eq!(std::fs::read(&path).expect("readable"), pixels);

    // The payload is base64 of the name, not of the pixels: that is the
    // whole difference between this medium and the inline one.
    let decoded = base64_turbo::STANDARD
        .decode(frame.payload())
        .expect("payload decodes");
    assert_eq!(decoded, frame.name().as_bytes());
    assert!(decoded.len() < pixels.len() / 4, "payload must be the name");

    // Dropping removes the object so a long session does not leak names.
    let leaked = path.to_path_buf();
    drop(frame);
    assert!(std::fs::read(&leaked).is_err(), "object outlived the frame");
}

#[test]
fn shm_format_declares_the_shared_memory_medium() {
    assert_eq!(Shm.medium(), "s");
    assert!(!Shm.is_inline(), "a name payload must never be chunked");
    // RGB, so the terminal interprets the object as packed pixels.
    assert_eq!(Shm.to_cd().format(), "f=24");
}

#[test]
fn inline_formats_stay_on_the_direct_medium() {
    // Each is a distinct type, so they are asserted one at a time.
    assert_eq!(Png.medium(), "d");
    assert!(Png.is_inline());
    assert_eq!(Png.to_cd().format(), "f=100");

    assert_eq!(Rgb.medium(), "d");
    assert!(Rgb.is_inline());
    assert_eq!(Rgb.to_cd().format(), "f=24");

    assert_eq!(Rgba.medium(), "d");
    assert!(Rgba.is_inline());
    assert_eq!(Rgba.to_cd().format(), "f=32");
}

/// The wire command a shared memory transmit produces: the medium is named,
/// the payload is the object's name, and there is no `m` key because a name
/// is never chunked.
#[test]
fn shm_transmit_puts_the_medium_and_a_name_on_the_wire() {
    let pixels = vec![7u8; 4 * 4 * 3];
    let frame = ShmFrame::create(&pixels).expect("create shm object");
    let name_payload = frame.payload();

    // What ImageRequest::exec_transmit assembles for an Shm format.
    let escape = Message::new()
        .cd('I', 1)
        .cd_opt('s', Some(4u32))
        .cd_opt('v', Some(4u32))
        .push_cd(Shm.to_cd())
        .cd('t', Shm.medium())
        .payload(name_payload.clone())
        .into_escape();

    let text = String::from_utf8(escape).expect("escape is ASCII");
    assert!(text.starts_with("\x1b_G"), "{text:?}");
    assert!(text.ends_with("\x1b\\"), "{text:?}");
    assert!(text.contains(",f=24"), "packed RGB: {text:?}");
    // `t` is the last key before the payload, so it is terminated by `;`.
    assert!(text.contains(",t=s;"), "the shared memory medium: {text:?}");
    assert!(!text.contains(",m="), "a name must never be chunked: {text:?}");
    assert!(
        !text.contains("t=d"),
        "must not fall back to the inline medium: {text:?}"
    );

    // The payload is the name, not the pixels.
    let payload = text
        .split_once(';')
        .and_then(|(_, rest)| rest.strip_suffix("\x1b\\"))
        .expect("payload");
    assert_eq!(
        base64_turbo::STANDARD.decode(payload).unwrap(),
        frame.name().as_bytes()
    );
}

/// The saving the medium exists for: at any real frame size the payload is
/// the object's name, so it does not grow with the image at all.
#[test]
fn shm_payload_does_not_scale_with_the_image() {
    // A 1920x1080 RGB frame, the size the medium is worth using at.
    let pixels = vec![0u8; 1920 * 1080 * 3];
    let frame = ShmFrame::create(&pixels).expect("create shm object");
    let payload = frame.payload();

    // The inline medium would carry base64 of every pixel.
    let inline = base64_turbo::STANDARD.encode(&pixels).len();
    assert!(
        payload.len() * 1000 < inline,
        "payload {} should be orders of magnitude below the inline {}",
        payload.len(),
        inline
    );
    // Still exactly the object's name.
    assert_eq!(
        base64_turbo::STANDARD.decode(&payload).unwrap(),
        frame.name().as_bytes()
    );
}

/// A raw frame must carry its dimensions: the terminal sizes an `f=24`
/// frame's payload as `width * height * 3` and rejects a zero-sized one with
/// "ERROR: EINVAL:Zero width/height not allowed". PNG escaped this because it
/// describes its own size, so the omission stayed invisible until a raw
/// medium existed.
#[test]
fn a_raw_frame_falls_back_to_the_image_size() {
    let image_size = Some(ImageSize {
        width: 1692,
        height: 1220,
    });

    // Unset, as `..Default::default()` leaves them: the image's own size is
    // used, which is what keeps a raw frame from being zero-sized.
    assert_eq!(resolve_dims(None, None, image_size), (Some(1692), Some(1220)));

    // An explicit size always wins (the tiled renderer passes one).
    assert_eq!(
        resolve_dims(Some(64), Some(32), image_size),
        (Some(64), Some(32))
    );

    // Mixed: each axis falls back independently.
    assert_eq!(
        resolve_dims(Some(64), None, image_size),
        (Some(64), Some(1220))
    );

    // No size anywhere stays None, leaving the terminal to report it.
    assert_eq!(resolve_dims(None, None, None), (None, None));
}

/// A self-describing format must declare nothing: the terminal still checks
/// a declared size against the image, so a stale dimension would be rejected
/// where omitting it was fine. This is what keeps the PNG path unchanged.
#[test]
fn a_self_describing_format_declares_no_dimensions() {
    assert!(
        !Png.needs_dimensions(),
        "PNG carries its own size in its header"
    );
    assert!(Rgb.needs_dimensions(), "raw RGB is sized from s/v");
    assert!(Rgba.needs_dimensions());
    assert!(Shm.needs_dimensions(), "the shm medium carries raw RGB");

    // The rule the call sites apply.
    let declared = |needs: bool, requested: Option<u32>, image: Option<ImageSize>| {
        if needs {
            resolve_dims(requested, requested, image).0
        } else {
            None
        }
    };
    let size = Some(ImageSize {
        width: 1692,
        height: 1220,
    });
    assert_eq!(declared(Png.needs_dimensions(), None, size), None);
    assert_eq!(declared(Shm.needs_dimensions(), None, size), Some(1692));
}

/// A frame's object must outlive the send that named it.
///
/// This is the bug that produced "EBADF:Failed to open file for graphics
/// transmission ... No such file or directory": the request was dropped at
/// the end of `new_frame`, whose `Drop` unlinked the object before the
/// terminal could open it. The image now holds it across the send.
#[test]
fn a_transmitted_frame_keeps_its_object_alive() {
    let pixels = vec![5u8; 8 * 8 * 3];
    let frame = Arc::new(ShmFrame::create(&pixels).expect("create shm object"));
    let path =
        std::path::Path::new(SHM_DIR).join(frame.name().trim_start_matches('/'));

    let mut image: DynImage<Shm> = DynImage {
        id: DynId::Assigned(Arc::new(1)),
        placements: HashSet::new(),
        frames: HashMap::new(),
        // The base image's own object: held for the image's whole life, so
        // it is deliberately not one of the ones `sync` releases.
        request: ImageRequest {
            format: Shm,
            size: Some(ImageSize {
                width: 8,
                height: 8,
            }),
            data: vec![],
            shm: None,
        },
        held: None,
        frame_shm: vec![],
        pl_id_alloc: {
            let mut slab = Slab::new();
            slab.next();
            slab
        },
        pending: vec![],
    };

    // A frame created from a request hands its object to the image rather
    // than letting the request's drop remove it.
    image.frame_shm.push(frame.clone());
    assert_eq!(std::fs::read(&path).expect("readable"), pixels);

    // Dropping the request's own handle must not unlink while the image
    // still holds one: this is the ownership the fix establishes.
    drop(frame);
    assert!(
        std::fs::read(&path).is_ok(),
        "the object must outlive the sending call"
    );

    // With nothing in flight, `sync` resolves at once and releases the
    // objects, so an animation does not accumulate a frame per frame. There
    // is no runtime here, but an empty pending list makes it ready on the
    // first poll, so a noop waker is enough.
    let mut sync = image.sync();
    let waker = std::task::Waker::noop();
    let mut cx = std::task::Context::from_waker(&waker);
    assert!(matches!(
        std::pin::Pin::new(&mut sync).poll(&mut cx),
        std::task::Poll::Ready(Ok(()))
    ));
    drop(sync);
    assert!(image.frame_shm.is_empty(), "objects are freed once acked");
    // The newest one is not freed outright: it moves into `held`, which is
    // what lets the image be sent again after the terminal erases it
    // without re-encoding a frame the caller has already moved past.
    assert!(
        image.last_object().is_some(),
        "the newest object is kept for a re-send"
    );
    assert!(
        std::fs::read(&path).is_ok(),
        "the held object must stay on disk"
    );
}

/// The inline path is unchanged: direct medium, and still chunked when the
/// payload is larger than one command can carry.
#[test]
fn inline_transmit_keeps_the_direct_medium_and_chunks() {
    let escape = Message::new()
        .cd('I', 1)
        .push_cd(Rgb.to_cd())
        .cd('t', Rgb.medium())
        .payload(vec![b'A'; 10])
        .into_escape();
    let text = String::from_utf8(escape).unwrap();
    assert!(text.contains(",t=d;"), "{text:?}");
    assert!(text.contains(",f=24"), "{text:?}");
}

#[test]
fn consecutive_replies_are_parsed_one_by_one() {
    let mut buf = "\x1b_Gi=31;\x1b\\\x1b_Gq=1;OK\x1b\\".bytes().peekable();

    let first = Message::parse_next(&mut buf).unwrap();
    assert_eq!(cds(&first), [('i', "31")]);

    let second = Message::parse_next(&mut buf).unwrap();
    assert_eq!(cds(&second), [('q', "1")]);
    assert_eq!(second.payload.as_deref(), Some("OK".as_bytes()));

    assert!(Message::parse_next(&mut buf).is_none());
}

#[test]
fn value_may_contain_equals_sign() {
    let r = parse("\x1b_Ga=b=c;\x1b\\").unwrap();
    assert_eq!(cds(&r), [('a', "b=c")]);
}

#[test]
fn truncated_replies_return_none() {
    for s in ["\x1b_G", "\x1b_Gi", "\x1b_Gi=3", "\x1b_Gi=31;AA"] {
        assert!(parse(s).is_none(), "expected None for {s:?}");
    }
}

/// Parse `input`, and report both what came back and what was forwarded.
///
/// The two together, because the whole point of the forwarding path is that a
/// reply is *removed* from the stream and everything else is *kept*: asserting
/// only one of the two would pass on an implementation that dropped input or
/// left a reply in it.
fn parse_forwarding(input: &str) -> (Vec<Message>, String) {
    let mut buf = input.bytes().peekable();
    let mut forwarded = Vec::new();
    let mut messages = Vec::new();
    while let Some(message) = Message::parse_next_with(&mut buf, &mut forwarded) {
        messages.push(message);
    }
    (messages, String::from_utf8(forwarded).expect("forwarded input is text"))
}

/// Input with no reply in it is passed through, byte for byte.
#[test]
fn plain_input_is_forwarded_unchanged() {
    let (messages, forwarded) = parse_forwarding("hello");
    assert!(messages.is_empty(), "there was no reply to parse");
    assert_eq!(forwarded, "hello");
}

/// A reply is consumed and the input around it is forwarded, in order.
#[test]
fn a_reply_is_removed_and_the_input_around_it_survives() {
    let (messages, forwarded) = parse_forwarding("ls\x1b_Gi=31;\x1b\\-la");
    assert_eq!(messages.len(), 1);
    assert_eq!(cds(&messages[0]), [('i', "31")]);
    // The command is not run, but as far as the parse is concerned the bytes
    // before the reply and after it are one continuous stream of input.
    assert_eq!(forwarded, "ls-la");
}

/// Several replies, with input between them, each removed once.
#[test]
fn each_reply_is_removed_from_a_mixed_stream() {
    let (messages, forwarded) =
        parse_forwarding("a\x1b_Gi=1;OK\x1b\\b\x1b_Gi=2;OK\x1b\\c");
    assert_eq!(messages.len(), 2);
    assert_eq!(cds(&messages[0]), [('i', "1")]);
    assert_eq!(cds(&messages[1]), [('i', "2")]);
    assert_eq!(forwarded, "abc");
}

/// An `ESC` that begins a key rather than a reply is input, not a reply.
///
/// This is the case that makes the naive version wrong: an arrow key also
/// starts with `ESC`, and treating it as a truncated reply would eat it.
#[test]
fn an_escape_that_is_not_a_reply_is_forwarded() {
    let (messages, forwarded) = parse_forwarding("\x1b[A");
    assert!(messages.is_empty(), "an arrow key is not a reply");
    assert_eq!(forwarded, "\x1b[A");
}

/// A truncated reply is not forwarded as input either.
///
/// Half a reply has nowhere to go: it is not a message the router can use, and
/// handing it to the command would type the terminal's own answer into it.
#[test]
fn a_truncated_reply_is_neither_parsed_nor_forwarded() {
    let (messages, forwarded) = parse_forwarding("\x1b_Gi=31;AA");
    assert!(messages.is_empty());
    assert_eq!(forwarded, "");
}

/// The ordinary entry point behaves exactly like the forwarding one with a sink.
#[test]
fn parsing_without_forwarding_matches_the_forwarding_path() {
    let input = "key\x1b_Gi=7;OK\x1b\\more";
    let (with, _) = parse_forwarding(input);
    let plain = {
        let mut buf = input.bytes().peekable();
        let mut messages = Vec::new();
        while let Some(message) = Message::parse_next(&mut buf) {
            messages.push(message);
        }
        messages
    };
    assert_eq!(with.len(), plain.len());
    assert_eq!(cds(&with[0]), cds(&plain[0]));
}

/// A controller routes the replies out of a stream and forwards the rest.
///
/// Synthetic input rather than a terminal: the point is the splitting, and a
/// real terminal would only add the latencies that make such a test flaky. The
/// keystrokes must arrive on the forward pipe *while* the router runs, which is
/// the property that lets both halves live in one process.
#[test]
fn a_controller_routes_replies_and_forwards_the_input_between_them() {
    /// Collects everything written to it, so the test can look at it after.
    struct Collect(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for Collect {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    // One keystroke, one reply, another keystroke. The reply is the ack for the
    // liveness probe this library sends when a transfer is awaited, so it is
    // also what the router is looking for.
    let forwarded = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let input = b"ab\x1b_Gi=100001;OK\x1b\\cd".to_vec();
    let _controller = Controller::init_with_forwarding(
        std::io::Cursor::new(input),
        Collect(forwarded.clone()),
    );

    // The router runs on its own thread, so the input arrives asynchronously.
    // A short wait is enough: the whole stream is in memory and needs no IO.
    for _ in 0..100 {
        if forwarded.lock().unwrap().len() >= 4 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    // Both keystrokes survive, in order, with the reply removed from between
    // them. If the reply were forwarded the command would be typed the
    // terminal's own answer, and if the intro were treated as input the two
    // `ESC _ G` bytes would show up here too.
    let got = forwarded.lock().unwrap().clone();
    assert_eq!(String::from_utf8(got).expect("input is text"), "abcd");
}

/// The escape destination cannot be moved once it is set.
///
/// A second destination would split one image's escapes across two pipes, so the installer
/// refuses. That the *installed* one receives an escape is not asserted here: the destination
/// is process-global and the first test to run wins it, which makes routing order-dependent in
/// a test binary. It is checked where it matters instead, by a real session showing the
/// escapes on the terminal.
#[test]
fn the_escape_destination_cannot_be_moved() {
    let (_reader, first) = std::io::pipe().expect("a pipe");
    assert!(
        writer::set_sink(Box::new(first)),
        "the first destination must be taken"
    );
    let (_other, second) = std::io::pipe().expect("a pipe");
    assert!(
        !writer::set_sink(Box::new(second)),
        "a second destination must be refused"
    );
}
