//! The bar's end of the channel to desicompass.
//!
//! The compositor starts the bar with the fd number in `DESICOMPASS_BAR_FD`.
//! A reader thread turns what arrives into [`ToBar`] messages on a channel,
//! which the loop drains; writes are small and go straight out. The same
//! shape as the superkey's (`lib_superkey/src/ipc.rs`).

use std::io::{Read, Write};
use std::os::fd::{FromRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};

use desicompass_bar_protocol::{FromBar, LineDecoder, ToBar, VERSION, encode};

pub struct Ipc {
    writer: std::sync::Mutex<UnixStream>,
    rx: Receiver<ToBar>,
    closed: Arc<AtomicBool>,
}

impl Ipc {
    /// Take over the inherited descriptor `fd`, after checking it is one.
    pub fn from_fd(fd: RawFd) -> std::io::Result<Self> {
        // SAFETY: F_GETFD only reads the descriptor's flags.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        if flags < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // Close-on-exec from here on: spd-say and wpctl must not inherit it.
        // SAFETY: as above, on a descriptor that exists.
        unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) };
        // SAFETY: the descriptor exists and was handed to this process to own.
        let stream = unsafe { UnixStream::from_raw_fd(fd) };
        Self::from_stream(stream)
    }

    /// Wrap a connected stream (tests use a socketpair). Starts the reader
    /// thread and says hello.
    pub fn from_stream(stream: UnixStream) -> std::io::Result<Self> {
        let mut reader = stream.try_clone()?;
        let (tx, rx) = channel();
        let closed = Arc::new(AtomicBool::new(false));
        let closed_by_reader = Arc::clone(&closed);
        std::thread::Builder::new()
            .name("bar-ipc".into())
            .spawn(move || {
                let mut decoder = LineDecoder::new();
                let mut buf = [0u8; 4096];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            for msg in decoder.feed::<ToBar>(&buf[..n]) {
                                if tx.send(msg).is_err() {
                                    return;
                                }
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                        Err(e) => {
                            tracing::warn!("bar channel read failed: {e}");
                            break;
                        }
                    }
                }
                tracing::info!("the compositor closed the bar channel");
                closed_by_reader.store(true, Ordering::Release);
            })?;
        let ipc = Self {
            writer: std::sync::Mutex::new(stream),
            rx,
            closed,
        };
        ipc.send(&FromBar::Hello { version: VERSION });
        Ok(ipc)
    }

    pub fn send(&self, msg: &FromBar) {
        let mut w = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        if let Err(e) = w.write_all(&encode(msg)) {
            tracing::warn!("could not tell the compositor {msg:?}: {e}");
        }
    }

    /// Everything that has arrived since the last call.
    pub fn drain(&self) -> Vec<ToBar> {
        self.rx.try_iter().collect()
    }

    /// Whether the compositor has closed its end: the bar should exit.
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use desicompass_bar_protocol::Edge;
    use std::time::{Duration, Instant};

    fn wait_for<T>(mut f: impl FnMut() -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(v) = f() {
                return v;
            }
            assert!(Instant::now() < deadline, "timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn it_says_hello_places_itself_and_hears_the_compositor() {
        let (compositor, bar) = UnixStream::pair().unwrap();
        let ipc = Ipc::from_stream(bar).unwrap();
        ipc.send(&FromBar::Place {
            edge: Edge::Top,
            height: 60,
        });

        let mut dec = LineDecoder::new();
        let mut got: Vec<FromBar> = Vec::new();
        let mut buf = [0u8; 256];
        while got.len() < 2 {
            let n = (&compositor).read(&mut buf).unwrap();
            got.extend(dec.feed::<FromBar>(&buf[..n]));
        }
        assert_eq!(
            got,
            vec![
                FromBar::Hello { version: VERSION },
                FromBar::Place {
                    edge: Edge::Top,
                    height: 60
                }
            ]
        );

        (&compositor).write_all(&encode(&ToBar::SayTime)).unwrap();
        let msgs = wait_for(|| Some(ipc.drain()).filter(|m| !m.is_empty()));
        assert_eq!(msgs, vec![ToBar::SayTime]);
        drop(compositor);
        wait_for(|| ipc.is_closed().then_some(()));
    }

    #[test]
    fn a_bad_fd_is_refused_not_adopted() {
        assert!(Ipc::from_fd(987_654).is_err());
    }
}
