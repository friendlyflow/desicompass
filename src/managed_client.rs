//! A helper process the compositor starts and keeps running itself: the
//! superkey, the bar.
//!
//! Each is a Wayland client over a socketpair the compositor inserted, so its
//! surfaces are known by `ClientId` (an app_id would do the same job, but any
//! client can claim any app_id), and each has a second socketpair for its own
//! newline-delimited JSON protocol. When one exits it is started again, with
//! a back-off, until it has died too often to be worth it.
//!
//! What its window is and does stays in its own module (`superkey.rs`,
//! `bar.rs`). This is only the process, the channel and the restarts.

use std::fmt::Debug;
use std::io::{ErrorKind, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command};
use std::sync::Arc;
use std::time::{Duration, Instant};

use desicompass_superkey_protocol::{LineDecoder, encode};
use serde::Serialize;
use serde::de::DeserializeOwned;
use smithay::reexports::{
    calloop::{Interest, Mode, PostAction, RegistrationToken, generic::Generic},
    wayland_server::backend::ClientId,
};
use tracing::{debug, error, info, warn};

use crate::state::{ClientState, State};

/// Restarts allowed within [`RESTART_WINDOW`] before the compositor stops
/// trying. A helper that crashes on start would otherwise be relaunched
/// forever, each time taking a Vulkan device.
pub const MAX_RESTARTS: usize = 5;
pub const RESTART_WINDOW: Duration = Duration::from_secs(60);

/// The process, its Wayland client and its channel.
pub struct ManagedClient {
    /// For the log: "superkey", "bar".
    name: &'static str,
    /// The shell command that starts it. `None`: there is none, and nothing
    /// is started.
    pub cmd: Option<String>,
    pub child: Option<Child>,
    pub client: Option<ClientId>,
    pub ipc: Option<UnixStream>,
    pub ipc_source: Option<RegistrationToken>,
    /// Bytes not yet written, because the socket was full.
    pub out: Vec<u8>,
    /// When it was (re)started, for the restart limit.
    pub starts: Vec<Instant>,
    /// When to start it next. `None` before the first start and after giving up.
    pub next_start: Option<Instant>,
}

impl ManagedClient {
    pub fn new(name: &'static str, cmd: Option<String>) -> Self {
        let cmd = cmd.filter(|c| !c.trim().is_empty());
        Self {
            name,
            next_start: cmd.as_ref().map(|_| Instant::now()),
            cmd,
            child: None,
            client: None,
            ipc: None,
            ipc_source: None,
            out: Vec::new(),
            starts: Vec::new(),
        }
    }

    pub fn is_client(&self, client: Option<ClientId>) -> bool {
        client.is_some() && client == self.client
    }

    /// Queue one message and write what the socket takes.
    pub fn send<T: Serialize>(&mut self, msg: &T) {
        if self.ipc.is_none() {
            return;
        }
        self.out.extend(encode(msg));
        self.flush();
    }

    /// Write what is queued, as far as the socket takes it. Never blocks: a
    /// helper that stops reading must not stop the compositor.
    pub fn flush(&mut self) {
        let Some(ipc) = self.ipc.as_mut() else {
            self.out.clear();
            return;
        };
        while !self.out.is_empty() {
            match ipc.write(&self.out) {
                Ok(0) => break,
                Ok(n) => {
                    self.out.drain(..n);
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => {
                    debug!("{} ipc write failed: {e}", self.name);
                    self.out.clear();
                    break;
                }
            }
        }
    }

    /// Whether its process has exited since the last look.
    pub fn exited(&mut self) -> bool {
        let Some(child) = self.child.as_mut() else {
            return false;
        };
        match child.try_wait() {
            Ok(Some(status)) => {
                warn!("the {} exited ({status})", self.name);
                true
            }
            Ok(None) => false,
            Err(e) => {
                debug!("{} wait failed: {e}", self.name);
                false
            }
        }
    }

    /// Whether it is time to start it.
    pub fn due(&self, now: Instant) -> bool {
        self.child.is_none() && self.next_start.is_some_and(|t| now >= t)
    }
}

/// The shell command that starts a helper, or `None` for none.
///
/// `env` wins when set (empty turns the helper off), otherwise `binary` next
/// to this executable (what `cargo build --workspace` produces), else on
/// `PATH`.
pub fn resolve(env: &str, binary: &str, what: &str) -> Option<String> {
    let explicit = std::env::var_os(env).map(|v| v.to_string_lossy().into_owned());
    let own_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf));
    let path = std::env::var_os("PATH");
    let found = resolve_with(
        binary,
        explicit,
        own_dir.as_deref(),
        path.as_deref(),
        is_executable,
    );
    match &found {
        Some(cmd) => info!("{what}: {cmd}"),
        None => info!("no {binary} found, running without the {what}"),
    }
    found
}

