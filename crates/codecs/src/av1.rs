//! AV1, as one encoder and one decoder, on hardware only.
//!
//! No software AV1 encode or decode happens in a session: the picture work is
//! done by the silicon vendor's codec through the OS and driver. Encoders and
//! decoders are opened by name — `av1_vaapi`, `av1_nvenc`, `av1_cuvid`,
//! `av1_videotoolbox` — and the native `av1` decoder is used only as VAAPI's
//! decode host, where `get_format` refuses anything but hardware surfaces.
//! A machine with no hardware path fails loudly, naming `--jpeg`.
//!
//! Hardware is more than a GPU: an iGPU (VAAPI), a dGPU (NVENC / CUVID) or
//! Apple's media engine (VideoToolbox) all count. The probe opens candidate
//! codecs rather than looking for a GPU.
//!
//! The color conversions around the codec are `yuvutils-rs`: the capture
//! speaks B,G,R,A, the hardware speaks NV12, and that translation is not this
//! file's to hand-maintain.

use std::io;

use ffmpeg_next::ffi::{AVHWDeviceType, AVPictureType, AVPixelFormat};
use ffmpeg_next::format::Pixel;
use ffmpeg_next::{frame, Rational};
use yuvutils_rs::{
    bgra_to_yuv_nv12, yuv_nv12_to_rgb, BufferStoreMut, YuvBiPlanarImage, YuvBiPlanarImageMut,
    YuvConversionMode, YuvRange, YuvStandardMatrix,
};

use crate::hw;
use crate::{Error, Frame};

/// The stream's shape. The encoder runs at this and the wire assumes it.
///
// TODO: make these configurable — a session that wants a different framerate,
// bitrate or keyframe cadence has to be able to ask.
pub const FRAMERATE: f32 = 90.0;
pub const GOP: u32 = 30;
// Target about 1 MiB/s of stream: 4 Mb/s, because AV1 hw encode at 2 Mb/s left
// visible quality loss on text.
const BPS: u32 = 1000 * 1024 * 8;

/// How many hardware surfaces the encoder's pool holds at once. The encoder
/// keeps a few references while it works, so the pool outnumbers the pictures
/// this module ever holds.
const POOL: i32 = 20;

/// A hardware encoder this build may open, in probe order.
struct EncoderBackend {
    /// The name FFmpeg knows the encoder by.
    name: &'static str,
    /// The device that does the encoding and the pixel format of its surfaces.
    hardware: Option<(AVHWDeviceType, AVPixelFormat)>,
}

#[cfg(target_os = "linux")]
const ENCODERS: &[EncoderBackend] = &[
    EncoderBackend {
        name: "av1_vaapi",
        hardware: Some((
            AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI,
            AVPixelFormat::AV_PIX_FMT_VAAPI,
        )),
    },
    EncoderBackend {
        name: "av1_nvenc",
        hardware: Some((
            AVHWDeviceType::AV_HWDEVICE_TYPE_CUDA,
            AVPixelFormat::AV_PIX_FMT_CUDA,
        )),
    },
];

// Current FFmpeg ships no `av1_videotoolbox` encoder (see ffmpeg.nix), so macOS
// has no AV1 encoder to open and a macOS session streams JPEG.
#[cfg(target_os = "macos")]
const ENCODERS: &[EncoderBackend] = &[];

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
const ENCODERS: &[EncoderBackend] = &[];

/// A hardware decoder this build may open, in probe order.
struct DecoderBackend {
    /// The name FFmpeg knows the decoder by.
    name: &'static str,
    /// The device that does the decoding and the pixel format of its surfaces.
    hardware: Option<(AVHWDeviceType, AVPixelFormat)>,
    /// Whether the format has to be forced through `get_format`. VAAPI has no
    /// decoder of its own in FFmpeg: the native `av1` decoder does the work
    /// through a VAAPI device, and `get_format` is where it is made to refuse
    /// anything but the hardware surfaces. The standalone decoders choose
    /// their own output format.
    demand: bool,
}

