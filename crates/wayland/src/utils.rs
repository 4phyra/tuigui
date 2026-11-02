//! Small helpers with no home of their own: the cursor overlay, and the
//! dispatch-forwarding macros our generic state needs in place of smithay's
//! `delegate_*!` macros (which take lifetimes, but not type generics).

use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;

/// Fire all pending frame callbacks on a window's surface tree, and its
/// subsurfaces' — the "you may draw the next frame" signal a client waits on
/// after registering one with `wl_surface.frame`.
pub(crate) fn fire_frame_callbacks(surface: &WlSurface) {
    smithay::wayland::compositor::with_surface_tree_downward(
        surface,
        (),
        |_, _, _| smithay::wayland::compositor::TraversalAction::DoChildren(()),
        |_, states, _| {
            let mut attrs = states
                .cached_state
                .get::<smithay::wayland::compositor::SurfaceAttributes>();
            let now = crate::server_time();
            for cb in attrs.current().frame_callbacks.drain(..) {
                cb.done(now);
            }
        },
        |_, _, _| true,
    );
}

/// The connecting process's pid, straight from the socket (`SO_PEERCRED`).
/// Linux-only by the crate gate.
pub(crate) fn peer_pid(stream: &std::os::unix::net::UnixStream) -> Option<u32> {
    use std::os::unix::io::AsRawFd;

    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let ok = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut cred as *mut libc::ucred).cast(),
            &mut len,
        )
    };
    if ok == 0 {
        Some(cred.pid as u32)
    } else {
        None
    }
}

/// The parent of `pid`, as the kernel reports it. `None`: no parent known
/// (init's child, or the process vanished between connect and lookup).
pub(crate) fn proc_parent(pid: u32) -> Option<u32> {
    sysinfo::System::new_all()
        .process(sysinfo::Pid::from_u32(pid))
        .and_then(|process| process.parent())
        .map(sysinfo::Pid::as_u32)
}


/// Convert a committed wl_shm buffer into pixels in `out`, sized `out_size`.
///
/// The capture surface is ARGB8888 little-endian, i.e. bytes B,G,R,A. A
/// straight row copy of an Argb8888/Xrgb8888 shm buffer already yields that
/// layout, so no channel order is changed here.
///
/// The buffer is **scaled** to `out_size`, not cropped to it. A committed buffer
/// covers the whole surface and the surface covers the whole output, so the two
/// differ only in resolution: the client picks how many pixels one logical pixel
/// is worth (`wl_surface.set_buffer_scale`) and the output's density is another
/// number. Cropping assumes the two agree, which loses a client's right and
/// bottom edges when it renders at a higher density and leaves it in a corner
/// when it does not.
#[tracing::instrument(name = "map_rgba", skip(buffer, out))]
pub(crate) fn map_rgba(
    buffer: &smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer,
    out: &mut Vec<u8>,
    out_size: (u32, u32),
) -> Result<(), crate::CompError> {
    use smithay::wayland::shm;
    shm::with_buffer_contents(buffer, |ptr, len, data| {
        let bpp = wl_bpp(data.format);
        if bpp != 4 {
            // Unsupported layout. Said out loud, because the consequence is invisible:
            // the window keeps its previous frame — which, for a window that has never
            // captured one, is no picture at all, so the pane shows the mask's blank and
            // nothing behind it.
            log::warn!(
                "a committed buffer is in {:?}, which this capture cannot read",
                data.format
            );
            return;
        }
        // Every dimension is the client's, so none of them is trusted to be
        // positive or to fit. A buffer that cannot be read whole keeps the
        // previous frame rather than producing a torn one.
        if data.width <= 0 || data.height <= 0 || data.stride <= 0 || data.offset < 0 {
            return;
        }
        let (bw, bh) = (data.width as usize, data.height as usize);
        let stride = data.stride as usize;
        let base = data.offset as usize;
        let (ow, oh) = (out_size.0 as usize, out_size.1 as usize);
        if ow == 0 || oh == 0 {
            return;
        }
        // One row has to fit in its stride, and the last row in the pool.
        let fits = (bw * 4 <= stride)
            .then(|| bh.checked_mul(stride).and_then(|s| base.checked_add(s)))
            .flatten()
            .is_some_and(|end| end <= len);
        if !fits {
            return;
        }
        out.clear();
        out.resize(ow * oh * 4, 0);
        // The pool is handed to this closure for exactly this, and the bounds
        // are checked above, so the slice covers every byte that is read.
        let pool = unsafe { std::slice::from_raw_parts(ptr, len) };
        // The margin a client draws around its window — a shadow, the strips it grabs its
        // edges by — is transparent in the buffer, and no codec here carries alpha: scaled
        // as it stands, that margin arrives as black bars around the picture. The opaqueness
        // is the only thing in the buffer that says where the window really is, so the
        // capture crops to it.
        let (left, top, cw, ch) = opaque_bounds(&pool[base..], stride, (bw, bh));
        if cw == 0 || ch == 0 {
            return;
        }
        let start = top * stride + left * 4;
        scale_bgra(&pool[base + start..], stride, (cw, ch), out, (ow, oh));
    })
    .map_err(|e| crate::CompError::Capture(e.to_string()))
}

