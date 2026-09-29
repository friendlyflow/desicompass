//! The superkey's end of the channel to desicompass.
//!
//! The compositor starts the superkey with the fd number in
//! `DESICOMPASS_SUPERKEY_FD`. A reader thread turns what arrives into
//! [`ToSuperkey`] messages on a channel, which the render loop drains once per
//! frame; writes are small and go straight out.

use std::io::{Read, Write};
use std::os::fd::{FromRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};

use desicompass_superkey_protocol::{FromSuperkey, LineDecoder, ToSuperkey, VERSION, encode};

pub struct Ipc {
    writer: std::sync::Mutex<UnixStream>,
    rx: std::sync::Mutex<Receiver<ToSuperkey>>,
    closed: Arc<AtomicBool>,
    /// Messages the reader has queued, and messages drained: the channel is
    /// finished only once the two agree.
    queued: Arc<AtomicUsize>,
    drained: AtomicUsize,
}

impl Ipc {
    /// Take over the inherited descriptor `fd`.
    ///
    /// Checked before it is wrapped: a stale or mistyped number would
    /// otherwise have this process close some other file on drop.
    pub fn from_fd(fd: RawFd) -> std::io::Result<Self> {
        // SAFETY: F_GETFD only reads the descriptor's flags.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        if flags < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // Close-on-exec from here on: the power commands this process starts
        // must not inherit the channel.
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
        let queued = Arc::new(AtomicUsize::new(0));
        let queued_by_reader = Arc::clone(&queued);
        std::thread::Builder::new()
            .name("superkey-ipc".into())
            .spawn(move || {
                let mut decoder = LineDecoder::new();
                let mut buf = [0u8; 4096];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            for msg in decoder.feed::<ToSuperkey>(&buf[..n]) {
                                if tx.send(msg).is_err() {
                                    return;
                                }
                                queued_by_reader.fetch_add(1, Ordering::Release);
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                        Err(e) => {
                            tracing::warn!("superkey channel read failed: {e}");
                            break;
                        }
                    }
                }
                tracing::info!("the compositor closed the superkey channel");
                closed_by_reader.store(true, Ordering::Release);
            })?;
        let ipc = Self {
            writer: std::sync::Mutex::new(stream),
            rx: std::sync::Mutex::new(rx),
            closed,
            queued,
            drained: AtomicUsize::new(0),
        };
        ipc.send(&FromSuperkey::Hello { version: VERSION });
        Ok(ipc)
    }

    /// Send one message. A failure is logged: the compositor going away is
    /// noticed by the reader, which ends the superkey.
    pub fn send(&self, msg: &FromSuperkey) {
        let mut w = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        if let Err(e) = w.write_all(&encode(msg)) {
            tracing::warn!("could not tell the compositor {msg:?}: {e}");
        }
    }

    /// Everything that has arrived since the last call.
    pub fn drain(&self) -> Vec<ToSuperkey> {
        let rx = self.rx.lock().unwrap_or_else(|e| e.into_inner());
        let mut out = Vec::new();
        while let Ok(m) = rx.try_recv() {
            self.drained.fetch_add(1, Ordering::Relaxed);
            out.push(m);
        }
        out
    }

    /// Whether the compositor has closed its end. Nothing more will come, and
    /// the superkey should exit.
    pub fn is_closed(&self) -> bool {
        // Only once everything that did arrive has been drained. `closed` is
        // set after the last message was queued, so reading it first means
        // `queued` is final by the time it is compared.
        self.closed.load(Ordering::Acquire)
            && self.queued.load(Ordering::Acquire) == self.drained.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use desicompass_superkey_protocol::{Section, WindowInfo};
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
    fn it_says_hello_and_hears_the_compositor() {
        let (compositor, superkey) = UnixStream::pair().unwrap();
        let ipc = Ipc::from_stream(superkey).unwrap();

        let mut got = Vec::new();
        let mut dec = LineDecoder::new();
        let mut buf = [0u8; 256];
        let n = (&compositor).read(&mut buf).unwrap();
        got.extend(dec.feed::<FromSuperkey>(&buf[..n]));
        assert_eq!(got, vec![FromSuperkey::Hello { version: VERSION }]);

        let show = ToSuperkey::Show {
            section: Section::Windows,
            windows: vec![WindowInfo::new(1, "foot", "foot", true)],
        };
        (&compositor).write_all(&encode(&show)).unwrap();
        let msgs = wait_for(|| Some(ipc.drain()).filter(|m| !m.is_empty()));
        assert_eq!(msgs, vec![show]);
        assert!(!ipc.is_closed());
    }

    #[test]
    fn a_closed_channel_is_noticed_after_the_last_message() {
        let (compositor, superkey) = UnixStream::pair().unwrap();
        let ipc = Ipc::from_stream(superkey).unwrap();
        (&compositor)
            .write_all(&encode(&ToSuperkey::Hidden))
            .unwrap();
        drop(compositor);
        wait_for(|| ipc.closed.load(Ordering::Relaxed).then_some(()));
        assert!(!ipc.is_closed(), "the last message is still to be drained");
        assert_eq!(ipc.drain(), vec![ToSuperkey::Hidden]);
        assert!(ipc.is_closed());
    }

    #[test]
    fn a_bad_fd_is_refused_not_adopted() {
        assert!(Ipc::from_fd(987_654).is_err());
    }
}
