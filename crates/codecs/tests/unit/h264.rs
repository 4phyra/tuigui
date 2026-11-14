use super::*;
use crate::Decoder as _;

/// A decoder that has shown a picture keeps showing it when the stream
/// says nothing new.
#[test]
fn the_last_frame_is_the_answer_to_a_quiet_stream() {
    let mut decoder = Decoder::new();
    let mut encoder = Encoder::new(16, 16).expect("an encoder");
    let bgra = vec![0u8, 0, 255, 255].repeat(16 * 16);
    let stream = crate::Encoder::encode(&mut encoder, &bgra, 16, 16, false)
        .expect("encode a frame");
    assert!(!stream.is_empty(), "the first frame carries its picture");

    let first = decoder.decode(&stream).expect("decodes").expect("a picture");
    let again = decoder.decode(&stream).expect("decodes");
    assert_eq!(again.as_ref().map(|f| f.rgb.as_slice()), Some(first.rgb.as_slice()));
}
