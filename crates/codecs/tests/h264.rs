//! The codec pair, tested end to end on whatever hardware this machine has:
//! encode with the crate's encoder, decode with the crate's decoder, and
//! compare pixels.
//!
//! A test that feeds the decoder bytes another codec produced would pass by
//! accident; these feed it what this crate's own encoder emits.
//!
//! There is no software H.264 here, on purpose: a machine with no hardware
//! path — CI, for one — skips every test with the reason on stderr instead of
//! silently exercising a fallback that the shipped binary does not carry.

use codecs::{CodecId, Decoder, Encoder};

/// Whether this machine has a hardware H.264 pair, printing why not when it
/// has none. The smallest frame the hardware accepts is chosen so a backend
/// that is present cannot be missed for being handed too small a picture.
fn hw_backend() -> bool {
    match (
        codecs::h264::Encoder::new(128, 128),
        codecs::h264::Decoder::new(),
    ) {
        (Ok(_), Ok(_)) => true,
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("skipping: {e}");
            false
        }
    }
}

/// One solid B,G,R,A frame, encoded through the real codec.
fn encoded(width: u32, height: u32, (b, g, r): (u8, u8, u8)) -> Vec<u8> {
    let mut encoder = codecs::h264::Encoder::new(width, height).expect("an encoder");
    let bgra = vec![b, g, r, 255].repeat((width * height) as usize);
    let stream = encoder.encode(&bgra, width, height, false).expect("encode");
    assert!(!stream.is_empty(), "the first frame carries its picture");
    assert!(
        stream.starts_with(&[0, 0, 0, 1]) || stream.starts_with(&[0, 0, 1]),
        "the wire carries Annex-B: the stream starts with a start code"
    );
    stream
}

/// The whole pair on a real stream: a picture in, the same picture out, close
/// enough that nothing on a terminal grid can tell the difference.
///
/// H.264 is lossy and 4:2:0, so exact bytes are not the assertion; a solid
/// red frame that comes back mostly red is.
#[test]
fn encode_then_decode_round_trip() {
    if !hw_backend() {
        return;
    }
    let stream = encoded(128, 128, (0, 0, 255));
    let mut decoder = codecs::h264::Decoder::new().expect("a decoder");
    let frame = decoder
        .decode(&stream)
        .expect("decodes")
        .expect("a picture");
    assert_eq!(frame.width, 128);
    assert_eq!(frame.height, 128);
    assert_eq!(frame.rgb.len(), 128 * 128 * 3);
    for pixel in frame.rgb.chunks_exact(3) {
        assert!(pixel[0] > 150, "red stayed red: {pixel:?}");
        assert!(pixel[1] < 110, "red stayed red: {pixel:?}");
        assert!(pixel[2] < 110, "red stayed red: {pixel:?}");
    }
}

/// A decoder that has shown a picture keeps showing it when the stream says
/// nothing new: a repaint gets the last picture, not nothing.
#[test]
fn the_last_frame_is_the_answer_to_a_quiet_stream() {
    if !hw_backend() {
        return;
    }
    let stream = encoded(128, 128, (0, 0, 255));
    let mut decoder = codecs::h264::Decoder::new().expect("a decoder");
    let first = decoder
        .decode(&stream)
        .expect("decodes")
        .expect("a picture");
    let again = decoder.decode(&stream).expect("decodes");
    assert_eq!(
        again.as_ref().map(|f| f.rgb.as_slice()),
        Some(first.rgb.as_slice())
    );
}

/// A stream's first picture is decodable without a second: a stream that
/// needed lookahead would leave the first client waiting for a frame that
/// never comes.
#[test]
fn the_first_frame_is_available_before_anything_else_arrives() {
    if !hw_backend() {
        return;
    }
    let stream = encoded(128, 128, (0, 0, 255));
    let mut decoder = codecs::h264::Decoder::new().expect("a decoder");
    assert!(
        decoder.decode(&stream).expect("decodes").is_some(),
        "the first picture decodes from the first stretch of stream"
    );
}

/// The announced codec builds the decoder; JPEG always, H.264 when the
/// hardware is there. An H.264 decoder on a machine without hardware is the
/// loud failure this crate exists to give.
#[test]
fn the_announced_codec_picks_the_implementations() {
    assert!(CodecId::Jpeg.to_decoder().is_ok());
    if hw_backend() {
        assert!(CodecId::H264.to_decoder().is_ok());
    }
}

/// A codec's stream describes a picture size, and capture sizes are pixel
/// sizes: a window whose physical width is odd gets a chroma row padded to a
/// pair, and the picture comes back a pixel wider. That has to round-trip
/// anyway — the pane asks for odd sizes all the time.
#[test]
fn an_odd_capture_size_still_makes_a_picture() {
    if !hw_backend() {
        return;
    }
    let stream = encoded(233, 129, (0, 0, 255));
    let mut decoder = codecs::h264::Decoder::new().expect("a decoder");
    let frame = decoder
        .decode(&stream)
        .expect("decodes")
        .expect("a picture");
    assert!(frame.width >= 233 && frame.height >= 129);
    assert_eq!(
        frame.rgb.len(),
        frame.width as usize * frame.height as usize * 3
    );
    let pixel = &frame.rgb[..3];
    assert!(pixel[0] > 150, "red stayed red: {pixel:?}");
}
