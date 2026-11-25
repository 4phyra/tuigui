# The FFmpeg this workspace links: statically, so the binary carries the
# codec glue instead of loading libav* at run time. `--disable-everything`
# strips every software codec, muxer, demuxer, filter and protocol; only the
# hardware AV1 path is compiled back in:
#   - encoders: av1_vaapi (Intel/AMD), av1_nvenc (NVIDIA)
#   - decoders: av1_cuvid, av1_videotoolbox (macOS), and the native `av1`
#     decoder, which is FFmpeg's VAAPI decode host (there is no separate
#     av1_vaapi decoder). It never runs software: the `get_format` callback in
#     crates/codecs refuses every format that is not a VAAPI surface, so a
#     machine without VAAPI fails the decode instead of silently decoding in
#     software.
# macOS has no av1_videotoolbox encoder in current FFmpeg, so AV1 encode is
# Linux-only for now; macOS streams JPEG. Verify at build time with
# `ffmpeg -hide_banner -encoders | grep -i av1`.
# Patent note: AV1 is royalty-free under the AOM Patent License 1.0 (see
# docs/licenses/AV1-PATENT-LICENSE). No AVC/H.264, HEVC, VVC/LCEVC/EVC, AVS,
# AMR or MP3 code is compiled in: no codec with an active licensing pool,
# and none of the external x264/x265/OpenH264/LAME libraries.
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
      "--enable-decoder=av1"
      "--enable-parser=av1"
    ]
    ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux [
      "--enable-decoder=av1_cuvid"
      "--enable-encoder=av1_vaapi"
      "--enable-encoder=av1_nvenc"
      "--enable-hwaccel=av1_vaapi"
    ]
    ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isDarwin [
      "--enable-decoder=av1_videotoolbox"
      "--enable-hwaccel=av1_videotoolbox"
    ];
})