/// The smallest rectangle holding every opaque pixel of a buffer, as
/// `(left, top, width, height)` in pixels.
///
/// A client draws its window somewhere inside its buffer and leaves the rest transparent:
/// shadows above and below, and the strips along the edges that it grabs a drag by. None of
/// that margin means anything to a codec without alpha, which would turn it into black bars.
/// The protocol lets a client state its window's rectangle with `set_window_geometry`, and
/// this capture does not track that — so the buffer itself is asked instead, and its opaque
/// pixels are the window.
///
/// A buffer with no opaque pixel at all answers with the whole buffer: there is no smaller
/// rectangle that would be right, and a blank frame beats no frame.
fn opaque_bounds(src: &[u8], stride: usize, size: (usize, usize)) -> (usize, usize, usize, usize) {
    let (w, h) = size;
    let (mut left, mut right) = (w, 0);
    let (mut top, mut bottom) = (h, 0);
    for y in 0..h {
        let row = y * stride;
        for x in 0..w {
            // Argb8888 little-endian: the fourth byte is the alpha.
            if src[row + x * 4 + 3] == 0 {
                continue;
            }
            left = left.min(x);
            right = right.max(x + 1);
            top = top.min(y);
            bottom = bottom.max(y + 1);
        }
    }
    if right <= left || bottom <= top {
        return (0, 0, w, h);
    }
    (left, top, right - left, bottom - top)
}

/// Resample one B,G,R,A buffer into another, of any size.
///
/// Each destination pixel is the source pixel it lands on. Nearest rather than
/// interpolated because what is being captured is an interface on its way to a
/// grid of terminal cells, where a pixel of blur costs legibility and buys
/// nothing.
///
/// `src` is `src_size.0 * 4` bytes of picture per row with `src_stride` bytes
/// between rows, and `dst` is written densely at `dst_size.0 * 4` per row.
#[tracing::instrument(name = "scale_bgra", skip(src, dst))]
fn scale_bgra(
    src: &[u8],
    src_stride: usize,
    src_size: (usize, usize),
    dst: &mut [u8],
    dst_size: (usize, usize),
) {
    let (bw, bh) = src_size;
    let (ow, oh) = dst_size;
    if bw == 0 || bh == 0 || ow == 0 || oh == 0 {
        return;
    }
    // Per-row memcpy: shm pools may be sparse, so never assume the row stride
    // makes rows contiguous.
    if bw == ow && bh == oh {
        for row in 0..oh {
            let from = row * src_stride;
            let to = row * ow * 4;
            dst[to..to + ow * 4].copy_from_slice(&src[from..from + ow * 4]);
        }
        return;
    }
    // One index per row and per column rather than a divide per pixel: at frame
    // sizes the divides are the whole capture.
    let cols: Vec<usize> = (0..ow).map(|x| x * bw / ow).collect();
    let rows: Vec<usize> = (0..oh).map(|y| y * bh / oh).collect();
    for (y, &sy) in rows.iter().enumerate() {
        let from = sy * src_stride;
        let to = y * ow * 4;
        for (x, &sx) in cols.iter().enumerate() {
            let s = from + sx * 4;
            let d = to + x * 4;
            dst[d..d + 4].copy_from_slice(&src[s..s + 4]);
        }
    }
}

