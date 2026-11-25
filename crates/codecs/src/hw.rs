//! The hardware-frame plumbing `ffmpeg-next` does not wrap.
//!
//! Device contexts, frame-pool contexts, moving a picture into or out of a
//! hardware surface, and the `get_format` callback that refuses to decode in
//! software all go through `ffmpeg::ffi` here. Everything above this module
//! works through these wrappers, so the raw pointers and the ownership rules
//! are in one place.

use std::ffi::c_void;
use std::io;

use ffmpeg_next::ffi;
use ffmpeg_next::frame;

/// A failure from the FFmpeg C API, with what was being attempted.
#[tracing::instrument(skip_all)]
pub fn failure(what: &str, code: i32) -> io::Error {
    io::Error::other(format!("{what}: {}", ffmpeg_next::Error::from(code)))
}

/// An owned reference to a hardware device (`AVHWDeviceContext`).
pub struct Device(*mut ffi::AVBufferRef);

impl Device {
    /// Open a hardware device of `kind`.
    ///
    /// This is the hardware probe: the driver is initialised here, so a
    /// machine without one fails here and not at the first picture.
    ///
    /// VAAPI is asked for the driver's default first — a display-backed device
    /// when there is one — and then for each DRM render node in turn. FFmpeg's
    /// "default" VAAPI device is an empty `AVVAAPIDeviceContext.display`, which
    /// a machine with no X or Wayland display cannot initialise, so the render
    /// nodes are the real candidates on a headless box. Each is tried by
    /// opening it, so a node that exists but cannot decode still fails the
    /// probe; the node list is a list of guesses, never a decision.
    #[tracing::instrument(skip_all)]
    pub fn open(kind: ffi::AVHWDeviceType) -> Result<Self, io::Error> {
        let mut last = match Self::create(kind, None) {
            Ok(device) => return Ok(device),
            Err(e) => e,
        };
        if kind == ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI {
            for path in render_nodes() {
                match Self::create(kind, Some(&path)) {
                    Ok(device) => return Ok(device),
                    Err(e) => last = e,
                }
            }
        }
        Err(last)
    }

    /// Create one device, by name or with the driver's own default.
    #[tracing::instrument(skip_all)]
    fn create(kind: ffi::AVHWDeviceType, device: Option<&str>) -> Result<Self, io::Error> {
        let device = match device {
            Some(device) => Some(
                std::ffi::CString::new(device)
                    .map_err(|_| io::Error::other("a device path with a NUL in it"))?,
            ),
            None => None,
        };
        unsafe {
            let mut reference: *mut ffi::AVBufferRef = std::ptr::null_mut();
            let code = ffi::av_hwdevice_ctx_create(
                &mut reference,
                kind,
                device
                    .as_ref()
                    .map_or(std::ptr::null(), |device| device.as_ptr()),
                std::ptr::null_mut(),
                0,
            );
            if code < 0 {
                return Err(failure("creating the hardware device", code));
            }
            Ok(Self(reference))
        }
    }

    /// The reference to hand to a codec context.
    #[tracing::instrument(skip_all)]
    pub fn reference(&self) -> *mut ffi::AVBufferRef {
        self.0
    }
}

/// The DRM render nodes this machine exposes, in name order.
#[tracing::instrument(skip_all)]
fn render_nodes() -> Vec<String> {
    let mut nodes: Vec<String> = std::fs::read_dir("/dev/dri")
        .map(|entries| {
            entries
                .filter_map(|entry| Some(entry.ok()?.file_name().into_string().ok()?))
                .filter(|name| name.starts_with("renderD"))
                .map(|name| format!("/dev/dri/{name}"))
                .collect()
        })
        .unwrap_or_default();
    nodes.sort();
    nodes
}

impl Drop for Device {
    #[tracing::instrument(skip_all)]
    fn drop(&mut self) {
        unsafe { ffi::av_buffer_unref(&mut self.0) }
    }
}

// SAFETY: an AVBufferRef is atomically reference-counted (`av_buffer_ref` and
// `av_buffer_unref` are thread-safe), and owning the reference moves no other
// state with it.
unsafe impl Send for Device {}

/// An owned reference to a hardware frame pool (`AVHWFramesContext`).
pub struct Frames(*mut ffi::AVBufferRef);

impl Frames {
    /// A pool of `format` surfaces whose system-memory layout is `sw_format`.
    ///
    /// `pool` is how many surfaces may be alive at once; an encoder keeps a
    /// few references while it works, so the pool is generous.
    #[tracing::instrument(skip_all)]
    pub fn new(
        device: &Device,
        format: ffi::AVPixelFormat,
        sw_format: ffi::AVPixelFormat,
        width: u32,
        height: u32,
        pool: i32,
    ) -> Result<Self, io::Error> {
        unsafe {
            let reference = ffi::av_hwframe_ctx_alloc(device.reference());
            if reference.is_null() {
                return Err(io::Error::other("allocating a hardware frame pool failed"));
            }
            let context = (*reference).data as *mut ffi::AVHWFramesContext;
            (*context).format = format;
            (*context).sw_format = sw_format;
            (*context).width = width as i32;
            (*context).height = height as i32;
            (*context).initial_pool_size = pool;
            let code = ffi::av_hwframe_ctx_init(reference);
            if code < 0 {
                let mut reference = reference;
                ffi::av_buffer_unref(&mut reference);
                return Err(failure("initialising the hardware frame pool", code));
            }
            Ok(Self(reference))
        }
    }