/// [`resolve`], with the filesystem passed in.
pub fn resolve_with(
    binary: &str,
    explicit: Option<String>,
    own_dir: Option<&Path>,
    path: Option<&std::ffi::OsStr>,
    executable: impl Fn(&Path) -> bool,
) -> Option<String> {
    if let Some(cmd) = explicit {
        return (!cmd.trim().is_empty()).then_some(cmd);
    }
    if let Some(p) = own_dir.map(|d| d.join(binary)).filter(|p| executable(p)) {
        return Some(shell_quote(&p.to_string_lossy()));
    }
    let on_path = path
        .map(|p| std::env::split_paths(p).any(|d| executable(&d.join(binary))))
        .unwrap_or(false);
    on_path.then(|| binary.to_owned())
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// A path as one `sh -c` word.
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// How long to wait before starting a helper again, given when it was started
/// before. `None`: it keeps dying, stop trying.
///
/// The wait doubles with every recent start (250 ms, 500 ms, 1 s...), so a
/// helper that dies once is back at once and one that dies on start does not
/// spin.
pub fn respawn_delay(starts: &[Instant], now: Instant) -> Option<Duration> {
    let recent = starts
        .iter()
        .filter(|&&t| now.duration_since(t) < RESTART_WINDOW)
        .count();
    if recent >= MAX_RESTARTS {
        return None;
    }
    Some(Duration::from_millis(250) * 2u32.pow(recent.saturating_sub(1) as u32))
}

/// The environment a helper is started with, besides the inherited one.
pub fn child_env(
    socket_name: &str,
    wayland_fd: RawFd,
    ipc_env: &'static str,
    ipc_fd: RawFd,
) -> Vec<(&'static str, String)> {
    vec![
        // libwayland connects to this fd rather than to WAYLAND_DISPLAY.
        ("WAYLAND_SOCKET", wayland_fd.to_string()),
        // Still set, because SDL checks it before it tries Wayland at all.
        ("WAYLAND_DISPLAY", socket_name.to_owned()),
        // Load-bearing. SDL 3.4 first tries a "preferred" Wayland start that
        // wants wp_fifo_v1, and when this compositor lacks it SDL disconnects
        // and connects again. libwayland unsets WAYLAND_SOCKET on the first
        // connect, so the second one would reach the public socket as an
        // ordinary client, and the helper would be tiled like any window.
        // Naming the driver skips the preferred attempt.
        ("SDL_VIDEO_DRIVER", "wayland".to_owned()),
        (ipc_env, ipc_fd.to_string()),
        ("SICOMPASS_SESSION", "1".to_owned()),
    ]
}

/// Let a child inherit `fd`: `UnixStream` sets close-on-exec.
pub fn clear_cloexec(fd: RawFd) -> std::io::Result<()> {
    // SAFETY: fcntl on an fd this process owns; async-signal-safe, so fine
    // between fork and exec.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Picks one helper out of the compositor state.
pub type Which = fn(&mut State) -> &mut ManagedClient;

impl State {
    /// Start the helper `which` names: its Wayland client, its channel, its
    /// process. Messages it sends go to `on_message`.
    pub fn start_managed<M>(
        &mut self,
        which: Which,
        ipc_env: &'static str,
        on_message: fn(&mut State, M),
    ) -> std::io::Result<()>
    where
        M: DeserializeOwned + Debug + 'static,
    {
        let Some(cmd) = which(self).cmd.clone() else {
            return Ok(());
        };
        let name = which(self).name;
        let (wayland_ours, wayland_theirs) = UnixStream::pair()?;
        let (ipc_ours, ipc_theirs) = UnixStream::pair()?;

        let client = self
            .display_handle
            .insert_client(wayland_ours, Arc::new(ClientState::default()))?;

        let wayland_fd = wayland_theirs.as_raw_fd();
        let ipc_fd = ipc_theirs.as_raw_fd();
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", &cmd])
            .envs(child_env(&self.socket_name, wayland_fd, ipc_env, ipc_fd))
            .env_remove("DISPLAY");
        // SAFETY: only async-signal-safe calls (fcntl) run in the child.
        unsafe {
            command.pre_exec(move || {
                clear_cloexec(wayland_fd)?;
                clear_cloexec(ipc_fd)
            });
        }
        // On failure the child's ends are dropped on return, so the client
        // just inserted sees its socket close and goes away by itself.
        let child = command.spawn()?;
        // The child has its copies now.
        drop(wayland_theirs);
        drop(ipc_theirs);

        ipc_ours.set_nonblocking(true)?;
        let reader = ipc_ours.try_clone()?;
        let mut decoder = LineDecoder::new();
        let token = self
            .loop_handle
            .insert_source(
                Generic::new(reader, Interest::READ, Mode::Level),
                move |_, stream, state: &mut State| {
                    let mut buf = [0u8; 4096];
                    loop {
                        match stream.as_ref().read(&mut buf) {
                            Ok(0) => {
                                debug!("the {name} closed its channel");
                                let m = which(state);
                                m.ipc = None;
                                m.ipc_source = None;
                                return Ok(PostAction::Remove);
                            }
                            Ok(n) => {
                                for msg in decoder.feed::<M>(&buf[..n]) {
                                    debug!("{name}: {msg:?}");
                                    on_message(state, msg);
                                }
                            }
                            Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                            Err(e) if e.kind() == ErrorKind::Interrupted => {}
                            Err(e) => {
                                debug!("{name} ipc read failed: {e}");
                                let m = which(state);
                                m.ipc = None;
                                m.ipc_source = None;
                                return Ok(PostAction::Remove);
                            }
                        }
                    }
                    Ok(PostAction::Continue)
                },
            )
            .map_err(|e| std::io::Error::other(e.to_string()))?;

        info!("started the {name} (pid {})", child.id());
        let m = which(self);
        m.starts.push(Instant::now());
        m.child = Some(child);
        m.client = Some(client.id());
        m.ipc = Some(ipc_ours);
        m.ipc_source = Some(token);
        m.out.clear();
        Ok(())
    }

    /// The helper's process is gone: forget it, and schedule the next start.
    /// What it had on screen is the caller's to clear.
    pub fn managed_gone(&mut self, which: Which, now: Instant) {
        let running = self.running;
        let token = {
            let m = which(self);
            if let Some(mut child) = m.child.take() {
                let _ = child.try_wait();
            }
            m.ipc = None;
            m.client = None;
            m.ipc_source.take()
        };
        if let Some(token) = token {
            self.loop_handle.remove(token);
        }
        let m = which(self);
        match respawn_delay(&m.starts, now) {
            Some(d) if running => m.next_start = Some(now + d),
            Some(_) => {}
            None => error!(
                "the {} keeps exiting ({MAX_RESTARTS} starts in {}s); not starting it again",
                m.name,
                RESTART_WINDOW.as_secs()
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_helper_without_a_command_is_never_due() {
        let later = || Instant::now() + Duration::from_secs(1);
        assert!(!ManagedClient::new("bar", None).due(later()));
        assert!(!ManagedClient::new("bar", Some(" ".into())).due(later()));
        assert!(ManagedClient::new("bar", Some("desicompass-bar".into())).due(later()));
    }

    #[test]
    fn each_helper_finds_its_own_binary() {
        let exists = |p: &Path| p == Path::new("/own/desicompass-bar");
        assert_eq!(
            resolve_with(
                "desicompass-bar",
                None,
                Some(Path::new("/own")),
                None,
                exists
            )
            .as_deref(),
            Some("'/own/desicompass-bar'")
        );
        assert_eq!(
            resolve_with(
                "desicompass-superkey",
                None,
                Some(Path::new("/own")),
                None,
                exists
            ),
            None
        );
    }

    #[test]
    fn the_channel_variable_is_the_helpers_own() {
        let env = child_env("wayland-3", 7, "DESICOMPASS_BAR_FD", 9);
        let get = |k: &str| env.iter().find(|(n, _)| *n == k).map(|(_, v)| v.as_str());
        assert_eq!(get("DESICOMPASS_BAR_FD"), Some("9"));
        assert_eq!(get("DESICOMPASS_SUPERKEY_FD"), None);
        assert_eq!(get("WAYLAND_SOCKET"), Some("7"));
    }
}