#[cfg(target_os = "linux")]
const DECODERS: &[DecoderBackend] = &[
    // No FFmpeg build ships a decoder named `av1_vaapi`; VAAPI decode is the
    // native `av1` decoder driven through a VAAPI device. `demand` is what
    // keeps it hardware-only: a format list without VAAPI fails the decode
    // rather than silently decoding in software.
    DecoderBackend {
        name: "av1",
        hardware: Some((
            AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI,
            AVPixelFormat::AV_PIX_FMT_VAAPI,
        )),
        demand: true,
    },
    DecoderBackend {
        name: "av1_cuvid",
        hardware: Some((
            AVHWDeviceType::AV_HWDEVICE_TYPE_CUDA,
            AVPixelFormat::AV_PIX_FMT_CUDA,
        )),
        demand: false,
    },
];

#[cfg(target_os = "macos")]
const DECODERS: &[DecoderBackend] = &[DecoderBackend {
    name: "av1_videotoolbox",
    hardware: Some((
        AVHWDeviceType::AV_HWDEVICE_TYPE_VIDEOTOOLBOX,
        AVPixelFormat::AV_PIX_FMT_VIDEOTOOLBOX,
    )),
    demand: false,
}];

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
const DECODERS: &[DecoderBackend] = &[];

/// Encode B,G,R,A pixels into an AV1 OBU stream on the hardware.
pub struct Encoder {
    inner: ffmpeg_next::encoder::Video,
    /// The surface pool, and with it the device. `None` when the encoder takes
    /// system-memory NV12; no current backend does.
    frames: Option<hw::Frames>,
    width: u32,
    height: u32,
    pts: i64,
}

impl Encoder {
    /// Build the encoder for a stream of `width` x `height`, by opening the
    /// first hardware encoder this machine has.
    ///
    /// # Errors
    ///
    /// [`Error::HardwareUnavailable`] when no candidate opens: no hardware
    /// AV1 encoder on this machine, which is the designed failure and names
    /// `--jpeg`.
    #[cfg_attr(feature = "tracy", tracing::instrument(skip_all))]
    pub fn new(width: u32, height: u32) -> Result<Self, Error> {
        ffmpeg_next::init().map_err(Error::boxed)?;
        let mut tried = String::new();
        let mut reason = String::from("this build carries no hardware AV1 encoders");
        for backend in ENCODERS {
            match Self::open(backend, width, height) {
                Ok(encoder) => return Ok(encoder),
                Err(e) => {
                    if !tried.is_empty() {
                        tried.push_str(", ");
                    }
                    tried.push_str(backend.name);
                    reason = e.to_string();
                }
            }
        }
        Err(Error::HardwareUnavailable {
            direction: "encoder",
            tried,
            reason,
        })
    }

    /// Open one candidate, with its device, pool and codec context.
    #[cfg_attr(feature = "tracy", tracing::instrument(skip_all))]
    fn open(backend: &EncoderBackend, width: u32, height: u32) -> Result<Self, io::Error> {
        let codec = ffmpeg_next::encoder::find_by_name(backend.name).ok_or_else(|| {
            io::Error::other(format!("{} is not built into this ffmpeg", backend.name))
        })?;
        let (frames, format) = match backend.hardware {
            Some((device_type, format)) => {
                let device = hw::Device::open(device_type)?;
                let frames = hw::Frames::new(
                    &device,
                    format,
                    AVPixelFormat::AV_PIX_FMT_NV12,
                    coded(width),
                    coded(height),
                    POOL,
                )?;
                (Some(frames), Pixel::from(format))
            }
            None => (None, Pixel::NV12),
        };
        let mut video = ffmpeg_next::codec::Context::new_with_codec(codec)
            .encoder()
            .video()
            .map_err(io::Error::other)?;
        video.set_width(coded(width));
        video.set_height(coded(height));
        video.set_format(format);
        video.set_time_base(Rational::new(1, FRAMERATE as i32));
        video.set_frame_rate(Some(Rational::new(FRAMERATE as i32, 1)));
        video.set_gop(GOP);
        video.set_max_b_frames(0);
        video.set_bit_rate(BPS as usize);
        // What is fed in is BT.601 full range, so what comes out says so: the
        // decoder on the other side reads these same values back.
        video.set_colorspace(ffmpeg_next::color::Space::SMPTE170M);
        video.set_color_range(ffmpeg_next::color::Range::JPEG);
        if let Some(frames) = &frames {
            // SAFETY: `video` is a live context and has not been opened.
            unsafe { hw::set_frames(video.as_mut_ptr(), frames) };
        }
        // SAFETY: as above: the context is live and not yet opened.
        unsafe { hw::emit_each_frame(video.as_mut_ptr())? };
        let inner = video.open_as(codec).map_err(io::Error::other)?;
        Ok(Self {
            inner,
            frames,
            width,
            height,
            pts: 0,
        })
    }
}

