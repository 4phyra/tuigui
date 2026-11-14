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

# Run the tests, then review the screens they drew.
#
# nextest rather than `cargo test`: every test here can leave state behind on the
# machine — a pty, a terminal mode, a socket — and nextest runs each of them in a
# process of its own, so one test's terminal cannot be the next one's starting
# point. It also reports a panicking test by name instead of as the exit status
# of a binary that ran twelve of them.
#
# `--tests`, not `--all-targets`: a plain `cargo test` builds the examples
# too, and the compositor demo does not compile at all since the window
# refactor — a test run has no business building it. `--tests` asks for the
# targets that have tests and leaves the examples alone.
#
# The e2e test spawns `foot`, so this only runs in the dev shell (`nix develop`).
#
# The component tests spawn `examples/pane`, and `--tests` leaves examples
# out of what it builds: without the `cargo build` below, a clean tree fails
# them with ENOENT instead of running them. `--examples` replaces the default
# target selection, so this builds the example and nothing else.
#
# `cargo insta test` is that run, but a screen that no longer matches is left as
# a `.snap.new` and the assertion is forced to pass, so it collects every change
# instead of stopping at the first. Nothing replaces a `.snap` before review;
# `INSTA_UPDATE=always` would write every frame unseen, which is recorded but
# not verified. The run exits nonzero both when a test fails and when it has
# left snapshots for review, so its status cannot gate the review; the review
# runs either way, and a failure is still visible in the output above.
#
# `cargo insta review` is the review, and it is the only one: the frames of a
# test arrive in the order they were drawn as `[1/25]`, `a` keeps one and moves
# on, `A` keeps it and every frame after it. With `--snapshot '<test name>'`
# "every frame after" means that test's frames, which is approving one test all
# or nothing. Nothing goes back a frame: one to look at again is
# `cargo insta review --snapshot '…-7.snap'`. `--workspace` is there for the
# same reason the run has it: the root package is `tuigui`, and the component
# snapshots belong to `tuigui_components` — without it the review finds nothing
# at all.
#
# `termlens` is the picture. insta hands a diff tool the old and the new
# snapshot as two files and prints what it prints, and termlens reads a whole
# `.snap`, header included — so the changed cells arrive in colour rather than
# as escaped text. `--color=always` is because insta captures the tool's output
# through a pipe, where `--color=auto` would decide nobody was looking. A screen
# nobody had before has no old file to compare with, and insta hands the tool an
# empty one, which `termlens diff` refuses; `snapshot-diff` below renders that
# new frame instead, so a first recording is a coloured picture too. insta runs
# the tool from a temporary directory, hence the justfile path and
# `--working-directory .`: the filenames it passes are relative to where it runs.
test *args:
    @CARGO_TARGET_DIR={{target}} cargo build -p tuigui_components --examples
    @CARGO_TARGET_DIR={{target}} cargo insta test --test-runner nextest --workspace --tests {{args}} \
        || echo "test: the run reported failures or pending snapshots; reviewing what it recorded"
    @INSTA_DIFF_TOOL="just --justfile {{justfile()}} --working-directory . snapshot-diff" cargo insta review --workspace

# The diff tool `test` gives insta: one snapshot, one picture.
#
# insta hands it `<old> <new>`; `-s` asks whether the old file has anything in
# it, so an empty one is a screen nobody had before and the new frame is what
# there is to show. `|| true` keeps a difference — which is the normal answer —
# from being printed as a failed recipe.
snapshot-diff old new:
    @if [ -s "{{old}}" ]; then termlens diff --color=always "{{old}}" "{{new}}" || true; else termlens render --ansi "{{new}}" || true; fi