    /// The reference to attach to an encoder's context.
    #[tracing::instrument(skip_all)]
    pub fn reference(&self) -> *mut ffi::AVBufferRef {
        self.0
    }
}

impl Drop for Frames {
    #[tracing::instrument(skip_all)]
    fn drop(&mut self) {
        unsafe { ffi::av_buffer_unref(&mut self.0) }
    }
}

// SAFETY: as for `Device`: the reference is atomically reference-counted and
// the pool it names is only touched through this owner.
unsafe impl Send for Frames {}

/// Upload the system-memory frame `cpu` into a fresh surface of `frames`.
#[tracing::instrument(skip_all)]
pub fn upload(frames: &Frames, cpu: &frame::Frame) -> Result<frame::Frame, io::Error> {
    let mut surface = unsafe { frame::Frame::empty() };
    unsafe {
        let code = ffi::av_hwframe_get_buffer(frames.reference(), surface.as_mut_ptr(), 0);
        if code < 0 {
            return Err(failure("allocating a hardware surface", code));
        }
        let code = ffi::av_hwframe_transfer_data(surface.as_mut_ptr(), cpu.as_ptr(), 0);
        if code < 0 {
            return Err(failure("uploading a picture to the hardware", code));
        }
    }
    Ok(surface)
}

/// Copy the hardware surface `surface` back to system memory as NV12.
#[tracing::instrument(skip_all)]
pub fn download(
    surface: &frame::Frame,
    width: u32,
    height: u32,
) -> Result<frame::Frame, io::Error> {
    let mut cpu = unsafe { frame::Frame::empty() };
    unsafe {
        let raw = cpu.as_mut_ptr();
        (*raw).format = ffi::AVPixelFormat::AV_PIX_FMT_NV12 as i32;
        (*raw).width = width as i32;
        (*raw).height = height as i32;
        let code = ffi::av_hwframe_transfer_data(cpu.as_mut_ptr(), surface.as_ptr(), 0);
        if code < 0 {
            return Err(failure("downloading a picture from the hardware", code));
        }
    }
    Ok(cpu)
}

/// A fresh system-memory NV12 picture of `width` x `height`.
#[tracing::instrument(skip_all)]
pub fn nv12(width: u32, height: u32) -> Result<frame::Frame, io::Error> {
    let mut picture = unsafe { frame::Frame::empty() };
    unsafe {
        let raw = picture.as_mut_ptr();
        (*raw).format = ffi::AVPixelFormat::AV_PIX_FMT_NV12 as i32;
        (*raw).width = width as i32;
        (*raw).height = height as i32;
        let code = ffi::av_frame_get_buffer(raw, 0);
        if code < 0 {
            return Err(failure("allocating a picture", code));
        }
    }
    Ok(picture)
}

/// Fill `picture`'s NV12 planes from tightly packed `y` and `uv` planes,
/// honouring the picture's own strides.
///
/// `uv` is the interleaved chroma plane at `2 * ceil(width / 2)` bytes per
/// row, `ceil(height / 2)` rows: the tight NV12 layout, which is what
/// `yuvutils-rs` hands out and takes back.
#[tracing::instrument(skip_all)]
pub fn fill_nv12(picture: &mut frame::Frame, y: &[u8], uv: &[u8], width: u32, height: u32) {
    unsafe {
        let raw = picture.as_mut_ptr();
        let y_stride = (*raw).linesize[0] as usize;
        let uv_stride = (*raw).linesize[1] as usize;
        let (w, h) = (width as usize, height as usize);
        let uv_row = w.div_ceil(2) * 2;
        for row in 0..h {
            std::ptr::copy_nonoverlapping(
                y[row * w..].as_ptr(),
                (*raw).data[0].add(row * y_stride),
                w,
            );
        }
        for row in 0..h.div_ceil(2) {
            std::ptr::copy_nonoverlapping(
                uv[row * uv_row..].as_ptr(),
                (*raw).data[1].add(row * uv_stride),
                uv_row,
            );
        }
    }
}