fn wl_bpp(f: smithay::reexports::wayland_server::protocol::wl_shm::Format) -> usize {
    use smithay::backend::allocator::format::get_bpp;
    use smithay::wayland::shm;
    shm::shm_format_to_fourcc(f)
        .and_then(|fourcc| get_bpp(fourcc))
        .map(|bpp| bpp / 8)
        .unwrap_or(0)
}

/// Forward a `GlobalDispatch` from our state to the helper that owns the
/// protocol's bookkeeping.
///
/// Every name is fully qualified because a `macro_rules!` body resolves paths
/// at the definition site, not where it is invoked.
macro_rules! forward_global {
    ($interface:ty: $udata:ty => $helper:ty) => {
        impl
            smithay::reexports::wayland_server::GlobalDispatch<$interface, $udata>
            for crate::state::State
        {
            fn bind(
                state: &mut Self,
                dh: &smithay::reexports::wayland_server::DisplayHandle,
                client: &smithay::reexports::wayland_server::Client,
                resource: smithay::reexports::wayland_server::New<$interface>,
                data: &$udata,
                data_init: &mut smithay::reexports::wayland_server::DataInit<'_, Self>,
            ) {
                <$helper as smithay::reexports::wayland_server::GlobalDispatch<
                    $interface,
                    $udata,
                    Self,
                >>::bind(state, dh, client, resource, data, data_init)
            }
        }
    };
}

/// Forward a `Dispatch` from our state to the helper that owns the protocol's
/// bookkeeping. Same rule: every name is fully qualified.
macro_rules! forward {
    ($interface:ty: $udata:ty => $helper:ty) => {
        impl
            smithay::reexports::wayland_server::Dispatch<$interface, $udata>
            for crate::state::State
        {
            fn request(
                state: &mut Self,
                client: &smithay::reexports::wayland_server::Client,
                resource: &$interface,
                request: <$interface as smithay::reexports::wayland_server::Resource>::Request,
                data: &$udata,
                dhandle: &smithay::reexports::wayland_server::DisplayHandle,
                data_init: &mut smithay::reexports::wayland_server::DataInit<'_, Self>,
            ) {
                <$helper as smithay::reexports::wayland_server::Dispatch<
                    $interface,
                    $udata,
                    Self,
                >>::request(state, client, resource, request, data, dhandle, data_init)
            }
            fn destroyed(
                state: &mut Self,
                client: smithay::reexports::wayland_server::backend::ClientId,
                resource: &$interface,
                data: &$udata,
            ) {
                <$helper as smithay::reexports::wayland_server::Dispatch<
                    $interface,
                    $udata,
                    Self,
                >>::destroyed(state, client, resource, data)
            }
        }
    };
}

pub(crate) use forward;
pub(crate) use forward_global;

#[cfg(test)]
mod tests {
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
    fn the_same_size_is_copied_untouched() {
        let src = tagged(6, 4, 6 * 4);
        let mut dst = vec![0u8; 6 * 4 * 4];
        scale_bgra(&src, 6 * 4, (6, 4), &mut dst, (6, 4));
        assert_eq!(dst, src);
    }

