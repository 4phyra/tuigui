# Build and run the three binaries.
#
# The recipes exist because the invocations are long and fixed: a target directory that must
# be `./target` (a different one invalidates every fingerprint and rebuilds the TLS stack), a
# package to build, the feature to build it with, and a binary to run. Spelling that out each
# time is where it goes wrong.
#
# Run inside the dev shell (`nix develop`), which is where cargo and the system libraries it
# links against come from.
#
# Every recipe starts its command with `@`, so `just` prints no echoed invocation: the recipe
# is one line the reader already knows, and a run should start with the program's own output
# rather than with the command that started it. `tui!` and `gui!` in the dev shell are the
# same recipes with cargo's progress lines hidden as well.

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
    @CARGO_TARGET_DIR={{target}} cargo run --release -p tui --bin tui {{tracy}} -- {{command}}

# Build and run the client.
#
# On its own it binds a socket and waits to be found, so it does nothing useful until a
# server is reading the pane it draws in. `just tui` with this as the command is the usual
# way to start it.
gui *args:
    @CARGO_TARGET_DIR={{target}} cargo run --release -p gui --bin gui {{tracy}} -- {{args}}

# Build and run both halves as one command: the server owns this terminal, and the client
# runs inside the pty it creates.
tuigui *args:
    @CARGO_TARGET_DIR={{target}} cargo run --release -p tuigui --bin tuigui {{tracy}} -- {{args}}

lines:
    tokei -e tests -e examples -e spec -e docs
# Build and run the capybara demo, with no program, no codec and no link behind it.
#
# The compositor drawing itself: what it exercises is the stacking, the chrome, the dragging
# and the resizing. No profiler here, because there is no session under it to profile, and
# `q` or `Esc` ends it.
dummy:
    @CARGO_TARGET_DIR={{target}} cargo run --release -p compositor --example dummy

# Draw every widget in `tuigui_components` and press them.
#
# The components crate's own demo: a centred column of the widgets, and a line under them saying
# what was last pressed. No session, no link, no program behind it — what it exercises is the
# dispatch, where one frame's events are offered to each widget in draw order, and the tab strip's
# move from one tab to the next.
gallery:
    @CARGO_TARGET_DIR={{target}} cargo run -p tuigui_components --bin gallery

# Run the tests. Screens that changed are queued, not printed.
#
# `cargo insta test` runs nextest — one process per test, so a pty cannot leak —
# and writes each mismatched screen as a `.snap.new`. `INSTA_OUTPUT=none` keeps it
# from rendering the screen to stdout: the one line it writes to stderr saying a
# snapshot was stored is the whole report, and `just test-review` is where they
# are looked at.
test *args:
    @CARGO_TARGET_DIR={{target}} cargo build -p tuigui_components --examples
    @INSTA_OUTPUT=none CARGO_TARGET_DIR={{target}} cargo insta test --test-runner nextest --workspace --tests {{args}}

# Look at the screens a run left behind: accept, reject, or skip each one.
#
# `snapshot-diff` shows each one: an empty old file is a new screen rendered
# whole, a real one a coloured cell diff.
test-review:
    @INSTA_DIFF_TOOL="just --justfile {{justfile()}} --working-directory . snapshot-diff" cargo insta review --workspace

# Show one changed screen: the lines that differ, then both screens in their own colours.
#
# Three facts, in the order a reader needs them.
#
# First the unified diff, as text: it is the only part that says *where* a change is,
# and for a change of colour alone it is the `styles:` lines and nothing else.
#
# Then the old screen and the new one, each rendered with `termlens render --ansi`, so
# every cell arrives in the colour it actually is. This is the part that is not
# optional: `termlens diff` paints changed cells hard red and green, which tells you a
# cell changed but throws away the one fact a style change is about — what colour it
# changed from and to. Rendering each screen whole is the only way to show that.
#
# `--color never` on the text diff because insta clears and redraws the screen around
# this output; the escapes in the rendered screens are the colours, not decoration.
snapshot-diff old new:
    @diff -u --label old --label new "{{old}}" "{{new}}" || true
    @echo
    @echo "── old ──"
    @termlens render --ansi "{{old}}" || true
    @echo "── new ──"
    @termlens render --ansi "{{new}}" || true
