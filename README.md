<p align="center">
  <img src="assets/tuigui-10x.png" alt="tuigui" width="340" height="110">
</p>

<p align="center">
  <a href="https://github.com/4phyra/tuigui"><img src="https://img.shields.io/endpoint?url=https%3A%2F%2Fraw.githubusercontent.com%2F4phyra%2Ftuigui%2Fmaster%2F.github%2Ftokei.json" alt="Rust lines of code"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-EUPL--1.2-blue.svg" alt="License: EUPL-1.2"></a>
  <a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/rust-1.88%2B-orange.svg" alt="Rust: 1.88+"></a>
  <a href="https://conventionalcommits.org"><img src="https://img.shields.io/badge/Conventional%20Commits-1.0.0-%23FE5196?logo=conventionalcommits&logoColor=white" alt="Conventional Commits"></a>
</p>

## What is tuigui?

tuigui puts graphical programs in the terminal, alongside the text tools you
already use. You give it a program; it gives you that program's window and
video in the pane, through the kitty graphics protocol, while keeping the
terminal's normal input and output intact. The same split between the terminal
side and the program side is intended to let a program run on another machine
and still be used from your terminal.

## Installation

`tui!` and `gui!` are the two halves of a session: the server that owns your
terminal and the client that draws into it. The bang keeps them clear of a
`tui` or `gui` your PATH may already answer.

```sh
# Added to your profile: both halves, on PATH.
nix profile add github:4phyra/tuigui

# Or one half at a time, without touching your profile.
nix run github:4phyra/tuigui#tui -- <command>
nix run github:4phyra/tuigui#gui -- <program>
```

Bare `nix run github:4phyra/tuigui` refuses on purpose: the package holds two
programs and there is no single one to pick.

## Usage

A local session is the server, with the client as the command it runs:

```sh
tui! gui! foot
```

The server owns the pane, `gui!` draws `foot`'s windows into it, and the picture
comes through the kitty graphics protocol.

A remote session splits the two ends. The server wraps the command that reaches
the other machine, and the client that runs there draws the program:

```sh
# where the terminal is: the server owns the pane, the command is an ssh away
tui! ssh host gui! foot
```

The client announces itself in the pane's output and the server dials it back
over `p2p`, so no port has to be forwarded. The terminal must speak the kitty
graphics protocol, and the program must be an X11 or Wayland app: the session
puts it on its own headless Wayland display. `--channel p2p` is the default and
the only one that crosses machines; `--channel unix` is the cheaper one when
both ends share a host.

## Contributing

The dev shell carries the toolchain and the system libraries, so development
happens inside it:

```sh
nix develop
just tui <command>   # the server, built from the working tree
just gui <program>   # the client
just test            # nextest; changed screens wait for `just test-review`
```

## Thanks

Built on excellent crates:

<p>
  <a href="https://github.com/tokio-rs/tokio"><img src="https://img.shields.io/badge/tokio-async%20runtime-2b2b2b?logo=tokio&logoColor=white" alt="tokio: async runtime"></a>
  <a href="https://github.com/ratatui/ratatui"><img src="https://img.shields.io/badge/ratatui-widgets%20and%20layout-2b2b2b?logo=ratatui&logoColor=white" alt="ratatui: widgets and layout"></a>
  <a href="https://github.com/Smithay/smithay"><img src="https://img.shields.io/badge/smithay-Wayland%20compositor-2b2b2b" alt="smithay: the Wayland compositor"></a>
  <a href="https://github.com/crossterm-rs/crossterm"><img src="https://img.shields.io/badge/crossterm-terminal%20I%2FO-2b2b2b" alt="crossterm: terminal I/O"></a>
  <a href="https://github.com/helix-editor/termina"><img src="https://img.shields.io/badge/termina-input%20parsing-2b2b2b" alt="termina: input parsing"></a>
  <a href="https://github.com/doy/vt100-rust"><img src="https://img.shields.io/badge/vt100-terminal%20emulation-2b2b2b" alt="vt100: terminal emulation"></a>
  <a href="https://github.com/dfrankland/keycode"><img src="https://img.shields.io/badge/keycode-physical%20key%20codes-2b2b2b" alt="keycode: physical key codes"></a>
  <a href="https://github.com/n0-computer/iroh"><img src="https://img.shields.io/badge/iroh-QUIC%20transport-2b2b2b" alt="iroh: QUIC transport"></a>
  <a href="https://github.com/rkyv/rkyv"><img src="https://img.shields.io/badge/rkyv-message%20format-2b2b2b" alt="rkyv: message format"></a>
  <a href="https://github.com/zesterer/flume"><img src="https://img.shields.io/badge/flume-channels-2b2b2b" alt="flume: channels"></a>
  <a href="https://github.com/bytecodealliance/rustix"><img src="https://img.shields.io/badge/rustix-system%20calls-2b2b2b?logo=rust&logoColor=white" alt="rustix: system calls"></a>
</p>

## License

EUPL-1.2. See [LICENSE](LICENSE).

The AV1 codec is royalty-free under the Alliance for Open Media Patent License
1.0; its text ships in
[docs/licenses/AV1-PATENT-LICENSE](docs/licenses/AV1-PATENT-LICENSE).