    /// The size the output's scale produces: a client renders at twice the
    /// logical size it was configured with, so the buffer is larger than the
    /// output. The picture must be resampled across its whole width and height
    /// — which is what cropping got wrong.
    #[test]
    fn a_larger_buffer_is_scaled_down_across_its_whole_picture() {
        let (bw, bh) = (24, 16);
        let (ow, oh) = (20, 13);
        let src = tagged(bw, bh, bw * 4);
        let mut dst = vec![0u8; ow * oh * 4];
        scale_bgra(&src, bw * 4, (bw, bh), &mut dst, (ow, oh));

        assert_eq!(pixel(&dst, ow * 4, 0, 0), [0, 0, 0xa5, 0xff]);
        // The last output pixel reaches the far side of the buffer: column
        // 19 * 24 / 20 = 22, row 12 * 16 / 13 = 14. A crop left it at column 19
        // and row 12 of the buffer, which is the edge of the interface that went
        // missing.
        assert_eq!(pixel(&dst, ow * 4, ow - 1, oh - 1), [22, 14, 0xa5, 0xff]);
    }

    /// The other direction: a client that ignores the scale commits at the
    /// logical size, which is smaller than the output. The picture has to fill
    /// the frame rather than sit in the top-left corner of it.
    #[test]
    fn a_smaller_buffer_is_scaled_up_and_fills_the_frame() {
        let (bw, bh) = (10, 7);
        let (ow, oh) = (20, 13);
        let src = tagged(bw, bh, bw * 4);
        let mut dst = vec![0u8; ow * oh * 4];
        scale_bgra(&src, bw * 4, (bw, bh), &mut dst, (ow, oh));

        assert_eq!(pixel(&dst, ow * 4, 0, 0), [0, 0, 0xa5, 0xff]);
        assert_eq!(pixel(&dst, ow * 4, ow - 1, oh - 1), [9, 6, 0xa5, 0xff]);
        // The middle is the picture too, not the zero the old copy left behind.
        assert_eq!(pixel(&dst, ow * 4, 10, 6), [5, 3, 0xa5, 0xff]);
    }

    /// Rows are read where the stride says they are, so padding between them is
    /// never mistaken for picture.
    #[test]
    fn a_padded_stride_is_read_by_row() {
        let (bw, bh, stride) = (5, 3, 5 * 4 + 12);
        let src = tagged(bw, bh, stride);
        let mut dst = vec![0u8; bw * bh * 4];
        scale_bgra(&src, stride, (bw, bh), &mut dst, (bw, bh));

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
        scale_bgra(&src, 4 * 4, (4, 4), &mut dst, (0, 4));
        assert!(dst.iter().all(|&b| b == 0xff), "an empty target was written");
    }

    /// The margin a client leaves around its window is transparent, and cropping it is
    /// what keeps it from arriving as black bars: no codec here carries alpha.
    #[test]
    fn the_opaque_pixels_are_the_window() {
        let (bw, bh) = (8, 6);
        let stride = bw * 4;
        let mut src = vec![0u8; bw * bh * 4];
        // A window at (2, 1) of 4x3, with transparent margins around it.
        for y in 1..4 {
            for x in 2..6 {
                let o = y * stride + x * 4;
                src[o..o + 4].copy_from_slice(&[1, 2, 3, 0xff]);
            }
        }
        assert_eq!(opaque_bounds(&src, stride, (bw, bh)), (2, 1, 4, 3));
    }

    /// Nothing opaque is not a crop to nothing: the whole buffer is the only answer that
    /// draws something rather than a rectangle of no size.
    #[test]
    fn a_buffer_with_no_opaque_pixel_keeps_its_size() {
        let (bw, bh) = (4, 3);
        let src = vec![0u8; bw * bh * 4];
        assert_eq!(opaque_bounds(&src, bw * 4, (bw, bh)), (0, 0, bw, bh));
    }
}
