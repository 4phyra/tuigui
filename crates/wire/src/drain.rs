//! Taking everything a reader already has, in one call.

use std::io;

/// Everything `reader` holds right now, or `None` when it holds nothing.
///
/// One call is the whole of it: the loop is in here, and it ends at the first read that would wait,
/// which is what "right now" means for a pipe. A caller that looped itself would be asking the same
/// question again with nothing changed in between.
///
/// The reader must be non-blocking — a pipe with nothing to give has to answer rather than wait.
pub fn drain_reader(reader: &mut impl io::BufRead) -> io::Result<Option<Vec<u8>>> {
    let mut all = Vec::new();
    loop {
        match reader.fill_buf() {
            // Closed, and nothing more is coming: what came before it is the answer, and nothing
            // is, if nothing did.
            Ok([]) => return Ok((!all.is_empty()).then_some(all)),
            Ok(bytes) => {
                let len = bytes.len();
                all.extend_from_slice(bytes);
                reader.consume(len);
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                return Ok((!all.is_empty()).then_some(all))
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
}

/// [`drain_reader`] for a reader the runtime waits on.
///
/// Everything the reader holds right now, and then the answer rather than a wait. The read is
/// polled against a no-op waker, so a stream with nothing to give reports `Pending` and this
/// returns what it has collected: same question as [`drain_reader`], asked of a reader the runtime
/// owns rather than a non-blocking descriptor.
///
/// Waiting here instead would be the whole session's latency. This function is called at the top of
/// the server's loop, and everything below it is in the same task, so an `await` on this parks the
/// loop — and the clients' messages are read below it. A client whose window draws while the
/// command is quiet then has nothing reading it, and its next frame blocks on a write until the
/// socket has room or the client gives up on it.
///
/// The no-op waker's wake is dropped, which loses nothing: readiness is kept by the runtime and
/// re-checked on the next poll, so the bytes are still there for the next call.
///
/// `None` only when the stream is closed. A closed pty master is reported by the kernel as `EIO`
/// rather than as an empty read, so that is `None` too. "Nothing waiting" is therefore
/// `Some(vec![])` — an empty chunk, which every consumer already treats as nothing to do — because
/// the caller ends the session on `None`.
pub async fn drain_reader_async(
    reader: &mut (impl tokio::io::AsyncBufRead + Unpin),
) -> io::Result<Option<Vec<u8>>> {
    let mut all = Vec::new();
    let mut reader = std::pin::Pin::new(reader);
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    loop {
        match reader.as_mut().poll_fill_buf(&mut context) {
            std::task::Poll::Ready(Ok([])) => return Ok(None),
            std::task::Poll::Ready(Ok(bytes)) => {
                let len = bytes.len();
                all.extend_from_slice(bytes);
                reader.as_mut().consume(len);
            }
            std::task::Poll::Pending => return Ok(Some(all)),
            std::task::Poll::Ready(Err(e))
                if e.raw_os_error() == Some(rustix::io::Errno::IO.raw_os_error()) =>
            {
                return Ok(None)
            }
            std::task::Poll::Ready(Err(e)) => return Err(e),
        }
    }
}
