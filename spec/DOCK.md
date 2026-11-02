# DOCK — two screens, one line of furniture

Status: design only, nothing built. Replaces the current footer, which is a fixed
row at the bottom of a screen the client owns outright.

## The idea

The client has **two screens**, and the terminal keeps them apart for it:

- the **Logs** screen is the terminal's **main screen**. Ordinary text, printed as
  ordinary output, accumulating in the terminal's own history exactly like any
  other program's output. Everything above the client when it started is still
  above it, untouched.
- the **Windows** screen is the **alternate screen** — a blank screen the terminal
  hands over for as long as the client wants it. The program's window fills it,
  with nothing above it.

A **dock** switches between them, sits under whatever is on screen, and carries
the session's status.

```text
   Logs: short                 Logs: full page              Windows

   17:03 dialed                17:01 line 40                the window
   17:03 starting ghostty      17:01 line 41
   17:03 window arrived        17:02 dialed                 (nothing above it)
   ┌────────────────────┐      17:02 starting ghostty
   │ Logs Windows    ●  │      17:03 window arrived
   └────────────────────┘      ┌────────────────────┐       ┌────────────────────┐
                               │ Logs Windows    ●  │       │ Logs Windows    ●  │
                               └────────────────────┘       └────────────────────┘
```

## Where the dock is

It sits under the last line of what is on screen, and **never below the last row
of the screen**. The bottom is a floor, not a fixed position:

```text
   dock_row = min(rows - DOCK_HEIGHT, last_content_row + 1)     DOCK_HEIGHT = 3
```

Both halves are load-bearing. Three log lines leave the dock on the fourth row
with blank screen below it — a dock pinned to the bottom of a nearly empty screen
is chrome pretending to be content. A full screen, or a window, pushes the dock
down to the last three rows, where it stays, because there is nowhere further to
go. That is what makes new output read as the content moving **under** the dock
rather than the dock being pushed off the bottom.

The dock's row is **derived from what is on screen, in the same frame**. It is not
remembered, because a remembered row is a second truth that disagrees with the
screen on the frame the screen changed.

## The dock is a bordered block, so it is three rows

A box with all four borders: a top border, the row the buttons and the status sit
on, and a bottom border. Three rows, whatever is in them. The sides are drawn too,
so the dock reads as a box rather than as a row of text that happens to be at the
bottom.

## The swap

One gesture, and the terminal does the work: it hides one screen and shows the
other. Coming back restores the main screen exactly as it was — every log line,
every line that scrolled into history, and everything the shell printed before the
client started.

Returning to the Logs screen happens in three parts, and all three are needed:

1. **Take back the main screen.**
2. **Print what arrived while the Windows screen was up.** The child's output is
   held for exactly as long as nobody could see it, and released here.
3. **Forget the screen.** What the client believes is drawn describes the screen it
   was drawing, and the child's bytes have since changed the screen under it. That
   belief is a fiction, and a fiction redrawn as a difference is corruption.

The release and the redraw of the dock are presented as one step, so a long hold
does not appear a screenful at a time.

There is no copy of the screen kept anywhere and nothing to reconstruct: the
visible text is never the storage, the terminal is. That is why a session can be
switched back and forth without limit and lose nothing.

## Who owns the alternate screen

The alternate screen is **one screen, and one bit in the outermost terminal**.
Nothing ties it to a program, a pty, or a connection: whatever switches last takes
it, and whatever leaves first drops everything else back to the main screen. Two
claimants cannot share it, and the second's entry destroys the first's content,
because a screen cannot be read back.

So the client takes it and keeps it for the whole session:

- **A child asking for the alternate screen is not obeyed.** The terminal has
  already handed it to the client, and the client owns the child's output — its
  bytes go into the hold and out through the client's own printing, so they never
  reach the terminal directly. No forwarded byte can move the screen out from
  under the dock.
- **The request is still recorded.** It is part of the child's output, in its
  place in the stream, and it is what the client prints when it returns to the
  Logs screen. So a program that drew still draws, at the moment the user is
  looking.

The alternative — handing the terminal over whenever a child asks, and taking it
back when the child is done — is rejected. The dock would vanish whenever a
wrapped program felt like drawing, the Windows screen would become unreachable
exactly when there is a window worth showing, and whichever claimant ended second
would leave the terminal on a screen nobody chose.

## Output while the Windows screen is up

The child keeps writing. Nobody can see it, because the screen it would be written
to is the one the client is drawing the window on. So it is held, and the hold is
bounded by the **visit**, not by the session: it lasts until the client returns to
the Logs screen, and it is released there.

Holding costs nothing that matters. The bytes are also in the client's log, so
nothing depends on the hold surviving a crash, and its length is a user's choice to
look at a window rather than a program's runtime.

A held stream is replayed, not relived: a program that drew for a minute appears in
an instant when it is finally printed. That is correct for output the user chose to
not be watching, and it is the reason a program that must be *interacted with*
belongs in a window of the compositor rather than in a pane's output.

## What the Logs screen carries

Everything the client has to say, as ordinary lines:

- its own diagnostics — the same lines that go to its log file, from the same
  writer, so the screen and the file never disagree;
- the announcement while it waits to be dialed;
- the program's own output, printed as it arrives: output like any other, on the
  screen where a user looks for it.

Nothing is filtered by level: a log that dropped lines would drop the ones nobody
thought to raise.

## Exiting

Leaving the Windows screen shows the Logs screen, and then the client leaves the
Logs screen alone: the dock is cleared, the cursor is left after the last line, and
the terminal is handed back as it was found. What remains is ordinary output with
the shell's prompt below it, and history above it where it always was.

A session that ends while on the Windows screen therefore ends with history
visible, not with a window-shaped hole — and with whatever the child wrote while
the window was up, printed before the client goes.

## State

Two facts outlive a frame:

- **which screen is up**;
- **the output that arrived where nobody could see it.**

Everything else — the dock's position, the page's rectangle, what is drawn —
follows from those and the terminal's size on every frame.

## What this replaces

The footer, and the panels that opened over the session with it. The footer is a
row drawn as part of the pane; the dock is furniture of a screen. And there is
nothing to open *over* anything any more: the two panels become the two screens.

## Open questions

- **A terminal without an alternate screen.** The capability is askable, and its
  absence is a real answer rather than a fallback to invent. Whether the client
  refuses to start there, or runs with one screen and no switching, is not decided.
- **The hold's size.** Bounded by the visit is not bounded in bytes. A chatty
  program left running while the user watches a window is the case that would make
  a cap necessary, and a cap is a silent truncation unless it says so.
- **Scrolling the Logs screen.** History above the screen belongs to the terminal,
  not to the client, so the client can scroll only what it wrote, and only by
  printing it again — which is not a scroll. The wheel in Logs mode is a no-op
  unless the client also keeps its own copy of the lines.
