//! Small helpers with no home of their own: the cursor overlay, and the
//! dispatch-forwarding macros our generic state needs in place of smithay's
//! `delegate_*!` macros (which take lifetimes, but not type generics).

use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;

/// Fire all pending frame callbacks on a window's surface tree, and its
/// subsurfaces' — the "you may draw the next frame" signal a client waits on
/// after registering one with `wl_surface.frame`.
#[tracing::instrument(name = "wayland.frame_callbacks", skip_all)]
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


/// Convert a committed wl_shm buffer into pixels in `out`, at the buffer's own size.
///
/// The capture surface is ARGB8888 little-endian, i.e. bytes B,G,R,A. A
/// straight row copy of an Argb8888/Xrgb8888 shm buffer already yields that
/// layout, so no channel order is changed here.
///
/// No scaling. What a client drew is what it drew, and that size is a fact about the picture
/// rather than something this side chooses: a surface's buffer is sized by the client — a
/// toplevel by the configure it answered, a subsurface by its own drawing — so scaling it to
/// some other rectangle would be inventing a size nobody measured. Placing the picture is the
/// composer's job; a window's box on the pane does that with the far side's own scaling.
///
/// The size comes back because the pixels are cropped to their opaque bounds, so a caller
/// cannot assume the buffer's dimensions.
#[tracing::instrument(name = "wayland.map_rgba", skip(buffer, out))]
pub(crate) fn map_rgba(
    buffer: &smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer,
    out: &mut Vec<u8>,
) -> Result<(u32, u32), crate::CompError> {
    use smithay::wayland::shm;
    let mut size = (0, 0);
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
        // One row has to fit in its stride, and the last row in the pool.
        let fits = (bw * 4 <= stride)
            .then(|| bh.checked_mul(stride).and_then(|s| base.checked_add(s)))
            .flatten()
            .is_some_and(|end| end <= len);
        if !fits {
            return;
        }
        // The pool is handed to this closure for exactly this, and the bounds
        // are checked above, so the slice covers every byte that is read.
        let pool = unsafe { std::slice::from_raw_parts(ptr, len) };
        // Copied whole, at the buffer's own size: the margin a client draws around its window
        // — a shadow, the strips it grabs its edges by — is transparent, and a codec without
        // alpha turns it black. That is the client's own drawing, though, and cropping it here
        // would report a size nobody asked for: the configure named the surface, this measures
        // the buffer, and the two have to be the same rectangle or the compositor's box can
        // never agree with the picture it is asked to fill.
        out.clear();
        out.resize(bw * bh * 4, 0);
        copy_bgra(&pool[base..], stride, (bw, bh), out);
        size = (bw as u32, bh as u32);
    })
    .map_err(|e| crate::CompError::Capture(e.to_string()))?;
    Ok(size)
}

/// Copy one B,G,R,A buffer into another, row by row at the same size.
///
/// `src` is `size.0 * 4` bytes of picture per row with `src_stride` bytes between rows — a shm
/// pool may be sparse, so the stride is not assumed to make rows contiguous — and `dst` is
/// written densely at `size.0 * 4` per row.
///
/// No resampling: the caller passes the size it cropped to, and the pixels are those pixels.
#[tracing::instrument(name = "wayland.copy_bgra", skip(src, dst))]
fn copy_bgra(src: &[u8], src_stride: usize, size: (usize, usize), dst: &mut [u8]) {
    let (w, h) = size;
    if w == 0 || h == 0 {
        return;
    }
    for row in 0..h {
        let from = row * src_stride;
        let to = row * w * 4;
        dst[to..to + w * 4].copy_from_slice(&src[from..from + w * 4]);
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
}
