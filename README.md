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
  <a href="https://github.com/ratatui/ratatui"><img src="https://raw.githubusercontent.com/ratatui/ratatui/main/assets/logo.png" height="28" alt="ratatui"></a>&nbsp;
  <a href="https://github.com/tokio-rs/tokio"><picture><source media="(prefers-color-scheme: dark)" srcset="https://cdn.simpleicons.org/tokio/ffffff"><img src="https://cdn.simpleicons.org/tokio" height="28" alt="tokio"></picture></a>&nbsp;
  <a href="https://github.com/rkyv/rkyv"><img src="https://raw.githubusercontent.com/rkyv/rkyv/main/media/logo_color.svg" height="28" alt="rkyv"></a>&nbsp;
  <a href="https://github.com/n0-computer/iroh"><img src="https://raw.githubusercontent.com/n0-computer/iroh/main/.img/iroh_wordmark.svg" height="20" alt="iroh"></a>&nbsp;
  <a href="https://github.com/Smithay/smithay"><img src="https://raw.githubusercontent.com/Smithay/smithay.github.io/content/content/images/smithay_logo.svg" height="24" alt="smithay"></a>
</p>

<b>Built on excellent crates:</b> smithay, tokio, ratatui, crossterm, termina,
vt100, keycode, iroh, rkyv, flume and rustix.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
