# Build and run the three binaries.
#
# The recipes exist because the invocations are long and fixed: a target directory that must
# be `./target` (a different one invalidates every fingerprint and rebuilds the TLS stack), a
# package to build, the feature to build it with, and a binary to run. Spelling that out each
# time is where it goes wrong.
#
# Run inside the dev shell (`nix develop`), which is where cargo and the system libraries it
# links against come from.

target := "./target"

# The profiler's client, linked into every build these recipes make.
#
# It is a compiled C++ library, so it only exists when the `tracy` feature is on — and the
# flags that turn it on (`--dbg-trace-tui`, `--dbg-trace-gui`) are compiled out along with
# it. Built with it here so a session can be looked at without needing a second way of
# building it; a plain `cargo build` links no profiler and offers no way to ask for one.
tracy := "--features=tracy"

# Build the server around a command, then run it in this terminal.
#
# The command is what the server puts on a pty, so `just tui ./target/release/gui` runs the
# client inside the server — which is the usual shape, since the client announces itself in
# the pane's output and the server is what reads it.
tui *command:
    CARGO_TARGET_DIR={{target}} cargo run --release -p tui --bin tui {{tracy}} -- {{command}}

# Build and run the client.
#
# On its own it binds a socket and waits to be found, so it does nothing useful until a
# server is reading the pane it draws in. `just tui` with this as the command is the usual
# way to start it.
gui *args:
    CARGO_TARGET_DIR={{target}} cargo run --release -p gui --bin gui {{tracy}} -- {{args}}

# Build and run both halves as one command: the server owns this terminal, and the client
# runs inside the pty it creates.
tuigui *args:
    CARGO_TARGET_DIR={{target}} cargo run --release -p tuigui --bin tuigui {{tracy}} -- {{args}}

# Build and run the capybara demo, with no program, no codec and no link behind it.
#
# The compositor drawing itself: what it exercises is the stacking, the chrome, the dragging
# and the resizing. No profiler here, because there is no session under it to profile, and
# `q` or `Esc` ends it.
dummy:
    CARGO_TARGET_DIR={{target}} cargo run --release -p compositor --example dummy

# Draw every widget in `tuigui_components` and press them.
#
# The components crate's own demo: a centred column of the widgets, and a line under them saying
# what was last pressed. No session, no link, no program behind it — what it exercises is the
# dispatch, where one frame's events are offered to each widget in draw order, and the tab strip's
# move from one tab to the next.
gallery:
    CARGO_TARGET_DIR={{target}} cargo run -p tuigui_components --bin gallery