impl crate::Encoder for Encoder {
    #[cfg_attr(feature = "tracy", tracing::instrument(name = "codec.encode", skip_all))]
    fn encode(
        &mut self,
        bgra: &[u8],
        width: u32,
        height: u32,
        force_keyframe: bool,
    ) -> Result<Vec<u8>, Error> {
        assert!(
            bgra.len() == width as usize * height as usize * 4,
            "a capture is {}x{}x4 B,G,R,A, and this one is {} bytes",
            width,
            height,
            bgra.len()
        );
        if (width, height) != (self.width, self.height) {
            // A stream describes pictures of one size; a resized window is a
            // new stream. The fresh parameter sets and opening IDR are what
            // let the decoder on the far side follow the change.
            *self = Self::new(width, height)?;
        }
        let (coded_width, coded_height) = (coded(width), coded(height));
        let padded;
        let source = if (coded_width, coded_height) == (width, height) {
            bgra
        } else {
            padded = pad_bgra(bgra, width, height, coded_width, coded_height);
            &padded[..]
        };
        let (y, uv) = bgra_to_nv12(source, coded_width, coded_height)?;
        let mut cpu = hw::nv12(coded_width, coded_height).map_err(encode_error(width, height))?;
        hw::fill_nv12(&mut cpu, &y, &uv, coded_width, coded_height);
        let mut picture = match &self.frames {
            Some(frames) => hw::upload(frames, &cpu).map_err(encode_error(width, height))?,
            None => cpu,
        };
        // SAFETY: `picture` is this function's live frame, not yet handed to
        // the codec, and these are plain AVFrame fields.
        unsafe {
            let raw = picture.as_mut_ptr();
            (*raw).pts = self.pts;
            if force_keyframe {
                // The documented way to force a keyframe at encode time.
                (*raw).pict_type = AVPictureType::AV_PICTURE_TYPE_I;
            }
        }
        self.pts += 1;
        {
            // The hand-over to the hardware codec, and the wait for what it has ready: the two
            // calls an AV1 frame is actually spent in, so each gets its own zone.
            let _send = tracing::info_span!("codec.send_frame").entered();
            self.inner
                .send_frame(&picture)
                .map_err(encode_error(width, height))?;
        }
        let mut out = Vec::new();
        let _recv = tracing::info_span!("codec.receive_packet").entered();
        loop {
            let mut packet = ffmpeg_next::Packet::empty();
            match self.inner.receive_packet(&mut packet) {
                Ok(()) => {
                    if let Some(data) = packet.data() {
                        out.extend_from_slice(data);
                    }
                }
                Err(e) if quiet(e) => break,
                Err(e) => return Err(encode_error(width, height)(e)),
            }
        }
        Ok(out)
    }
}

/// Decode an AV1 OBU stream back into pixels, on the hardware.
///
/// A decoder that has decoded at least one picture never answers with nothing:
/// a stretch of stream that carries no new picture (headers, or a frame the
/// codec held back) is answered with the last picture, which is what the pane
/// is already showing and what a repaint wants anyway.
pub struct Decoder {
    inner: ffmpeg_next::decoder::Video,
    /// The last picture decoded, which is the answer to a stretch of stream
    /// that produced nothing new. `None` only before the first picture.
    last: Option<Frame>,
}

