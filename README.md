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
    <td width="33%" valign="top"><table><tr>
      <td width="40"><img src="https://raw.githubusercontent.com/Smithay/smithay.github.io/content/content/images/smithay_logo.svg" width="34" alt=""></td>
      <td><b>smithay</b><br><sub>the Wayland compositor</sub></td>
    </tr></table></td>
    <td width="33%" valign="top"><table><tr>
      <td width="40"><picture><source media="(prefers-color-scheme: dark)" srcset="https://cdn.simpleicons.org/tokio/ffffff"><img src="https://cdn.simpleicons.org/tokio" width="34" alt=""></picture></td>
      <td><b>tokio</b><br><sub>async runtime</sub></td>
    </tr></table></td>
    <td width="33%" valign="top"><table><tr>
      <td width="40"><img src="https://raw.githubusercontent.com/ratatui/ratatui/main/assets/logo.png" width="34" alt=""></td>
      <td><b>ratatui</b><br><sub>widgets and layout</sub></td>
    </tr></table></td>
  </tr>
  <tr>
    <td width="33%" valign="top"><table><tr>
      <td width="40"><img src="https://github.com/crossterm-rs.png?size=64" width="34" alt=""></td>
      <td><b>crossterm</b><br><sub>terminal I/O</sub></td>
    </tr></table></td>
    <td width="33%" valign="top"><table><tr>
      <td width="40"><img src="https://github.com/helix-editor.png?size=64" width="34" alt=""></td>
      <td><b>termina</b><br><sub>input parsing</sub></td>
    </tr></table></td>
    <td width="33%" valign="top"><table><tr>
      <td width="40"><img src="https://github.com/doy.png?size=64" width="34" alt=""></td>
      <td><b>vt100</b><br><sub>terminal emulation</sub></td>
    </tr></table></td>
  </tr>
  <tr>
    <td width="33%" valign="top"><table><tr>
      <td width="40"><img src="https://github.com/dfrankland.png?size=64" width="34" alt=""></td>
      <td><b>keycode</b><br><sub>physical key codes</sub></td>
    </tr></table></td>
    <td width="33%" valign="top"><table><tr>
      <td width="40"><img src="https://raw.githubusercontent.com/n0-computer/iroh/main/.img/iroh_wordmark.svg" width="34" alt=""></td>
      <td><b>iroh</b><br><sub>QUIC transport</sub></td>
    </tr></table></td>
    <td width="33%" valign="top"><table><tr>
      <td width="40"><img src="https://raw.githubusercontent.com/rkyv/rkyv/main/media/logo_color.svg" width="34" alt=""></td>
      <td><b>rkyv</b><br><sub>message format</sub></td>
    </tr></table></td>
  </tr>
  <tr>
    <td width="33%" valign="top"><table><tr>
      <td width="40"><img src="https://github.com/zesterer.png?size=64" width="34" alt=""></td>
      <td><b>flume</b><br><sub>channels</sub></td>
    </tr></table></td>
    <td width="33%" valign="top"><table><tr>
      <td width="40"><img src="https://github.com/bytecodealliance.png?size=64" width="34" alt=""></td>
      <td><b>rustix</b><br><sub>system calls</sub></td>
    </tr></table></td>
    <td width="33%"></td>
  </tr>
</table>

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
