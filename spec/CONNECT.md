# CONNECT — how the two ends find each other

Status: **built** — on one machine, and across machines via the p2p channel.
The announcement, handshake and provenance story is unchanged; the missing
pice it waited on is [TRANSPORT.md](TRANSPORT.md)'s second transport.

## The idea

The server sits in the middle of the byte stream, so it can be the one that
**dials**. A client does not need to know where the server is. It only needs to
announce itself, and the server — which is already reading every byte the pane
produces — sees the announcement and connects back.

This reverses the original direction. The server used to bind a socket and
publish a session file, and the client read that file and dialed. Now the client
publishes and the server dials.

## Why the server can see it at all

The server owns the last buffer before the real terminal. Everything the wrapped
command — and any multiplexer inside it — writes comes through one pty, and the
server reads it. That is the only reason the announcement reaches the server
without a side channel: it travels up the same path as normal text.

The clean form of this is a **subscription, not a callback**: the pty hands out a
handle to what it produces, and the server reads it. The pty does not know who is
reading, so adding a reader is not a change to the pty. Two readers exist — the
scan and the terminal — and each gets every chunk, so neither can consume bytes
the other never saw.

## The announcement

```text
   unix : /run/user/1000/tuigui.sock
```

One line of text, written into the pane's output as *drawing*. It began as a
private OSC sequence, which is invisible and unambiguously ours, and that is
wrong for the case this exists for: a multiplexer parses a pane's output into a
grid of cells and re-draws it, so an OSC it does not model is dropped and never
read. Text is not dropped. The client makes its announcement by drawing it, so
the text the server reads is the same text the user sees.

The announcement carries no secret and proves nothing. What it gives is a
**channel to dial**, and the identity that matters comes from the handshake on
that channel.

## The handshake

Both ends are peers; either may dial and either may accept. The exchange is the
same with the order reversed:

```text
   dialer                              acceptor
   ──────                              ────────
   connect to the announced address    wait for a connection
   send Ping(label)                    recv → Ping
   recv → Pong                         send Ping's answer
   ⇒ an established link               ⇒ an established link
```

Only an established link is returned to the caller. A peer that never answers is
a peer that is not there, and there is no such thing as a link that is up but
unproven — that state was removed rather than left for every caller to check.

On one machine this is the whole of the authorization: a unix socket in a
directory only the user can reach is enforced by the kernel at `connect` time.

## Locality

One machine needs nothing beyond the socket: the client picks a path under
`XDG_RUNTIME_DIR` — private to the user, cleared on logout — binds it, and says so
in the pane. Nothing has to be provisioned and nothing survives a crash.

## Across machines

Built, and the default. The announcement travels fine, since a pane's output
crosses ssh like any other text — `p2p : <ticket>` is announced by the client
and the server dials it, with no session setup beyond starting both halves.
`tui! ssh host gui! foot` is the shape it enables (README). The two ends do
not need a shared network beyond what iroh's NAT traversal can punch through,
and the `--channel unix` flag opts back down to one machine.

The two proposals below remain, for networks where a direct iroh path cannot
be established or policy forbids it:

- **[SSH_SEAM.md](SSH_SEAM.md)** — arrange a port forward on the ssh that
  started the session. It reaches the terminal side through the connection that
  already exists, and it needs a shim in front of an ssh we do not own.
- **[REVERSE_SSH.md](REVERSE_SSH.md)** — the pane side reads `SSH_CONNECTION`
  and ssh's back to the user's machine. Nothing to arrange, and it depends on
  the user's machine being reachable from the pane.

Two things made the remote case different from the local one, and where they
landed:

- **A second transport brought the question of ranking.** It did not bring
  ranking: with p2p built, the choice is still the user's, made with
  `--channel`; there is no ranking to design.
- **The address is stable.** A p2p ticket names one endpoint until it is
  re-announced, so the scan's "one sighting per address, forever" rule — right
  for a per-pid socket path — also covers a fixed endpoint: a re-announcing
  client with the same endpoint is one client, not many.

## Open problem: pane provenance

A pty has a single master fd, and a multiplexer multiplexes all its panes onto
it. So an announcement arrives with **no indication of which pane wrote it**.

With one client this does not matter. With several it breaks twice:

- The server cannot tell which pane a stream belongs to, so it cannot draw the
  frames in the right place.
- The server cannot bind an announcement to a pane, so it cannot tell whose
  frames it is accepting.

Anything that fixes this needs the client to encode its own pane identity, plus a
way for the server to corroborate it. That is a separate design, and this one is
scoped to a single client until it is solved.
