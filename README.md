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

- <img src="https://raw.githubusercontent.com/ratatui/ratatui/main/assets/logo.png" height="26" alt="">&nbsp; **[ratatui](https://github.com/ratatui/ratatui)**&nbsp;&nbsp;<sub>widgets and layout</sub>
- <picture><source media="(prefers-color-scheme: dark)" srcset="https://cdn.simpleicons.org/tokio/ffffff"><img src="https://cdn.simpleicons.org/tokio" height="26" alt=""></picture>&nbsp; **[tokio](https://github.com/tokio-rs/tokio)**&nbsp;&nbsp;<sub>async runtime</sub>
- <img src="https://github.com/Smithay.png?size=256" height="26" alt="">&nbsp; **[smithay](https://github.com/Smithay/smithay)**&nbsp;&nbsp;<sub>the Wayland compositor</sub>
- <img src="https://github.com/crossterm-rs.png?size=256" height="26" alt="">&nbsp; **[crossterm](https://github.com/crossterm-rs/crossterm)**&nbsp;&nbsp;<sub>terminal I/O</sub>
- <img src="https://github.com/helix-editor.png?size=256" height="26" alt="">&nbsp; **[termina](https://github.com/helix-editor/termina)**&nbsp;&nbsp;<sub>terminal input parsing</sub>
- <img src="https://github.com/doy.png?size=256" height="26" alt="">&nbsp; **[vt100](https://github.com/doy/vt100-rust)**&nbsp;&nbsp;<sub>terminal emulation</sub>
- <img src="https://github.com/dfrankland.png?size=256" height="26" alt="">&nbsp; **[keycode](https://github.com/dfrankland/keycode)**&nbsp;&nbsp;<sub>physical key codes</sub>
- <img src="https://iroh.computer/img/logo/iroh-wordmark-purple.svg" height="17" alt="">&nbsp; **[iroh](https://github.com/n0-computer/iroh)**&nbsp;&nbsp;<sub>QUIC transport</sub>
- <img src="https://raw.githubusercontent.com/rkyv/rkyv/main/media/logo_color.svg" height="26" alt="">&nbsp; **[rkyv](https://github.com/rkyv/rkyv)**&nbsp;&nbsp;<sub>message format</sub>
- <img src="https://github.com/zesterer.png?size=256" height="26" alt="">&nbsp; **[flume](https://github.com/zesterer/flume)**&nbsp;&nbsp;<sub>channels</sub>
- <img src="https://github.com/bytecodealliance.png?size=256" height="26" alt="">&nbsp; **[rustix](https://github.com/bytecodealliance/rustix)**&nbsp;&nbsp;<sub>system calls</sub>

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
