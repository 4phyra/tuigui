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

<table>
  <tr>
    <td align="center" width="150"><a href="https://github.com/tokio-rs/tokio"><picture><source media="(prefers-color-scheme: dark)" srcset="https://cdn.simpleicons.org/tokio/ffffff"><img src="https://cdn.simpleicons.org/tokio" height="34" alt="tokio"></picture></a><br><a href="https://github.com/tokio-rs/tokio">tokio</a><br><sub>async runtime</sub></td>
    <td align="center" width="150"><a href="https://github.com/zesterer/flume"><img src="https://github.com/zesterer.png?size=64" height="34" alt="flume"></a><br><a href="https://github.com/zesterer/flume">flume</a><br><sub>channels</sub></td>
    <td align="center" width="150"><a href="https://github.com/ratatui/ratatui"><img src="https://raw.githubusercontent.com/ratatui/ratatui/main/assets/logo.png" height="34" alt="ratatui"></a><br><a href="https://github.com/ratatui/ratatui">ratatui</a><br><sub>widgets</sub></td>
    <td align="center" width="150"><a href="https://github.com/crossterm-rs/crossterm"><img src="https://github.com/crossterm-rs.png?size=64" height="34" alt="crossterm"></a><br><a href="https://github.com/crossterm-rs/crossterm">crossterm</a><br><sub>terminal I/O</sub></td>
  </tr>
  <tr>
    <td align="center" width="150"><a href="https://github.com/helix-editor/termina"><img src="https://github.com/helix-editor.png?size=64" height="34" alt="termina"></a><br><a href="https://github.com/helix-editor/termina">termina</a><br><sub>input parsing</sub></td>
    <td align="center" width="150"><a href="https://github.com/doy/vt100-rust"><img src="https://github.com/doy.png?size=64" height="34" alt="vt100"></a><br><a href="https://github.com/doy/vt100-rust">vt100</a><br><sub>terminal emulation</sub></td>
    <td align="center" width="150"><a href="https://github.com/dfrankland/keycode"><img src="https://github.com/dfrankland.png?size=64" height="34" alt="keycode"></a><br><a href="https://github.com/dfrankland/keycode">keycode</a><br><sub>physical keys</sub></td>
    <td align="center" width="150"><a href="https://github.com/Smithay/smithay"><img src="https://github.com/Smithay.png?size=64" height="34" alt="smithay"></a><br><a href="https://github.com/Smithay/smithay">smithay</a><br><sub>Wayland compositor</sub></td>
  </tr>
  <tr>
    <td align="center" width="150"><a href="https://github.com/n0-computer/iroh"><img src="https://raw.githubusercontent.com/n0-computer/iroh/main/.img/iroh_wordmark.svg" height="20" alt="iroh"></a><br><a href="https://github.com/n0-computer/iroh">iroh</a><br><sub>QUIC transport</sub></td>
    <td align="center" width="150"><a href="https://github.com/rkyv/rkyv"><img src="https://raw.githubusercontent.com/rkyv/rkyv/main/media/logo_color.svg" height="34" alt="rkyv"></a><br><a href="https://github.com/rkyv/rkyv">rkyv</a><br><sub>message format</sub></td>
    <td align="center" width="150"><a href="https://github.com/bytecodealliance/rustix"><img src="https://github.com/bytecodealliance.png?size=64" height="34" alt="rustix"></a><br><a href="https://github.com/bytecodealliance/rustix">rustix</a><br><sub>system calls</sub></td>
  </tr>
</table>

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
