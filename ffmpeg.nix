# The FFmpeg this workspace links: statically, so the binary carries the
# codec glue instead of loading libav* at run time. `--disable-everything`
# strips every software codec, muxer, demuxer, filter and protocol; only the
# hardware H.264 path is compiled back in:
#   - encoders: h264_vaapi (Intel/AMD), h264_nvenc (NVIDIA),
#     h264_v4l2m2m (embedded/ARM), h264_videotoolbox (macOS)
#   - decoders: h264_cuvid, h264_v4l2m2m, h264_videotoolbox, and the
#     native `h264` decoder, which is FFmpeg's VAAPI decode host (there is
#     no separate h264_vaapi decoder). It never runs software: the
#     `get_format` callback in crates/codecs refuses every format that is not
#     a VAAPI surface, so a machine without VAAPI fails the decode instead of
#     silently decoding in software.
# Patent note: what remains inside libavcodec is AVC/H.264 itself (the
# software AVC decoder above); everything else with an active licensing
# pool (HEVC, VVC/LCEVC/EVC, AVS, AMR, MP3, and the external x264/x265/
# OpenH264/LAME libraries) is not compiled in at all. AVC is the product's
# wire format and is always executed on the hardware block.
# `postFixup` is dropped because it exists only to patch shared libraries
# (vulkan add-needed, driver runpath) that a static-only build does not
# produce. `withVulkan` is off for the same reason: no Vulkan path is used,
# and its fixup cannot run against a static-only build.
pkgs:
(pkgs.ffmpeg.override {
  withStatic = true;
  withShared = false;
  withPic = true;
  withX264 = false;
  withX265 = false;
  withKvazaar = false; # HEVC
  withVvenc = false; # VVC
  withUavs3d = false; # AVS3 decoder
  withXavs = false; # AVS
  withXavs2 = false; # AVS2
  withXevd = false; # MPEG-5 EVC decoder
  withXeve = false; # MPEG-5 EVC encoder
  withOpenh264 = false; # software AVC, Cisco-licensed
  withOpencoreAmrnb = false; # AMR-NB
  withOpencoreAmrwb = false; # AMR-WB
  withVoAmrwbenc = false; # AMR-WB encoder
  withMp3lame = false; # MP3 encoder, not needed
  withVulkan = false;
  # VDPAU is X11-only; ffmpeg pulls in -lvdpau/-lX11 via its configure check,
  # and nixpkgs' generated .pc files carry neither the flag nor the -L path,
  # so the static link would fail to find them. VAAPI is the video path used.
  withVdpau = false;
}).overrideAttrs (old: {
  postFixup = "";
  # nixpkgs runs `make check`, which builds FFmpeg's in-tree test programs;
  # those unconditionally reference the very codecs --disable-everything
  # removes (e.g. libavfilter/tests/integral needs nlmeans).
  doCheck = false;
  configureFlags =
    old.configureFlags
    ++ [
      "--disable-everything"
      "--enable-decoder=h264"
      "--enable-parser=h264"
    ]
    ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux [
      "--enable-decoder=h264_cuvid"
      "--enable-decoder=h264_v4l2m2m"
      "--enable-encoder=h264_vaapi"
      "--enable-encoder=h264_nvenc"
      "--enable-encoder=h264_v4l2m2m"
      "--enable-hwaccel=h264_vaapi"
    ]
    ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isDarwin [
      "--enable-decoder=h264_videotoolbox"
      "--enable-encoder=h264_videotoolbox"
      "--enable-hwaccel=h264_videotoolbox"
    ];
})
