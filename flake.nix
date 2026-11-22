{
  description = "tuigui: GUI apps in the terminal — headless Wayland head + TGP streaming";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/20b1ddd1aa5ace70c9468305030aa4f9ef79671b";
    # Binary rust distribution (rustup profiles) instead of nixpkgs' from-source
    # rustc: ~1.2 GB devshell closure instead of ~2 GB.
    rust-overlay.url = "github:oxalica/rust-overlay/84fb3c79e477d5e7ec4eff9c24dbc5d5c99b0fd1";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";
    # blaze: the incremental build engine, added as an input now so the pin and its
    # lock are reviewed once. Not placed in the dev shell yet: as of v0.20261105.4
    # upstream's `packages.default` does not build (its `dist` derivation still calls
    # the retired `build_release.py` cargo recipe), so adding it would make the shell
    # unbuildable. `blaze.packages.${system}.default` goes into the shell's packages
    # once that builds.
    blaze.url = "github:indexable-inc/blaze";
  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
      blaze,
    }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems f;
      pkgsFor =
        system:
        import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };
    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = pkgsFor system;
          # Same binary toolchain as the dev shell (upstream dist tarballs,
          # rustc+cargo+rust-std in one prefix) wired into buildRustPackage
          # via makeRustPlatform, instead of nixpkgs' from-source rustc.
          toolchain = pkgs.rust-bin.stable.latest.minimal;
          rustPlatform = pkgs.makeRustPlatform {
            cargo = toolchain;
            rustc = toolchain;
          };

          # buildRustPackage splits the build into a `cargoDeps` derivation
          # hashed only on Cargo.lock plus the app derivation itself, so
          # dependency objects stay in the store across rebuilds that only
          # touch src/.
          #
          # The workspace built once, for the two halves this flake installs:
          # the server and the client. The `tuigui` wrapper is not among them,
          # which is why `cargoBuildFlags` names the two packages.
          tuiguiBin = rustPlatform.buildRustPackage {
            pname = "tuigui";
            version = "0.1.0";
            src = pkgs.lib.cleanSourceWith {
              src = ./.;
              filter =
                path: type:
                !pkgs.lib.hasInfix ".crush" path && path != toString ./result && path != toString ./target;
            };

            cargoLock.lockFile = ./Cargo.lock;
            auditable = false;

            # The suite streams real terminals: it looks for `foot` and `kitty` on
            # PATH and fails, by design, when they are absent. Those live in the
            # dev shell, and a sandboxed build has neither them nor a terminal to
            # draw on, so the tests are the shell's job (`just test`), not this
            # derivation's.
            doCheck = false;

            # `cargo build` at the workspace root builds only the root package's
            # binary. The two halves are separate members, so they are named.
            cargoBuildFlags = [
              "--package"
              "tui"
              "--package"
              "gui"
            ];

            nativeBuildInputs = with pkgs; [
              pkg-config
              wayland-scanner
              # bindgen (ffmpeg-sys-next) dlopens libclang at build time
              libclang
            ];
            # bindgen needs to be told where libclang lives and where the C
            # library headers are; neither is in a system search path on NixOS.
            LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";
            BINDGEN_EXTRA_CLANG_ARGS = "-isystem ${pkgs.glibc.dev}/include";
            buildInputs = with pkgs; [
              wayland
              wayland-protocols
              # system libs the compositor crate (in-process smithay head)
              # links against: pixman renderer + xkbcommon keymaps
              pixman
              libxkbcommon
              # X11 apps run through this, since it is what the compositor
              # spawns to serve them; without it a session is Wayland-only.
              xwayland
              # SIMD JPEG encoding for the streaming client. Nix's build already
              # has the SIMD kernels, so it is linked via pkg-config rather than
              # compiled from source: no cmake or nasm needed.
              libjpeg_turbo
              # The FFmpeg linked into the client and server: static libraries,
              # built without the software H.264 encoder.
              (import ./ffmpeg.nix pkgs)
              # ...and the CLI on PATH, because `ffmpeg-sidecar` spawns it to
              # decode the `--video` clip; that is a separate process's codec,
              # not ours. Its `.bin` output keeps its shared libraries' pkg-config
              # out of this build.
              pkgs.ffmpeg.bin
            ];

            meta = with pkgs.lib; {
              description = "GUI apps in the terminal with pixel-accurate TGP streaming";
              license = licenses.eupl12;
              platforms = platforms.linux;
            };
          };

          # `tui!` and `gui!`: the two halves as commands. The bang is part of
          # the installed file name — installed, not linked, so `$out/bin` holds
          # the name it reports — and it keeps the command clear of a bare `tui`
          # or `gui` the user's PATH may already answer.
          tuiPkg = pkgs.runCommand "tui" {
            meta = {
              description = "Run a command in a pty and paint the frames sent to it";
              license = pkgs.lib.licenses.eupl12;
              platforms = pkgs.lib.platforms.linux;
              mainProgram = "tui!";
            };
          } ''
            install -Dm755 ${tuiguiBin}/bin/tui $out/bin/tui!
          '';

          guiPkg = pkgs.runCommand "gui" {
            meta = {
              description = "Draw a runnable into the pane a tui server owns";
              license = pkgs.lib.licenses.eupl12;
              platforms = pkgs.lib.platforms.linux;
              mainProgram = "gui!";
            };
          } ''
            install -Dm755 ${tuiguiBin}/bin/gui $out/bin/gui!
          '';

          # What an install gets: both halves, and nothing else. No wrapper and
          # no `mainProgram`, so with two programs in `bin/` a `nix run` on the
          # flake itself refuses rather than picking one of them; `#tui` and
          # `#gui` are the ways in.
          defaultPkg = pkgs.runCommand "tuigui" { } ''
            install -Dm755 ${tuiguiBin}/bin/tui $out/bin/tui!
            install -Dm755 ${tuiguiBin}/bin/gui $out/bin/gui!
          '';
        in
        {
          # termlens' command line, which the snapshot review calls: `termlens
          # diff` paints the cells that changed between two saved screens, and
          # `termlens render --ansi` is one of them whole. nixpkgs has no package
          # for it, so it is built from the published crate; the dev shell puts
          # the binary on PATH, since `cargo install` would write outside the
          # shell and outside the fingerprints the shell is made against.
          termlens-cli = rustPlatform.buildRustPackage {
            pname = "termlens-cli";
            version = "0.11.9";
            src = pkgs.fetchCrate {
              pname = "termlens-cli";
              version = "0.11.9";
              hash = "sha256-guqUNl6054IK3EBANqEyQBT6NLgDLY5njE5IiuJ7aIQ=";
            };
            cargoHash = "sha256-ytVdfI+kRC+nY1qRT4bOpQNYbRvbgn0vMP7mgU+xBj8=";
          };

          # Public-domain (1887 Eadweard Muybridge) capybara clip, transcoded
          # from Wikimedia's WebM to a YUV420P H.264 MP4 (even-dims) so any
          # consumer can decode it with baseline ffmpeg. Stable URL, no
          # attribution or reuse restrictions.
          # capyVideo — sample capybara video for the `--video` streaming path.
          #
          # Source: "Animal Locomotion Pl.746 - Capybara Walking" (1887), a
          # manually reconstructed animation of Eadweard Muybridge's walk-cycle
          # photographs (photos 1-9 of Plate 746).
          #   - Project homepage: https://commons.wikimedia.org/wiki/File:Animal_Locomotion_Pl.746_-_Capybara_Walking.webm
          #   - Full history / background article (the 1887 Animal Locomotion
          #     plate series this clip came from):
          #     https://en.wikipedia.org/wiki/Animal_Locomotion_(Muybridge)
          #   - License: Public domain (PD-old-100 + PD-Mark; no attribution,
          #     no reuse restrictions). Not used on any wiki project page —
          #     only hosted on Wikimedia Commons.
          #   - Direct file URL (stable, permanent): the fetchurl below.
          #   - Format: 1s WebM (VP9), 1693x1221, ~1.2 MB. Transcoded to
          #     H.264 MP4 here because the source has ODD pixel dimensions
          #     (1693x1221), which baseline decoders/encoders reject; the
          #     scale filter rounds to even dims (1692x1220), YUV420P, audio
          #     dropped. ~1 MB output.
          #   - Exposed to the dev shell as `CAPY_VID_PATH`.
          capyVideo = pkgs.stdenv.mkDerivation {
            pname = "capy-video";
            version = "0.1.0";
            src = pkgs.fetchurl {
              url = "https://upload.wikimedia.org/wikipedia/commons/e/e9/Animal_Locomotion_Pl.746_-_Capybara_Walking.webm";
              sha256 = "sha256-y85/twSZdOqqEmW22vlN3p8Cod5zvmbMyX+K2KT+xwg=";
            };
            nativeBuildInputs = with pkgs; [ ffmpeg ];
            buildCommand = ''
              mkdir -p $out
              ffmpeg -y -loglevel error -i $src \
                -vf "scale=trunc(iw/2)*2:trunc(ih/2)*2" \
                -c:v libx264 -pix_fmt yuv420p -an $out/capybara.mp4
            '';
          };

          # Capybara photos, from the freds0/capybara_dataset repo: the image
          # set the dummy windowing path picks one of at random for its
          # background. Pinned to the repo's last commit (2021-12).
          # Exposed to the dev shell as `CAPYBARA_IMG_DIR` (the train images).
          capyImages = pkgs.stdenv.mkDerivation {
            pname = "capybara-images";
            version = "0.1.0";
            src = pkgs.fetchFromGitHub {
              owner = "freds0";
              repo = "capybara_dataset";
              rev = "f6ba28f22064176c574629ea5a7cc2354acbcc78";
              hash = "sha256-tFdjk6XVrzQ1VG4GAJnzwfpHws9jbzUj2Jd/v9+zKoA=";
            };
            installPhase = ''
              mkdir -p $out
              cp -r $src/images/train $out/train
            '';
          };

          # revdiff — TUI diff-review tool used by the repo's Crush skill
          # (.agents/skills/revdiff). Not packaged in this nixpkgs pin, so
          # fetch the upstream static release tarball (hashes match the
          # published checksums.txt for v1.13.0).
          revdiff =
            let
              srcs = {
                x86_64-linux = {
                  url = "https://github.com/umputun/revdiff/releases/download/v1.13.0/revdiff_1.13.0_linux_amd64.tar.gz";
                  sha256 = "sha256-bML8vcsuhlpwY5RfBzX2mRb2jMIaiGhUIXIzx5PttMc=";
                };
                aarch64-linux = {
                  url = "https://github.com/umputun/revdiff/releases/download/v1.13.0/revdiff_1.13.0_linux_arm64.tar.gz";
                  sha256 = "sha256-QEkq6zalfxDCvKGlzJX66GnXI2v7BO+32dHD2ChRr/U=";
                };
              };
            in
            pkgs.stdenv.mkDerivation {
              pname = "revdiff";
              version = "1.13.0";
              src = pkgs.fetchurl srcs.${system};
              sourceRoot = ".";
              installPhase = ''
                install -Dm755 revdiff $out/bin/revdiff
              '';
              meta = with pkgs.lib; {
                description = "Terminal UI for reviewing and annotating diffs";
                license = licenses.mit;
                platforms = platforms.linux;
                mainProgram = "revdiff";
              };
            };

          # Installed by `nix profile add`: both halves, under their bang
          # names, and no wrapper. No `mainProgram`, so a bare `nix run` on the
          # flake has no program to pick and refuses; `#tui` and `#gui` are the
          # ways in.
          default = defaultPkg;

          # `nix run .#tui` / `.#gui`: one half each. The attribute is named for
          # the flake's vocabulary, the program it runs for what is installed.
          tui = tuiPkg;
          gui = guiPkg;
        }
      );

      devShells = forAllSystems (
        system:
        let
          pkgs = pkgsFor system;
          # Capybara demo video, pinned to the capyVideo package above.
          # `CAPY_VID_PATH` points at the transcoded H.264 MP4 so dev tools
          # can stream it with `tuigui --video "$CAPY_VID_PATH"` without
          # handling licensing or re-encoding themselves.
          capy =
            self.packages.${system}.capyVideo or (pkgs.runCommand "capy-video" { }
              "mkdir -p $out; touch $out/capybara.mp4; echo 'no capybara package; build default first' >&2"
            );
          # Capybara demo images, pinned to the capyImages package above.
          # `CAPYBARA_IMG_DIR` points at the train images so the dummy
          # windowing path can pick one of them at random.
          capys =
            self.packages.${system}.capyImages or (pkgs.runCommand "capybara-images" { }
              "mkdir -p $out/train; echo 'no capybara images package; build default first' >&2"
            );
        in
        {
          default = pkgs.mkShell {
            # bindgen (ffmpeg-sys-next) dlopens libclang at build time and must
            # be told where the C library headers are; neither is in a system
            # search path on NixOS.
            LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";
            BINDGEN_EXTRA_CLANG_ARGS = "-isystem ${pkgs.glibc.dev}/include";
            shellHook = ''
              export CAPY_VID_PATH="${capy}/capybara.mp4"
              export CAPYBARA_IMG_DIR="${capys}/train"
            '';
            packages = [
              # rustup "minimal" profile (rustc, cargo, rust-std) plus the two
              # dev tools; binaries come from upstream's dist tarballs.
              (pkgs.rust-bin.stable.latest.minimal.override {
                extensions = [
                  "clippy"
                  "rustfmt"
                ];
              })
              pkgs.pkg-config
              pkgs.rust-analyzer
              # `just`, for the recipes in the justfile: building and running the
              # three binaries is a fixed set of invocations, and a shell script
              # would be the same thing with no documentation.
              pkgs.just
              # The test runner: one process per test, so a pty child or a raw-mode change
              # cannot leak into a sibling, and per-test timings whose slow ones are named
              # rather than buried in the output. Here and not `cargo install`, which would
              # put it outside this shell and outside the fingerprints it is used against.
              pkgs.cargo-nextest
              pkgs.tokei
              pkgs.slint-viewer
              # `just test-record`: the review half of the snapshot loop. A changed
              # screen waits as a `.snap.new` until it has been looked at, and this is
              # the review that shows the old and new pictures one at a time; writing
              # the files outright would record every frame unseen. Here and not
              # `cargo install` for the same reason as nextest: the shell owns the
              # version the fingerprints are made against.
              pkgs.cargo-insta
              # `just test-record`: the picture that review shows. `termlens
              # diff` paints the cells that changed between the old and the new
              # saved screen, and `termlens render --ansi` is one of them whole.
              # Built from the published crate in `packages` above, since
              # nixpkgs has no package for it.
              self.packages.${system}.termlens-cli
              pkgs.wayland
              pkgs.wayland-protocols
              pkgs.wayland-scanner
              # libs the compositor crate (in-process smithay head) links against
              pkgs.pixman
              pkgs.libxkbcommon
              # X11 apps: the compositor spawns this to serve them, so an X11-only
              # app (xeyes, glxgears) can be run in a session.
              pkgs.xwayland
              # SIMD JPEG encoding: linked against Nix's libjpeg-turbo, so it
              # only needs to be on the linker/pkg-config path.
              pkgs.libjpeg_turbo
              # demo apps to stream + TGP test terminals
              pkgs.foot
              pkgs.kitty
              # wayland-native demo clients for streaming tests. Note that the
              # EGL ones (weston-flower, weston-terminal) render through
              # dma-buf, which the compositor's shm-only capture cannot read, so
              # they capture black; the shm clients and `foot` do not.
              pkgs.weston
              # The FFmpeg the codec links: static libraries without the software
              # H.264 encoder. Also gives the dev shell the headers and .pc files
              # that `ffmpeg-next` builds against.
              (import ./ffmpeg.nix pkgs)
              # The CLI for the `--video` path: `ffmpeg-sidecar` spawns it, so it
              # is a separate process's decoder. Its `.bin` output keeps the
              # shared build's pkg-config from shadowing the static one.
              pkgs.ffmpeg.bin
              # X11 side: xeyes + glxgears, plus a virtual X server and input
              # driver for the Xvfb capture path
              pkgs.xeyes
              pkgs.mesa-demos # glxgears — classic X11/GL gears test app
              pkgs.xvfb-run
              pkgs.xdotool
              # profiler GUI the client half streams its spans to
              pkgs.tracy
              # diff-review TUI invoked by .agents/skills/revdiff (binary
              # release, defined in packages above)
              self.packages.${system}.revdiff
            ];
          };
        }
      );
    };
}