/// The Y and interleaved UV planes of an NV12 picture, copied tight.
///
/// The copy is what makes odd widths and padded strides disappear: the
/// conversion library requires chroma rows exactly `2 * ceil(width / 2)`
/// bytes wide, and a codec's `linesize` is padded to its own alignment.
/// Returns the planes and the chroma row width.
#[tracing::instrument(skip_all)]
pub fn nv12_planes(picture: &frame::Frame) -> Result<(Vec<u8>, Vec<u8>, u32), io::Error> {
    unsafe {
        let raw = picture.as_ptr();
        if (*raw).format != ffi::AVPixelFormat::AV_PIX_FMT_NV12 as i32 {
            return Err(io::Error::other(format!(
                "a decoded picture is {:?}, not NV12",
                (*raw).format
            )));
        }
        let width = (*raw).width as usize;
        let height = (*raw).height as usize;
        let y_stride = (*raw).linesize[0] as usize;
        let uv_stride = (*raw).linesize[1] as usize;
        let uv_row = width.div_ceil(2) * 2;
        let uv_height = height.div_ceil(2);
        let mut y = vec![0u8; width * height];
        let mut uv = vec![0u8; uv_row * uv_height];
        for row in 0..height {
            std::ptr::copy_nonoverlapping(
                (*raw).data[0].add(row * y_stride),
                y[row * width..].as_mut_ptr(),
                width,
            );
        }
        for row in 0..uv_height {
            std::ptr::copy_nonoverlapping(
                (*raw).data[1].add(row * uv_stride),
                uv[row * uv_row..].as_mut_ptr(),
                uv_row,
            );
        }
        Ok((y, uv, uv_row as u32))
    }
}

/// Whether `frame` lives on a hardware surface rather than in system memory.
#[tracing::instrument(skip_all)]
pub fn is_hardware(frame: &frame::Frame) -> bool {
    unsafe { !(*frame.as_ptr()).hw_frames_ctx.is_null() }
}

/// Ask an encoder to emit each picture before taking the next.
///
/// VAAPI's `async_depth` (default 2) lets the encoder hold the first pictures
/// back while it works ahead, which makes the opening frames of a stream
/// arrive late and empty. The control is set only when the encoder has one;
/// an encoder without it (V4L2 M2M) is already synchronous.
///
/// # Safety
///
/// `context` must be a live, unopened encoder context.
#[tracing::instrument(skip_all)]
pub unsafe fn emit_each_frame(context: *mut ffi::AVCodecContext) -> Result<(), io::Error> {
    unsafe {
        let name = c"async_depth";
        let option = ffi::av_opt_find((*context).priv_data, name.as_ptr(), std::ptr::null(), 0, 0);
        if option.is_null() {
            return Ok(());
        }
        let code = ffi::av_opt_set_int((*context).priv_data, name.as_ptr(), 1, 0);
        if code < 0 {
            return Err(failure("setting async_depth", code));
        }
        Ok(())
    }
}

/// Attach a frame pool to an encoder context that has not been opened yet.
///
/// # Safety
///
/// `context` must be a live, unopened codec context.
#[tracing::instrument(skip_all)]
pub unsafe fn set_frames(context: *mut ffi::AVCodecContext, frames: &Frames) {
    unsafe {
        (*context).hw_frames_ctx = ffi::av_buffer_ref(frames.reference());
    }
}

/// Attach a device to a decoder context that has not been opened yet.
///
/// # Safety
///
/// `context` must be a live, unopened codec context.
#[tracing::instrument(skip_all)]
pub unsafe fn set_device(context: *mut ffi::AVCodecContext, device: &Device) {
    unsafe {
        (*context).hw_device_ctx = ffi::av_buffer_ref(device.reference());
    }
}

/// Make a decoder accept only `format`, and nothing else.
///
/// The decoder asks [`get_format`] what to return; a format list without
/// `format` answers `AV_PIX_FMT_NONE`, which fails the decode. That is the
/// guarantee that no software picture is ever produced as a fallback.
///
/// # Safety
///
/// `context` must be a live, unopened codec context.
#[tracing::instrument(skip_all)]
pub unsafe fn demand(context: *mut ffi::AVCodecContext, format: ffi::AVPixelFormat) {
    unsafe {
        (*context).opaque = format as usize as *mut c_void;
        (*context).get_format = Some(get_format);
    }
}

/// The `get_format` callback installed by [`demand`].
///
/// # Safety
///
/// Called by libavcodec with a live context whose `opaque` is the demanded
/// format, and a `AV_PIX_FMT_NONE`-terminated list of formats.
#[tracing::instrument(skip_all)]
unsafe extern "C" fn get_format(
    context: *mut ffi::AVCodecContext,
    formats: *const ffi::AVPixelFormat,
) -> ffi::AVPixelFormat {
    unsafe {
        let wanted = (*context).opaque as usize;
        let mut format = formats;
        while *format != ffi::AVPixelFormat::AV_PIX_FMT_NONE {
            if *format as usize == wanted {
                return *format;
            }
            format = format.add(1);
        }
        ffi::AVPixelFormat::AV_PIX_FMT_NONE
    }
}
