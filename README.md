<p align="center">
  <img src="assets/tuigui-10x.png" alt="tuigui" width="340" height="110">
</p>

<p align="center">
  <a href="https://github.com/4phyra/tuigui"><img src="https://img.shields.io/endpoint?url=https%3A%2F%2Fraw.githubusercontent.com%2F4phyra%2Ftuigui%2Fmaster%2F.github%2Ftokei.json" alt="Rust lines of code"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-GPL--3.0--or--later-blue.svg" alt="License: GPL-3.0-or-later"></a>
  <a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/rust-1.88%2B-orange.svg" alt="Rust: 1.88+"></a>
</p>

## What is tuigui?

tuigui puts graphical programs in the terminal, alongside the text tools you
already use. You give it a program; it gives you that program's window and
video in the pane, through the kitty graphics protocol, while keeping the
terminal's normal input and output intact. The same split between the terminal
side and the program side is intended to let a program run on another machine
and still be used from your terminal.

## Thanks

Built on excellent crates:

| | crate | role |
|:--:|:--|:--|
| <img src="https://raw.githubusercontent.com/ratatui/ratatui/main/assets/logo.png" height="20" alt=""> | [ratatui](https://github.com/ratatui/ratatui) | widgets and layout |
| <picture><source media="(prefers-color-scheme: dark)" srcset="https://cdn.simpleicons.org/tokio/ffffff"><img src="https://cdn.simpleicons.org/tokio" height="20" alt=""></picture> | [tokio](https://github.com/tokio-rs/tokio) | async runtime |
| <img src="https://github.com/crossterm-rs.png?size=64" height="20" alt=""> | [crossterm](https://github.com/crossterm-rs/crossterm) | terminal I/O |
| <img src="https://github.com/helix-editor.png?size=64" height="20" alt=""> | [termina](https://github.com/helix-editor/termina) | input parsing |
| <img src="https://github.com/doy.png?size=64" height="20" alt=""> | [vt100](https://github.com/doy/vt100-rust) | terminal emulation |
| <img src="https://github.com/dfrankland.png?size=64" height="20" alt=""> | [keycode](https://github.com/dfrankland/keycode) | physical key codes |
| <img src="https://github.com/Smithay.png?size=64" height="20" alt=""> | [smithay](https://github.com/Smithay/smithay) | Wayland compositor |
| <img src="https://raw.githubusercontent.com/n0-computer/iroh/main/.img/iroh_wordmark.svg" height="14" alt=""> | [iroh](https://github.com/n0-computer/iroh) | QUIC transport |
| <img src="https://raw.githubusercontent.com/rkyv/rkyv/main/media/logo_color.svg" height="20" alt=""> | [rkyv](https://github.com/rkyv/rkyv) | message format |
| <img src="https://github.com/zesterer.png?size=64" height="20" alt=""> | [flume](https://github.com/zesterer/flume) | channels |
| <img src="https://github.com/bytecodealliance.png?size=64" height="20" alt=""> | [rustix](https://github.com/bytecodealliance/rustix) | system calls |

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
