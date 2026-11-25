use super::*;
use crate::Decoder as _;

/// Whether this machine has a hardware AV1 pair, printing why not when it
/// has none. There is no software AV1 to fall back to, by design.
fn hw_backend() -> bool {
    match (Encoder::new(128, 128), Decoder::new()) {
        (Ok(_), Ok(_)) => true,
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("skipping: {e}");
            false
        }
    }
}

/// A decoder that has shown a picture keeps showing it when the stream
/// says nothing new.
#[test]
fn the_last_frame_is_the_answer_to_a_quiet_stream() {
    if !hw_backend() {
        return;
    }
    let mut decoder = Decoder::new().expect("a decoder");
    let mut encoder = Encoder::new(128, 128).expect("an encoder");
    let bgra = vec![0u8, 0, 255, 255].repeat(128 * 128);
    let stream =
        crate::Encoder::encode(&mut encoder, &bgra, 128, 128, false).expect("encode a frame");
    assert!(!stream.is_empty(), "the first frame carries its picture");

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
