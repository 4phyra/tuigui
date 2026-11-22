# TRANSPORT — the channel between a client and a server

Status: **built**, twice. `crates/wire/src/{transport,unix,p2p}.rs`:
`UnixSocket` for one machine, and `P2p` — an iroh/QUIC endpoint that works
across machines. Selection is per session, on the client: `--channel` (the
`Channel` enum in `crates/gui/src/lib.rs`, `unix` or `p2p`, default `p2p`).

## The shape

A session has two ends and a channel between them. The channel is a **transport**:
one type that owns its connection and does everything a caller wants over it.

```rust
pub trait Transport: Send + Sync {
    fn name(&self) -> &'static str;
    fn address(&self) -> Option<String>;
    fn to_magic(&self) -> Option<String>;              // defaults from name+address
    fn from_magic(name: &str, address: &str) -> Option<Self>;
    fn connect(&self) -> BoxFuture<'_, io::Result<Connection>>;
    fn accept(&self)  -> BoxFuture<'_, io::Result<Connection>>;
    fn send<'a>(&'a self, message: &'a Message) -> BoxFuture<'a, io::Result<()>>;
    fn recv(&self) -> BoxFuture<'_, io::Result<Option<Message>>>;
}
```

Three decisions in that signature, each of which was a different shape first.

### The transport *is* the link, not a factory for one

`send` and `recv` take no link argument, because the transport holds its own
connection. Framing is a property of how *these* bytes travel, and a transport
that carries several peers has to route — so both belong to the transport, and
neither is something a caller can choose.

An earlier version had `Transport` as a factory returning a `Connection` that the
caller then passed back into `send`/`recv`. That works for exactly one peer per
transport and falls apart for ssh, where one connection carries several clients
and the routing has to be consulted on every message.

### Reads wait

```rust
fn recv(&self) -> BoxFuture<'_, io::Result<Option<Message>>>;   // waits
```

`None` means the peer closed. There is no `try_recv`, deliberately.

A non-blocking read looks like the right primitive for a server serving several
peers, and it is not: a message has to be *sent* before it can arrive, so a read
that returns the instant it is asked asks before the peer has had any chance to
write. The handshake failed on peers that were merely not instant. What the
server actually needs is to wait on all its peers **at once** — and that is
`select_all` over per-peer `recv` futures, which is what it now does. One read
per peer, all pending together, no polling.

### `connect` and `accept` are the handshake, not the socket

Both dial or wait *and* perform the greeting, and both return only an
established link. The two ends of a session are peers — either may dial, either
may accept — and the exchange is the same with the order reversed:

```text
   the side that dialed          the side that was dialed
   ────────────────────          ───────────────────────
   connect()                     accept()
     dial the address              wait for a connection
     send Ping(label)              recv → Ping
     recv → Pong                   send Pong
     ⇒ Connection                  ⇒ Connection
```

The handshake lives here rather than in a caller because it is not optional: a
peer that never answers is not a peer, and the half that owns the channel is the
half that knows how to wait on it. A caller gets a peer it can talk to or the
reason it cannot — never a link that is up but unproven.

## The message layer

`Message` is what the two ends say, and it is not a transport concern:

`Message` is what the two ends say — `OpenWindow`, `Frame`, `WindowCreated`,
`CloseWindow`, `CompositorRequest`, and the greeting — and it is not a
transport concern; it is defined in `crates/wire/src/message.rs` and simply
carried opaquely here.

Framing is `Framing`: bytes in, whole messages out. A stream has no message
boundaries, so each message is a one-byte tag and a body behind a 4-byte
big-endian length. It is push-based — `push(bytes)` returns the messages those
bytes completed — because reading is the transport's job: only it knows how its
own IO works. It is cancel-safe, so a read dropped mid-message loses nothing.

## The announcement

A client cannot be dialed before the server knows a channel to dial, so it says
so in the one place the other end is already looking: its own output.

```text
   unix : /run/user/1000/tuigui.sock     # the local channel
   p2p : <ticket>                        # the QUIC channel
```

Plain text, one line, written into the pane. The scan accepts either, via
`crates/wire/src/transport.rs`'s `names()`; the ticket is a base32 string, so it
survives the same scan a socket path does.

**Not an escape sequence**, which is what it was first. A multiplexer is a
terminal emulator as well as a relay: it parses a pane's output into a grid of
cells and re-draws that grid, so an OSC it does not model is consumed and never
reaches anything reading the pane. A private OSC announcement dies there, which
is exactly the case this design exists for. Text does not die: it becomes cells,
and cells are what a multiplexer re-draws.

The cost is real and accepted: an announcement is no longer unambiguously ours.
A command that printed `unix : /some/path` would be dialed.

### Finding it

The bytes are a *rendered* stream, not a log. A client drawing its pane moves the
cursor with escapes and never emits a newline, so "read a line, then parse it"
finds the announcement glued to the end of a whole screen dump. The scan
therefore looks for the separator and reads the address after it, ending at the
first blank or control byte — which is what stops the renderer's row padding
becoming part of the address.

One sighting per address, because the text stays on screen and every redraw puts
it out again. The same client seen twice is one client.

## What is deliberately not here

- **A session file.** A path written to disk had to be created before anything
  ran, cleaned up after a crash, and kept in step with live processes; a stale one
  made a client dial a socket nobody was listening on. Announcing in band needs
  nothing provisioned and cannot go stale.
- **A transport ranking or `Plan`.** Ranking and capability comparison exist
  for a set of interchangeable candidates, and this is not one: the two
  transports are not interchangeable, and the choice between them is the
  user's, made with `--channel` on the client. There is no machinery to rank
  them, deliberately.
- **The p2p frame encoding.** The old crate-internal p2p *framing* from the
  first design is gone; what exists now is `P2p` (`crates/wire/src/p2p.rs`): an
  iroh endpoint per side, ALPN `tuigui/1`, announced as a ticket.

## Files

```
crates/wire/src/
  message.rs     Message, and its encoding
  transport.rs   Transport, Connection, Framing, the announcement format
  unix.rs        UnixSocket — one machine
  p2p.rs         P2p — QUIC via iroh, announced as a ticket
  drain.rs       drain whatever a reader holds right now
crates/tui/src/
  bytes.rs       the pty's output, fanned out to readers
  proxy/scanner.rs  the scan (shares the cell grid with the rewriter)
  server.rs      one loop: the announcement, and every attached client
crates/gui/src/
  lib.rs         bind, announce, accept, draw
```

## Open questions

1. **A ranking of the two transports.** Not wanted: the choice is `--channel`
   on the client, not something the code arbitrates. A third transport would
   face the same question, and the answer would still be the user's flag until
   one channel is clearly better everywhere. ssh remains unbuilt; see
   [SSH_SEAM.md](SSH_SEAM.md) and [REVERSE_SSH.md](REVERSE_SSH.md).
2. **One sighting per address, forever.** The scan remembers addresses so a
   redraw is not a second client, and never forgets them. Correct for the
   per-pid unix socket path; wrong for the p2p ticket, which is stable for the
   endpoint's lifetime — a client that is gone and comes back with a fresh
   endpoint ticket is fine, but re-announcing the same ticket would not be
   re-dialed.
3. **Pane provenance.** A pty has one master fd and a multiplexer puts every pane
   on it, so an announcement arrives with no indication of which pane wrote it.
   This is fine for one client and breaks for several; the fix needs the client
   to encode its own pane identity and the server to corroborate it.