impl Decoder {
    /// Build the decoder by opening the first hardware decoder this machine
    /// has. It learns the stream's shape from the first headers, which is why
    /// one is built before any frame arrives.
    ///
    /// # Errors
    ///
    /// [`Error::HardwareUnavailable`] when no candidate opens: no hardware
    /// AV1 decoder on this machine, which is the designed failure and names
    /// `--jpeg`.
    #[cfg_attr(feature = "tracy", tracing::instrument(skip_all))]
    pub fn new() -> Result<Self, Error> {
        ffmpeg_next::init().map_err(Error::boxed)?;
        let mut tried = String::new();
        let mut reason = String::from("this build carries no hardware AV1 decoders");
        for backend in DECODERS {
            match Self::open(backend) {
                Ok(decoder) => return Ok(decoder),
                Err(e) => {
                    if !tried.is_empty() {
                        tried.push_str(", ");
                    }
                    tried.push_str(backend.name);
                    reason = e.to_string();
                }
            }
        }
        Err(Error::HardwareUnavailable {
            direction: "decoder",
            tried,
            reason,
        })
    }

    /// Open one candidate, with its device and codec context.
    #[cfg_attr(feature = "tracy", tracing::instrument(skip_all))]
    fn open(backend: &DecoderBackend) -> Result<Self, io::Error> {
        let codec = ffmpeg_next::decoder::find_by_name(backend.name).ok_or_else(|| {
            io::Error::other(format!("{} is not built into this ffmpeg", backend.name))
        })?;
        let device = match backend.hardware {
            Some((device_type, _)) => Some(hw::Device::open(device_type)?),
            None => None,
        };
        let mut context = ffmpeg_next::codec::Context::new_with_codec(codec);
        if let (Some(device), Some((_, format))) = (&device, backend.hardware) {
            // SAFETY: `context` is live and unopened; the device outlives the
            // open because the context takes its own reference.
            unsafe {
                hw::set_device(context.as_mut_ptr(), device);
                if backend.demand {
                    hw::demand(context.as_mut_ptr(), format);
                }
            }
        }
        let inner = context.decoder().video().map_err(io::Error::other)?;
        Ok(Self { inner, last: None })
    }

    /// The picture `surface` carries, as R,G,B.
    #[cfg_attr(feature = "tracy", tracing::instrument(skip_all))]
    fn pixels(surface: &frame::Frame, len: usize) -> Result<Frame, Error> {
        let width = unsafe { (*surface.as_ptr()).width as u32 };
        let height = unsafe { (*surface.as_ptr()).height as u32 };
        let cpu;
        let nv12 = if hw::is_hardware(surface) {
            cpu = hw::download(surface, width, height).map_err(|e| Error::Decode {
                len,
                source: Box::new(e),
            })?;
            &cpu
        } else {
            surface
        };
        let (y, uv, uv_stride) = hw::nv12_planes(nv12).map_err(|e| Error::Decode {
            len,
            source: Box::new(e),
        })?;
        let image = YuvBiPlanarImage {
            y_plane: &y,
            y_stride: width,
            uv_plane: &uv,
            uv_stride,
            width,
            height,
        };
        let mut rgb = vec![0u8; width as usize * height as usize * 3];
        yuv_nv12_to_rgb(
            &image,
            &mut rgb,
            width * 3,
            YuvRange::Full,
            YuvStandardMatrix::Bt601,
            YuvConversionMode::Balanced,
        )?;
        Ok(Frame { width, height, rgb })
    }
}

impl crate::Decoder for Decoder {
    #[cfg_attr(feature = "tracy", tracing::instrument(name = "codec.decode", skip_all))]
    fn decode(&mut self, stream: &[u8]) -> Result<Option<Frame>, Error> {
        if !stream.is_empty() {
            let _send = tracing::info_span!("codec.send_packet").entered();
            let packet = ffmpeg_next::Packet::copy(stream);
            self.inner.send_packet(&packet).map_err(|e| Error::Decode {
                len: stream.len(),
                source: Box::new(e),
            })?;
        }
        // SAFETY: a fresh frame, owned by this call.
        let mut surface = unsafe { frame::Frame::empty() };
        let _recv = tracing::info_span!("codec.receive_frame").entered();
        match self.inner.receive_frame(&mut surface) {
            Ok(()) => {
                let picture = Self::pixels(&surface, stream.len())?;
                self.last = Some(picture.clone());
                Ok(Some(picture))
            }
            // Nothing new in this stretch of stream, and something already
            // shown: the last picture is the answer.
            Err(e) if quiet(e) => Ok(self.last.clone()),
            Err(e) => Err(Error::Decode {
                len: stream.len(),
                source: Box::new(e),
            }),
        }
    }
}

