use super::*;

/// A buffer whose every pixel carries its own coordinates, so a wrong
/// mapping shows up as the wrong pixel rather than the wrong colour.
fn tagged(w: usize, h: usize, stride: usize) -> Vec<u8> {
    let mut buf = vec![0u8; stride * h];
    for y in 0..h {
        for x in 0..w {
            let o = y * stride + x * 4;
            buf[o] = x as u8;
            buf[o + 1] = y as u8;
            buf[o + 2] = 0xa5;
            buf[o + 3] = 0xff;
        }
    }
    buf
}

fn pixel(buf: &[u8], stride: usize, x: usize, y: usize) -> [u8; 4] {
    let o = y * stride + x * 4;
    buf[o..o + 4].try_into().expect("four bytes")
}

#[test]
fn the_picture_is_copied_untouched() {
    let src = tagged(6, 4, 6 * 4);
    let mut dst = vec![0u8; 6 * 4 * 4];
    copy_bgra(&src, 6 * 4, (6, 4), &mut dst);
    assert_eq!(dst, src);
}

/// Rows are read where the stride says they are, so padding between them is
/// never mistaken for picture.
#[test]
fn a_padded_stride_is_read_by_row() {
    let (bw, bh, stride) = (5, 3, 5 * 4 + 12);
    let src = tagged(bw, bh, stride);
    let mut dst = vec![0u8; bw * bh * 4];
    copy_bgra(&src, stride, (bw, bh), &mut dst);

    for y in 0..bh {
        for x in 0..bw {
            assert_eq!(pixel(&dst, bw * 4, x, y), pixel(&src, stride, x, y));
        }
    }
}

/// A zero-sized side is not a picture, and must not reach the index maths.
#[test]
fn an_empty_side_writes_nothing() {
    let src = tagged(4, 4, 4 * 4);
    let mut dst = vec![0xffu8; 4 * 4 * 4];
    copy_bgra(&src, 4 * 4, (0, 4), &mut dst);
    assert!(dst.iter().all(|&b| b == 0xff), "an empty target was written");
}
