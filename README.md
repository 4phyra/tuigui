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

<table align="left"><tr>
<td valign="middle"><a href="https://github.com/Smithay/smithay"><img src="https://github.com/Smithay.png?size=256" height="26" alt="smithay"></a></td>
<td valign="middle"><b>smithay</b><br><sub>the Wayland compositor</sub></td>
</tr></table>&emsp;&emsp;
<table align="left"><tr>
<td valign="middle"><a href="https://github.com/tokio-rs/tokio"><picture><source media="(prefers-color-scheme: dark)" srcset="https://cdn.simpleicons.org/tokio/ffffff"><img src="https://cdn.simpleicons.org/tokio" height="26" alt="tokio"></picture></a></td>
<td valign="middle"><b>tokio</b><br><sub>async runtime</sub></td>
</tr></table>&emsp;&emsp;
<table align="left"><tr>
<td valign="middle"><a href="https://github.com/ratatui/ratatui"><img src="https://raw.githubusercontent.com/ratatui/ratatui/main/assets/logo.png" height="26" alt="ratatui"></a></td>
<td valign="middle"><b>ratatui</b><br><sub>widgets and layout</sub></td>
</tr></table>&emsp;&emsp;
<table align="left"><tr>
<td valign="middle"><a href="https://github.com/crossterm-rs/crossterm"><img src="https://github.com/crossterm-rs.png?size=256" height="26" alt="crossterm"></a></td>
<td valign="middle"><b>crossterm</b><br><sub>terminal I/O</sub></td>
</tr></table>&emsp;&emsp;
<br clear="all">
<table align="left"><tr>
<td valign="middle"><a href="https://github.com/helix-editor/termina"><img src="https://github.com/helix-editor.png?size=256" height="26" alt="termina"></a></td>
<td valign="middle"><b>termina</b><br><sub>input parsing</sub></td>
</tr></table>&emsp;&emsp;
<table align="left"><tr>
<td valign="middle"><a href="https://github.com/doy/vt100-rust"><img src="https://github.com/doy.png?size=256" height="26" alt="vt100"></a></td>
<td valign="middle"><b>vt100</b><br><sub>terminal emulation</sub></td>
</tr></table>&emsp;&emsp;
<table align="left"><tr>
<td valign="middle"><a href="https://github.com/dfrankland/keycode"><img src="https://github.com/dfrankland.png?size=256" height="26" alt="keycode"></a></td>
<td valign="middle"><b>keycode</b><br><sub>physical key codes</sub></td>
</tr></table>&emsp;&emsp;
<br clear="all">
<table align="left"><tr>
<td valign="middle"><a href="https://github.com/n0-computer/iroh"><img src="https://iroh.computer/img/logo/iroh-wordmark-purple.svg" height="26" alt="iroh"></a></td>
<td valign="middle"><b>iroh</b><br><sub>QUIC transport</sub></td>
</tr></table>&emsp;&emsp;
<table align="left"><tr>
<td valign="middle"><a href="https://github.com/rkyv/rkyv"><img src="https://raw.githubusercontent.com/rkyv/rkyv/main/media/logo_color.svg" height="26" alt="rkyv"></a></td>
<td valign="middle"><b>rkyv</b><br><sub>message format</sub></td>
</tr></table>&emsp;&emsp;
<table align="left"><tr>
<td valign="middle"><a href="https://github.com/zesterer/flume"><img src="https://github.com/zesterer.png?size=256" height="26" alt="flume"></a></td>
<td valign="middle"><b>flume</b><br><sub>channels</sub></td>
</tr></table>&emsp;&emsp;
<br clear="all">
<table align="left"><tr>
<td valign="middle"><a href="https://github.com/bytecodealliance/rustix"><img src="https://github.com/bytecodealliance.png?size=256" height="26" alt="rustix"></a></td>
<td valign="middle"><b>rustix</b><br><sub>system calls</sub></td>
</tr></table>
<br clear="all">

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
