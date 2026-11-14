use super::*;

/// Every variant survives a round trip, body and all.
#[test]
fn messages_round_trip() {
    for message in [
        Message::Ping("unix-socket : 3f9a1c2b".into()),
        // An empty label is not the same as no label: the variant must still
        // survive, or a client would read it as a frame.
        Message::Ping(String::new()),
        Message::Pong,
        Message::Frame {
            surface: SurfaceId {
                symbol: '❋',
                fg: (1, 2, 3),
                bg: (4, 5, 6),
            },
            z: 0,
            cols: 80,
            rows: 24,
            bytes: Some(b"raw frame bytes".to_vec()),
        },
        // Not text, so a codec that assumed UTF-8 would show up here.
        Message::Frame {
            surface: SurfaceId {
                symbol: '\u{10eeee}',
                fg: (0, 0, 0),
                bg: (255, 255, 255),
            },
            z: 9,
            cols: 1,
            rows: 1,
            bytes: Some(vec![0x00, 0xff, 0x1b, 0x9b]),
        },
        // No pixels at all: a frame that only moves the picture already on the far side.
        Message::Frame {
            surface: SurfaceId {
                symbol: '▒',
                fg: (7, 8, 9),
                bg: (10, 11, 12),
            },
            z: 1,
            cols: 12,
            rows: 5,
            bytes: None,
        },
        Message::OpenWindow {
            surface: SurfaceId {
                symbol: '❋',
                fg: (9, 8, 7),
                bg: (6, 5, 4),
            },
            codec: CodecId::H264,
        },
        Message::CloseWindow {
            surface: SurfaceId {
                symbol: '✳',
                fg: (1, 1, 1),
                bg: (2, 2, 2),
            },
        },
        Message::Error("the peer refused the frame".into()),
        Message::Error(String::new()),
    ] {
        let bytes = message.to_bytes().expect("messages serialize");
        let back = Message::from_bytes(&bytes).expect("archived messages decode");
        assert_eq!(back, message, "round trip lost something");
    }
}

/// The variants are distinguishable, which is the whole point of the archive's
/// variant index.
#[test]
fn variants_do_not_collide() {
    let ping = Message::Ping(String::new()).to_bytes().expect("serializes");
    let pong = Message::Pong.to_bytes().expect("serializes");
    assert_ne!(ping, pong);
}

/// A truncated or garbage message is an error, not a guess.
#[test]
fn malformed_messages_are_rejected() {
    assert!(
        Message::from_bytes(&[]).is_err(),
        "empty input is not a message"
    );
    assert!(
        Message::from_bytes(&[0xff, 0x01]).is_err(),
        "bytes that are not an archive must be refused, not guessed at"
    );
    // A real message cut in half: the archive is complete-shaped, so a
    // truncated body is caught by validation rather than by chance.
    let mut full = Message::Ping("a label".into())
        .to_bytes()
        .expect("serializes");
    full.truncate(full.len() / 2);
    assert!(
        Message::from_bytes(&full).is_err(),
        "half a message must not decode"
    );
}
