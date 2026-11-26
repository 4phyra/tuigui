//! A scripted session for the frame-path test: a windowing backend with no program, no
//! compositor and no terminal, that draws a fresh frame every turn it is given.
//!
//! This file is only compiled under the `test-windowing` feature, and only wired in under
//! `cfg(test)` inside the crate: it exists so the client's own session loop can be driven
//! from a test, which needs a `Compositable` window and a `WindowingServer` that both stand
//! up without a display server. Everything the client observes behaves as it does in the
//! real backend — a window is offered, it draws every frame, and it reports being done when
//! the test says so through [`stop`].
//!
//! The counters are process-global because the window and the test live in the same process
//! but talk through the client's event loop, not through any channel the test holds; reading
//! them is how the test knows what the window did.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::windowing::{Compositable, WindowingServer};

static FRAMES_SENT: AtomicU64 = AtomicU64::new(0);
static STOP: AtomicBool = AtomicBool::new(false);
/// How many frames the window handed over.
pub fn frames_sent() -> u64 {
    FRAMES_SENT.load(Ordering::SeqCst)
}

/// The window reports no longer running, which is what ends the session.
pub fn stop() {
    STOP.store(true, Ordering::SeqCst);
}

/// Zero everything for a fresh run.
pub fn reset() {
    FRAMES_SENT.store(0, Ordering::SeqCst);
    STOP.store(false, Ordering::SeqCst);
}

/// The scripted session: no program is spawned, the window always exists.
pub struct TestingWindowing {
    stdout: Option<std::io::PipeReader>,
    /// Kept so the reader never sees the pipe close while the session runs; the session is
    /// ended by [`stop`], not by the program's output ending.
    _stdout: Option<std::io::PipeWriter>,
}

impl TestingWindowing {
    pub fn new(_options: crate::windowing::Options) -> std::io::Result<Self> {
        let (reader, writer) = std::io::pipe()?;
        Ok(Self {
            stdout: Some(reader),
            _stdout: Some(writer),
        })
    }

    /// Hand over the one window the scripted session offers.
    pub fn take_root(
        &mut self,
    ) -> std::io::Result<impl std::future::Future<Output = std::io::Result<Box<dyn Compositable>>>> {
        Ok(async move { Ok(Box::new(TestWindow { frame_no: AtomicU64::new(0) }) as Box<dyn Compositable>) })
    }
}

impl WindowingServer for TestingWindowing {
    fn spawn(&mut self, _path: &std::path::Path) -> std::io::Result<Option<std::io::PipeReader>> {
        // The program is not started for real; the pipe handed out in `new` is the reader
        // `take_stdout` will return, kept here until then.
        Ok(None)
    }

    fn take_stdout(&mut self) -> Option<std::io::PipeReader> {
        self.stdout.take()
    }

    fn advance(&mut self, _timeout: std::time::Duration) {}

    fn is_running(&self) -> bool {
        !STOP.load(Ordering::SeqCst)
    }
}

/// The window the session places and draws.
struct TestWindow {
    frame_no: AtomicU64,
}

#[async_trait::async_trait]
impl Compositable for TestWindow {
    fn key_input(&self, _event: termina::event::KeyEvent) -> std::io::Result<()> {
        Ok(())
    }

    fn pointer_input(&self, _event: termina::event::MouseEvent) -> std::io::Result<()> {
        Ok(())
    }

    /// Draw a picture every time the client asks for one. The pixels change every call, so
    /// every frame is new work for the codec, like a program that redraws continuously.
    async fn capture(&self) -> std::io::Result<Option<Vec<u8>>> {
        tokio::time::sleep(std::time::Duration::from_millis(16)).await;
        let n = self.frame_no.fetch_add(1, Ordering::Relaxed);
        const WIDTH: u32 = 96;
        const HEIGHT: u32 = 64;
        let mut pixels = vec![0u8; (WIDTH * HEIGHT * 4) as usize];
        for (index, pixel) in pixels.chunks_exact_mut(4).enumerate() {
            let v = (index as u32).wrapping_mul(2654435761).wrapping_add(n as u32);
            pixel[0] = (v & 0xff) as u8;
            pixel[1] = ((v >> 8) & 0xff) as u8;
            pixel[2] = ((v >> 16) & 0xff) as u8;
            pixel[3] = 0xff;
        }
        FRAMES_SENT.fetch_add(1, Ordering::Relaxed);
        Ok(Some(pixels))
    }

    fn size_px(&self) -> std::io::Result<(u32, u32)> {
        Ok((96, 64))
    }

    fn set_size(&self, _size: (u32, u32)) -> std::io::Result<()> {
        Ok(())
    }

    fn is_popup(&self) -> bool {
        false
    }

    fn title(&self) -> std::io::Result<String> {
        Ok("test window".to_string())
    }

    async fn recv_new_child(&self) -> Option<Box<dyn Compositable>> {
        std::future::pending().await
    }

    async fn recv_new_popup(&self) -> Option<Box<dyn Compositable>> {
        std::future::pending().await
    }

    fn terminate(&self) -> std::io::Result<()> {
        Ok(())
    }
}
