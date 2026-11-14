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

<p>
  <a href="https://github.com/Smithay/smithay"><img align="middle" src="https://raw.githubusercontent.com/Smithay/smithay.github.io/content/content/images/smithay_logo.svg" height="24" alt="smithay"></a>&nbsp;<b>smithay</b>&nbsp;<sub>the&nbsp;Wayland&nbsp;compositor</sub>
   &nbsp; 
  <a href="https://github.com/tokio-rs/tokio"><picture><source media="(prefers-color-scheme: dark)" srcset="https://cdn.simpleicons.org/tokio/ffffff"><img align="middle" src="https://cdn.simpleicons.org/tokio" height="24" alt="tokio"></picture></a>&nbsp;<b>tokio</b>&nbsp;<sub>async&nbsp;runtime</sub>
   &nbsp; 
  <a href="https://github.com/ratatui/ratatui"><img align="middle" src="https://raw.githubusercontent.com/ratatui/ratatui/main/assets/logo.png" height="24" alt="ratatui"></a>&nbsp;<b>ratatui</b>&nbsp;<sub>widgets</sub>
   &nbsp; 
  <a href="https://github.com/crossterm-rs/crossterm"><img align="middle" src="https://github.com/crossterm-rs.png?size=256" height="24" alt="crossterm"></a>&nbsp;<b>crossterm</b>&nbsp;<sub>terminal&nbsp;I/O</sub>
   &nbsp; 
  <a href="https://github.com/helix-editor/termina"><img align="middle" src="https://github.com/helix-editor.png?size=256" height="24" alt="termina"></a>&nbsp;<b>termina</b>&nbsp;<sub>input&nbsp;parsing</sub>
   &nbsp; 
  <a href="https://github.com/doy/vt100-rust"><img align="middle" src="https://github.com/doy.png?size=256" height="24" alt="vt100"></a>&nbsp;<b>vt100</b>&nbsp;<sub>emulation</sub>
   &nbsp; 
  <a href="https://github.com/dfrankland/keycode"><img align="middle" src="https://github.com/dfrankland.png?size=256" height="24" alt="keycode"></a>&nbsp;<b>keycode</b>&nbsp;<sub>key&nbsp;codes</sub>
   &nbsp; 
  <a href="https://github.com/n0-computer/iroh"><img align="middle" src="https://raw.githubusercontent.com/n0-computer/iroh/main/.img/iroh_wordmark.svg" height="19" alt="iroh"></a>&nbsp;<b>iroh</b>&nbsp;<sub>QUIC&nbsp;transport</sub>
   &nbsp; 
  <a href="https://github.com/rkyv/rkyv"><img align="middle" src="https://raw.githubusercontent.com/rkyv/rkyv/main/media/logo_color.svg" height="24" alt="rkyv"></a>&nbsp;<b>rkyv</b>&nbsp;<sub>message&nbsp;format</sub>
   &nbsp; 
  <a href="https://github.com/zesterer/flume"><img align="middle" src="https://github.com/zesterer.png?size=256" height="24" alt="flume"></a>&nbsp;<b>flume</b>&nbsp;<sub>channels</sub>
   &nbsp; 
  <a href="https://github.com/bytecodealliance/rustix"><img align="middle" src="https://github.com/bytecodealliance.png?size=256" height="24" alt="rustix"></a>&nbsp;<b>rustix</b>&nbsp;<sub>system&nbsp;calls</sub>
</p>

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
