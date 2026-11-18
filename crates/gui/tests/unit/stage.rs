use super::*;
use ringbuf::traits::{Producer as _, Split as _};

/// A stage with a window up and words waiting to be printed.
///
/// The compositor is the one thing this cannot fake: a `Stream` page carries it, because a page
/// with a window on it is a page with something to composite. Nothing here draws it — the test
/// only reaches the page that comes after — so it is built over a window whose channels are open
/// and never written to, and whose painter paints nothing.
fn streaming_with_words(words: &str) -> Stage {
    let (mut producer, stdout) = ringbuf::HeapRb::<u8>::new(RING_BUF_CAPACITY).split();
    let pushed = producer.push_slice(words.as_bytes());
    assert_eq!(
        pushed,
        words.len(),
        "the ring has room for every word the test gives it"
    );

    let (set_size, _) = flume::unbounded();
    let (input, _) = flume::unbounded();
    let (close, _) = flume::unbounded();
    let (_, new_children) = flume::unbounded();
    let (_, new_popups) = flume::unbounded();
    Stage {
        page: Page::Stream {
            compositor: Compositor::new(CompositableWindow {
                set_size,
                size: std::sync::Arc::new(std::sync::Mutex::new((8, 8))),
                title: std::sync::Arc::new(std::sync::Mutex::new(String::new())),
                is_popup: false,
                offset_px: None,
                input,
                new_children,
                new_popups,
                close,
                rank: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)),
                render: Box::new(|_, _, _| {}),
            }),
            stdout,
        },
    }
}

/// The words a program printed survive the session ending.
///
/// They wait in a ring that belongs to the screen the window was on, and ending the session
/// replaces that screen. A version that leaves the ring behind ends a session in which the
/// program appears to have said nothing — which is the one thing the logs screen is read for.
#[test]
fn ending_keeps_the_words_the_program_printed() {
    let mut stage = streaming_with_words("the last thing it said\n");
    stage.from_any_to_exiting();

    let mut printed = Vec::new();
    stage
        .render_inner(
            Rect::new(0, 0, 80, 24),
            &mut Buffer::empty(Rect::new(0, 0, 80, 24)),
            &mut StageProps {
                winsize: rustix::termios::Winsize {
                    ws_row: 24,
                    ws_col: 80,
                    ws_xpixel: 0,
                    ws_ypixel: 0,
                },
                program_stdout: &mut printed,
            },
        )
        .expect("the frame is drawn");

    assert_eq!(
        String::from_utf8_lossy(&printed),
        "the last thing it said\n",
        "the program's words are printed on the screen the session ended on"
    );
}