/// Whether `error` means "no more pictures right now" rather than a failure.
#[cfg_attr(feature = "tracy", tracing::instrument(skip_all))]
fn quiet(error: ffmpeg_next::Error) -> bool {
    matches!(error, ffmpeg_next::Error::Eof)
        || matches!(
            error,
            ffmpeg_next::Error::Other { errno } if errno == ffmpeg_next::error::EAGAIN
        )
}

/// The size the codec is opened at. AV1's 4:2:0 needs even dimensions, and a
/// driver handed an odd one loses the whole GPU context instead of refusing it.
#[cfg_attr(feature = "tracy", tracing::instrument(skip_all))]
fn coded(size: u32) -> u32 {
    size + size % 2
}

/// `bgra` at `width` x `height`, padded to the coded even size by repeating the
/// last column and row, so an odd capture can be encoded as an even picture.
#[cfg_attr(feature = "tracy", tracing::instrument(skip_all))]
fn pad_bgra(bgra: &[u8], width: u32, height: u32, coded_width: u32, coded_height: u32) -> Vec<u8> {
    let (w, h) = (width as usize, height as usize);
    let (cw, ch) = (coded_width as usize, coded_height as usize);
    let mut out = vec![0u8; cw * ch * 4];
    for y in 0..ch {
        let source = &bgra[y.min(h - 1) * w * 4..][..w * 4];
        let row = &mut out[y * cw * 4..][..cw * 4];
        row[..w * 4].copy_from_slice(source);
        for x in w..cw {
            row[x * 4..][..4].copy_from_slice(&source[(w - 1) * 4..][..4]);
        }
    }
    out
}

/// An encode failure at a known size, as the error the caller sees.
#[cfg_attr(feature = "tracy", tracing::instrument(skip_all))]
fn encode_error<E>(width: u32, height: u32) -> impl FnOnce(E) -> Error
where
    E: std::error::Error + Send + Sync + 'static,
{
    move |e| Error::Encode {
        width,
        height,
        source: Box::new(e),
    }
}

/// B,G,R,A capture pixels into tightly packed NV12 planes.
///
/// The capture is a Wayland `Argb8888` buffer copied byte for byte, so its
/// bytes are B, G, R, A on a little-endian machine, which is exactly the input
/// the conversion library's BGRA path wants. The color math and the 4:2:0
/// subsample are its SIMD code, not ours.
#[cfg_attr(feature = "tracy", tracing::instrument(name = "codec.bgra_to_nv12", skip_all))]
fn bgra_to_nv12(bgra: &[u8], width: u32, height: u32) -> Result<(Vec<u8>, Vec<u8>), Error> {
    let (w, h) = (width as usize, height as usize);
    // The tight NV12 layout, including the odd-width chroma row padded to a
    // pair and the odd-height chroma row rounded up.
    let uv_stride = width.div_ceil(2) * 2;
    let uv_height = height.div_ceil(2);
    let mut image = YuvBiPlanarImageMut {
        y_plane: BufferStoreMut::Owned(vec![0u8; w * h]),
        y_stride: width,
        uv_plane: BufferStoreMut::Owned(vec![0u8; uv_stride as usize * uv_height as usize]),
        uv_stride,
        width,
        height,
    };
    bgra_to_yuv_nv12(
        &mut image,
        bgra,
        width * 4,
        YuvRange::Full,
        YuvStandardMatrix::Bt601,
        YuvConversionMode::Balanced,
    )?;
    // The planes were constructed owned, so what came back is the buffer.
    let (y, uv) = match (image.y_plane, image.uv_plane) {
        (BufferStoreMut::Owned(y), BufferStoreMut::Owned(uv)) => (y, uv),
        _ => unreachable!("the planes were constructed owned"),
    };
    Ok((y, uv))
}

#[cfg(test)]
#[path = "../tests/unit/av1.rs"]
mod tests;
