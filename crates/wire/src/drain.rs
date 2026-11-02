//! Taking everything a reader already has, in one call.

use std::io;

use tokio::io::AsyncBufReadExt as _;

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

/// [`drain_reader`] for a reader the runtime waits on: **one** chunk, waited on rather than taken.
///
/// Not a loop. The first read waits, because an idle stream has nothing to give, and a second one
/// would wait again — so this is one chunk's worth, and the caller loops because it is the caller
/// that knows when the stream is meant to end. Looping in here would hold every chunk until the
/// stream closed, which on a pty means holding the whole session's output until the command exited.
///
/// `None` only when the stream is closed. A closed pty master is reported by the kernel as `EIO`
/// rather than as an empty read, so that is `None` too: to a reader, "the writer is gone" and
/// "there is nothing waiting" are the same answer. Every other error is returned as it arrived.
pub async fn drain_reader_async(
    reader: &mut (impl tokio::io::AsyncBufRead + Unpin),
) -> io::Result<Option<Vec<u8>>> {
    match reader.fill_buf().await {
        Ok([]) => Ok(None),
        Ok(bytes) => {
            let all = Vec::from(bytes);
            reader.consume(all.len());
            Ok(Some(all))
        }
        Err(e) if e.raw_os_error() == Some(rustix::io::Errno::IO.raw_os_error()) => Ok(None),
        Err(e) => Err(e),
    }
}
