//! The superkey: a window of its own (desicompass-superkey, drawn by
//! sicompass-ui) that the compositor starts once per session and shows on a
//! bare Super tap, Super+W, Super+C or Super+S.
//!
//! It is a client, but not an ordinary one:
//!
//! * The compositor starts it, over a socketpair inserted as a Wayland client,
//!   so its toplevel is recognised by `ClientId`. An app_id would do the same
//!   job, but any client can claim any app_id.
//! * Its toplevel never enters the tiler, the window list or the focus stack.
//!   It fills the output, over the tiles, and is mapped only while shown.
//! * A second socketpair carries the superkey protocol
//!   (`desicompass-superkey-protocol`): the compositor sends the window list
//!   and where to open, the superkey asks to focus a window, start a program,
//!   hide, or end the session.
//!
//! Hiding is the compositor's job: it unmaps the window and tells the superkey,
//! which stops drawing. Nothing is torn down, so showing it again is instant.
//! See `docs/superkey.md`.

use std::io::{ErrorKind, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command};
use std::sync::Arc;
use std::time::{Duration, Instant};

use desicompass_superkey_protocol::{
    ENV_IPC_FD, FromSuperkey, LineDecoder, Section, ToSuperkey, VERSION, WindowInfo, encode,
};
use smithay::{
    desktop::Window,
    reexports::{
        calloop::{Interest, Mode, PostAction, RegistrationToken, generic::Generic},
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::{backend::ClientId, protocol::wl_surface::WlSurface},
    },
    utils::{Logical, Point, Rectangle, Serial, Size},
    wayland::{compositor::with_states, shell::xdg::XdgToplevelSurfaceData},
};
use tracing::{debug, error, info, warn};

use crate::focus::WindowId;
use crate::state::{ClientState, State};

/// How long a show waits for the superkey's first new frame before mapping
/// whatever it last drew. Mapping on that commit is what keeps the previous
/// showing's list from flashing up; the timeout is what keeps a superkey that
/// never draws from never appearing.
const SHOW_FALLBACK: Duration = Duration::from_millis(250);

/// Restarts allowed within [`RESTART_WINDOW`] before the compositor stops
/// trying. A superkey that crashes on start would otherwise be relaunched
/// forever, each time taking a Vulkan device.
const MAX_RESTARTS: usize = 5;
const RESTART_WINDOW: Duration = Duration::from_secs(60);

/// Where the superkey window is on screen, or on its way there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    Hidden,
    /// Asked to show at `since`, and waiting for a frame to map.
    Showing {
        since: Instant,
    },
    Visible,
}

/// Everything the compositor keeps about its superkey.
pub struct Superkey {
    /// The shell command that starts it. `None`: there is no superkey, and
    /// the bindings that would show it do nothing.
    cmd: Option<String>,
    child: Option<Child>,
    client: Option<ClientId>,
    window: Option<Window>,
    ipc: Option<UnixStream>,
    ipc_source: Option<RegistrationToken>,
    /// Bytes not yet written, because the socket was full.
    out: Vec<u8>,
    visibility: Visibility,
    /// Which window had the keyboard when the superkey opened.
    return_focus: Option<WindowId>,
    /// When it was (re)started, for the restart limit.
    starts: Vec<Instant>,
    /// When to start it next. `None` before the first start and after giving up.
    next_start: Option<Instant>,
}

impl Superkey {
    pub fn new(cmd: Option<String>) -> Self {
        let cmd = cmd.filter(|c| !c.trim().is_empty());
        Self {
            next_start: cmd.as_ref().map(|_| Instant::now()),
            cmd,
            child: None,
            client: None,
            window: None,
            ipc: None,
            ipc_source: None,
            out: Vec::new(),
            visibility: Visibility::Hidden,
            return_focus: None,
            starts: Vec::new(),
        }
    }

    /// Whether it is on screen or about to be.
    pub fn is_shown(&self) -> bool {
        self.visibility != Visibility::Hidden
    }

    pub fn is_superkey_client(&self, client: Option<ClientId>) -> bool {
        client.is_some() && client == self.client
    }

    pub fn window(&self) -> Option<&Window> {
        self.window.as_ref()
    }

    /// Whether `surface` is the superkey's toplevel.
    pub fn owns_surface(&self, surface: &WlSurface) -> bool {
        self.window
            .as_ref()
            .and_then(|w| w.toplevel())
            .is_some_and(|t| t.wl_surface() == surface)
    }

    fn send(&mut self, msg: &ToSuperkey) {
        if self.ipc.is_none() {
            return;
        }
        self.out.extend(encode(msg));
        self.flush();
    }

    /// Write what is queued, as far as the socket takes it. Never blocks: a
    /// superkey that stops reading must not stop the compositor.
    fn flush(&mut self) {
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
                    debug!("superkey ipc write failed: {e}");
                    self.out.clear();
                    break;
                }
            }
        }
    }
}

/// The executable the superkey is when nobody names one.
pub const BINARY: &str = "desicompass-superkey";

/// The environment variable naming the superkey command. Set, even to nothing,
/// it wins over the search: the Nix package points it at its own build, and
/// the login screen's compositor sets it empty, because a superkey there would
/// let anyone start programs before signing in. A variable and not a flag on
/// purpose: a compositor older than the superkey ignores it, where an unknown
/// flag would stop it from starting.
pub const ENV_COMMAND: &str = "DESICOMPASS_SUPERKEY";

/// The shell command that starts the superkey, or `None` for no superkey.
///
/// The superkey is part of desicompass, so there is nothing to configure:
/// [`ENV_COMMAND`] wins when set (empty turns the superkey off), otherwise
/// `desicompass-superkey` next to this binary (what `cargo build --workspace`
/// produces), else on `PATH`.
pub fn resolve_command() -> Option<String> {
    let explicit = std::env::var_os(ENV_COMMAND).map(|v| v.to_string_lossy().into_owned());
    let own_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf));
    let path = std::env::var_os("PATH");
    let found = resolve_with(explicit, own_dir.as_deref(), path.as_deref(), is_executable);
    match &found {
        Some(cmd) => info!("superkey: {cmd}"),
        None => info!("no {BINARY} found, running without the superkey"),
    }
    found
}

/// [`resolve_command`], with the filesystem passed in.
fn resolve_with(
    explicit: Option<String>,
    own_dir: Option<&Path>,
    path: Option<&std::ffi::OsStr>,
    executable: impl Fn(&Path) -> bool,
) -> Option<String> {
    if let Some(cmd) = explicit {
        return (!cmd.trim().is_empty()).then_some(cmd);
    }
    if let Some(p) = own_dir.map(|d| d.join(BINARY)).filter(|p| executable(p)) {
        return Some(shell_quote(&p.to_string_lossy()));
    }
    let on_path = path
        .map(|p| std::env::split_paths(p).any(|d| executable(&d.join(BINARY))))
        .unwrap_or(false);
    on_path.then(|| BINARY.to_owned())
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// A path as one `sh -c` word.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The superkey's place on an output of `output` size: all of it. It is a
/// screen of its own, over the windows, not a box among them.
pub fn superkey_rect(output: Size<i32, Logical>) -> Rectangle<i32, Logical> {
    Rectangle::new(Point::from((0, 0)), output)
}

/// How long to wait before starting the superkey again, given when it was
/// started before. `None`: it keeps dying, stop trying.
///
/// The wait doubles with every recent start (250 ms, 500 ms, 1 s...), so a
/// superkey that dies once is back at once and one that dies on start does
/// not spin.
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

/// The environment the superkey is started with, besides the inherited one.
pub fn child_env(
    socket_name: &str,
    wayland_fd: RawFd,
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
        // ordinary client, and the superkey would be tiled like any window.
        // Naming the driver skips the preferred attempt.
        ("SDL_VIDEO_DRIVER", "wayland".to_owned()),
        (ENV_IPC_FD, ipc_fd.to_string()),
        ("SICOMPASS_SESSION", "1".to_owned()),
    ]
}

/// The window list the superkey is sent: most recently used first, the
/// focused window marked.
pub fn window_infos(
    mru: &[WindowId],
    focused: Option<WindowId>,
    describe: impl Fn(WindowId) -> Option<(String, String)>,
) -> Vec<WindowInfo> {
    mru.iter()
        .filter_map(|&id| {
            let (title, app_id) = describe(id)?;
            Some(WindowInfo::new(
                id.0 as u64,
                &title,
                &app_id,
                Some(id) == focused,
            ))
        })
        .collect()
}

/// Let a child inherit `fd`: `UnixStream` sets close-on-exec.
fn clear_cloexec(fd: RawFd) -> std::io::Result<()> {
    // SAFETY: fcntl on an fd this process owns; async-signal-safe, so fine
    // between fork and exec.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

impl State {
    // -----------------------------------------------------------------------
    // Lifecycle
    // -----------------------------------------------------------------------

    /// Once per loop pass: start the superkey when it is due, notice when it
    /// has died, map a show whose frame never came, and write what the socket
    /// would not take earlier.
    pub fn maintain_superkey(&mut self) {
        let now = Instant::now();

        if let Some(child) = self.superkey.child.as_mut() {
            match child.try_wait() {
                Ok(Some(status)) => {
                    warn!("the superkey exited ({status})");
                    self.superkey_gone(now);
                }
                Ok(None) => {}
                Err(e) => debug!("superkey wait failed: {e}"),
            }
        }

        if self.superkey.child.is_none() && self.superkey.next_start.is_some_and(|t| now >= t) {
            self.superkey.next_start = None;
            if let Err(e) = self.start_superkey() {
                error!("could not start the superkey: {e}");
                self.superkey_gone(now);
            }
        }

        if let Visibility::Showing { since } = self.superkey.visibility
            && now.duration_since(since) >= SHOW_FALLBACK
            && self.superkey.window.is_some()
        {
            self.map_superkey();
        }

        self.superkey.flush();
    }

    fn start_superkey(&mut self) -> std::io::Result<()> {
        let Some(cmd) = self.superkey.cmd.clone() else {
            return Ok(());
        };
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
            .envs(child_env(&self.socket_name, wayland_fd, ipc_fd))
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
                                debug!("the superkey closed its channel");
                                state.superkey.ipc = None;
                                state.superkey.ipc_source = None;
                                return Ok(PostAction::Remove);
                            }
                            Ok(n) => {
                                for msg in decoder.feed::<FromSuperkey>(&buf[..n]) {
                                    state.handle_superkey_message(msg);
                                }
                            }
                            Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                            Err(e) if e.kind() == ErrorKind::Interrupted => {}
                            Err(e) => {
                                debug!("superkey ipc read failed: {e}");
                                state.superkey.ipc = None;
                                state.superkey.ipc_source = None;
                                return Ok(PostAction::Remove);
                            }
                        }
                    }
                    Ok(PostAction::Continue)
                },
            )
            .map_err(|e| std::io::Error::other(e.to_string()))?;

        info!("started the superkey (pid {})", child.id());
        self.superkey.starts.push(Instant::now());
        self.superkey.child = Some(child);
        self.superkey.client = Some(client.id());
        self.superkey.ipc = Some(ipc_ours);
        self.superkey.ipc_source = Some(token);
        self.superkey.out.clear();
        Ok(())
    }

    /// The superkey process is gone: forget it, and schedule the next start.
    fn superkey_gone(&mut self, now: Instant) {
        if let Some(mut child) = self.superkey.child.take() {
            let _ = child.try_wait();
        }
        if let Some(token) = self.superkey.ipc_source.take() {
            self.loop_handle.remove(token);
        }
        self.superkey.ipc = None;
        self.superkey.client = None;
        if self.superkey.is_shown() {
            self.hide_superkey(true);
        }
        if let Some(w) = self.superkey.window.take() {
            self.space.unmap_elem(&w);
        }
        match respawn_delay(&self.superkey.starts, now) {
            Some(d) if self.running => self.superkey.next_start = Some(now + d),
            Some(_) => {}
            None => error!(
                "the superkey keeps exiting ({MAX_RESTARTS} starts in {}s); not starting it again",
                RESTART_WINDOW.as_secs()
            ),
        }
    }

    // -----------------------------------------------------------------------
    // Its toplevel
    // -----------------------------------------------------------------------

    /// A toplevel from the superkey's client: keep it apart from the tiles.
    pub fn adopt_superkey_window(&mut self, window: Window) {
        info!("the superkey's window is up");
        if let Some(old) = self.superkey.window.replace(window) {
            self.space.unmap_elem(&old);
        }
        self.configure_superkey();
        if self.superkey.is_shown() {
            self.focus_superkey_surface();
        }
    }

    /// The superkey's toplevel went away (the process usually goes with it).
    pub fn superkey_window_destroyed(&mut self) {
        if self.superkey.is_shown() {
            self.hide_superkey(true);
        }
        if let Some(w) = self.superkey.window.take() {
            self.space.unmap_elem(&w);
        }
    }

    /// Called on every commit of the superkey's surface.
    pub fn superkey_committed(&mut self) {
        if matches!(self.superkey.visibility, Visibility::Showing { .. }) {
            self.map_superkey();
        }
    }

    /// Size the superkey for the current output, and keep it in place and on
    /// top if it is showing. Part of every relayout.
    pub fn place_superkey(&mut self) {
        self.configure_superkey();
        if self.superkey.visibility == Visibility::Visible
            && let Some(w) = self.superkey.window.clone()
        {
            let rect = superkey_rect(self.output_size());
            self.space.map_element(w, rect.loc, true);
        }
    }

    /// Tell the superkey its size: the whole output, full screen, and not
    /// tiled.
    pub fn configure_superkey(&mut self) {
        let Some(toplevel) = self.superkey.window.as_ref().and_then(|w| w.toplevel()) else {
            return;
        };
        let size = superkey_rect(self.output_size()).size;
        let shown = self.superkey.is_shown();
        toplevel.with_pending_state(|state| {
            state.size = Some(size);
            state.bounds = Some(self.output_size());
            // Full screen: no decorations, no edges of its own.
            state.states.set(xdg_toplevel::State::Fullscreen);
            if shown {
                state.states.set(xdg_toplevel::State::Activated);
            } else {
                state.states.unset(xdg_toplevel::State::Activated);
            }
        });
        let initial_sent = with_states(toplevel.wl_surface(), |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .map(|data| data.lock().unwrap().initial_configure_sent)
                .unwrap_or(false)
        });
        // Before the initial configure, `ensure_initial_configure` sends it,
        // and a configure sent now would be the protocol error of configuring
        // a surface that has not committed yet.
        if initial_sent {
            toplevel.send_pending_configure();
        }
    }

    fn map_superkey(&mut self) {
        let Some(window) = self.superkey.window.clone() else {
            return;
        };
        let rect = superkey_rect(self.output_size());
        self.space.map_element(window.clone(), rect.loc, true);
        self.space.raise_element(&window, true);
        self.superkey.visibility = Visibility::Visible;
        self.focus_superkey_surface();
    }

    fn focus_superkey_surface(&mut self) {
        let surface = self
            .superkey
            .window
            .as_ref()
            .and_then(|w| w.toplevel())
            .map(|t| t.wl_surface().clone());
        if surface.is_some()
            && let Some(keyboard) = self.seat.get_keyboard()
        {
            keyboard.set_focus(self, surface, Serial::from(0));
        }
    }

    /// Frame callbacks for the superkey while it is off screen.
    ///
    /// Only mapped windows get them from the render loop, and a Vulkan client
    /// presenting in FIFO mode waits for the callback of its previous frame
    /// before the next one. Without this, the superkey's first frame after a
    /// show would wait for a callback that never comes, and the show would
    /// wait for that frame.
    pub fn send_superkey_frame_if_unmapped(&self) {
        if self.superkey.visibility == Visibility::Visible {
            return;
        }
        if let Some(w) = self.superkey.window.as_ref() {
            let out = self.output.clone();
            w.send_frame(
                &out,
                self.start_time.elapsed(),
                Some(Duration::ZERO),
                |_, _| Some(out.clone()),
            );
        }
    }

    // -----------------------------------------------------------------------
    // Showing and hiding
    // -----------------------------------------------------------------------

    /// Open the superkey on `section`. Showing it again while it is up moves
    /// it to the new section.
    pub fn show_superkey(&mut self, section: Section) {
        if self.superkey.ipc.is_none() {
            warn!("no superkey is running to show");
            return;
        }
        if !self.superkey.is_shown() {
            self.superkey.return_focus = self.focus.focused();
        }
        let windows = self.window_list();
        self.superkey.send(&ToSuperkey::Show { section, windows });
        if !self.superkey.is_shown() {
            self.superkey.visibility = Visibility::Showing {
                since: Instant::now(),
            };
            self.configure_superkey();
        }
        // The keyboard moves now, before the window is on screen, so keys
        // typed straight after the tap are not lost to the previous window.
        self.focus_superkey_surface();
    }

    /// The bare Super tap: open at the root, or close if it is open.
    pub fn toggle_superkey(&mut self) {
        if self.superkey.is_shown() {
            self.hide_superkey(true);
        } else {
            self.show_superkey(Section::Root);
        }
    }

    /// Take the superkey off screen. With `restore`, the keyboard goes back
    /// to the window that had it. Without, the caller is about to focus
    /// something itself.
    pub fn hide_superkey(&mut self, restore: bool) {
        if !self.superkey.is_shown() {
            return;
        }
        self.superkey.visibility = Visibility::Hidden;
        if let Some(w) = self.superkey.window.clone() {
            self.space.unmap_elem(&w);
        }
        self.configure_superkey();
        self.superkey.send(&ToSuperkey::Hidden);
        let back = self.superkey.return_focus.take();
        if !restore {
            return;
        }
        match back
            .filter(|id| self.window(*id).is_some())
            .or(self.focus.focused())
        {
            Some(id) => self.focus_id(id),
            None => {
                if let Some(keyboard) = self.seat.get_keyboard() {
                    keyboard.set_focus(self, None, Serial::from(0));
                }
            }
        }
    }

    /// A window opened while the superkey is up. It does not take the
    /// keyboard from the superkey, but it is where the keyboard goes when
    /// the superkey closes, and it joins the list.
    pub fn superkey_window_added(&mut self, id: WindowId) {
        self.superkey.return_focus = Some(id);
        self.send_window_list();
    }

    /// A window closed while the superkey is up.
    pub fn superkey_window_removed(&mut self, id: WindowId) {
        if self.superkey.return_focus == Some(id) {
            self.superkey.return_focus = self.focus.focused();
        }
        self.send_window_list();
    }

    /// Send the window list again, if the superkey is showing it.
    pub fn send_window_list(&mut self) {
        if !self.superkey.is_shown() {
            return;
        }
        let windows = self.window_list();
        self.superkey.send(&ToSuperkey::Windows { windows });
    }

    fn window_list(&self) -> Vec<WindowInfo> {
        let focused = self.superkey.return_focus.or(self.focus.focused());
        // The focus stack is most recently used first, and holds every window.
        window_infos(self.focus.as_slice(), focused, |id| {
            let toplevel = self.window(id)?.toplevel()?.clone();
            Some(with_states(toplevel.wl_surface(), |states| {
                states
                    .data_map
                    .get::<XdgToplevelSurfaceData>()
                    .map(|d| {
                        let d = d.lock().unwrap();
                        (
                            d.title.clone().unwrap_or_default(),
                            d.app_id.clone().unwrap_or_default(),
                        )
                    })
                    .unwrap_or_default()
            }))
        })
    }

    // -----------------------------------------------------------------------
    // What the superkey asks for
    // -----------------------------------------------------------------------

    fn handle_superkey_message(&mut self, msg: FromSuperkey) {
        debug!("superkey: {msg:?}");
        match msg {
            FromSuperkey::Hello { version } => {
                if version != VERSION {
                    warn!("the superkey speaks protocol {version}, this compositor {VERSION}");
                }
            }
            FromSuperkey::Focus { id } => {
                let id = WindowId(id as usize);
                if self.window(id).is_some() {
                    self.hide_superkey(false);
                    self.focus_id(id);
                } else {
                    // Closed since the list was sent.
                    self.hide_superkey(true);
                }
            }
            FromSuperkey::Spawn { argv, cwd } => {
                self.hide_superkey(true);
                self.spawn_argv(&argv, cwd.as_deref());
            }
            FromSuperkey::Hide => self.hide_superkey(true),
            FromSuperkey::QuitSession => {
                info!("the superkey ended the session");
                self.running = false;
                self.loop_signal.stop();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exists(set: &'static [&'static str]) -> impl Fn(&Path) -> bool {
        move |p| set.iter().any(|s| Path::new(s) == p)
    }

    #[test]
    fn an_explicit_command_wins_and_empty_turns_it_off() {
        let all = exists(&["/own/desicompass-superkey", "/bin/desicompass-superkey"]);
        let own = Some(Path::new("/own"));
        assert_eq!(
            resolve_with(Some("my-superkey --x".into()), own, None, &all).as_deref(),
            Some("my-superkey --x")
        );
        assert_eq!(resolve_with(Some("  ".into()), own, None, &all), None);
    }

    #[test]
    fn next_to_the_compositor_comes_before_path() {
        let all = exists(&["/own dir/desicompass-superkey", "/bin/desicompass-superkey"]);
        let path = std::ffi::OsString::from("/bin");
        assert_eq!(
            resolve_with(None, Some(Path::new("/own dir")), Some(&path), &all).as_deref(),
            Some("'/own dir/desicompass-superkey'")
        );
    }

    #[test]
    fn then_path_then_nothing() {
        let path = std::ffi::OsString::from("/usr/bin:/bin");
        let on_path = exists(&["/bin/desicompass-superkey"]);
        assert_eq!(
            resolve_with(None, Some(Path::new("/own")), Some(&path), &on_path).as_deref(),
            Some("desicompass-superkey")
        );
        assert_eq!(
            resolve_with(None, Some(Path::new("/own")), Some(&path), exists(&[])),
            None
        );
    }

    #[test]
    fn a_quoted_path_survives_sh() {
        let q = shell_quote("/a b/it's");
        let out = Command::new("/bin/sh")
            .args(["-c", &format!("printf %s {q}")])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8(out.stdout).unwrap(), "/a b/it's");
    }

    #[test]
    fn the_superkey_fills_the_output() {
        for (w, h) in [(1920, 1080), (640, 400), (300, 200)] {
            let r = superkey_rect(Size::from((w, h)));
            assert_eq!(r.size, Size::from((w, h)));
            assert_eq!(r.loc, Point::from((0, 0)));
        }
    }

    #[test]
    fn restarts_back_off_and_then_stop() {
        let now = Instant::now();
        assert_eq!(respawn_delay(&[], now), Some(Duration::from_millis(250)));
        let one = [now];
        assert_eq!(respawn_delay(&one, now), Some(Duration::from_millis(250)));
        let three = [now; 3];
        assert_eq!(respawn_delay(&three, now), Some(Duration::from_secs(1)));
        let five = [now; MAX_RESTARTS];
        assert_eq!(respawn_delay(&five, now), None);
    }

    #[test]
    fn old_restarts_are_forgiven() {
        let now = Instant::now();
        let later = now + RESTART_WINDOW + Duration::from_secs(1);
        assert_eq!(
            respawn_delay(&[now; MAX_RESTARTS], later),
            Some(Duration::from_millis(250))
        );
    }

    #[test]
    fn the_child_gets_both_sockets_and_the_sdl_driver() {
        let env = child_env("wayland-3", 7, 9);
        let get = |k: &str| env.iter().find(|(n, _)| *n == k).map(|(_, v)| v.as_str());
        assert_eq!(get("WAYLAND_SOCKET"), Some("7"));
        assert_eq!(get("WAYLAND_DISPLAY"), Some("wayland-3"));
        assert_eq!(get(ENV_IPC_FD), Some("9"));
        assert_eq!(get("SDL_VIDEO_DRIVER"), Some("wayland"));
        assert_eq!(get("SICOMPASS_SESSION"), Some("1"));
        assert_eq!(get("DISPLAY"), None);
    }

    #[test]
    fn a_cleared_cloexec_fd_reaches_the_child_through_sh() {
        let (ours, theirs) = UnixStream::pair().unwrap();
        let fd = theirs.as_raw_fd();
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", &format!("echo ok >&{fd}")]);
        unsafe {
            cmd.pre_exec(move || clear_cloexec(fd));
        }
        let status = cmd.status().unwrap();
        drop(theirs);
        assert!(status.success());
        let mut got = String::new();
        (&ours).read_to_string(&mut got).unwrap();
        assert_eq!(got, "ok\n");
    }

    #[test]
    fn the_window_list_is_most_recent_first_with_the_focused_one_marked() {
        let mru = [WindowId(4), WindowId(1), WindowId(9)];
        let list = window_infos(&mru, Some(WindowId(4)), |id| match id.0 {
            4 => Some(("vim".into(), "foot".into())),
            1 => Some(("Inbox".into(), "sicompass".into())),
            _ => None, // already gone: left out
        });
        assert_eq!(
            list,
            vec![
                WindowInfo::new(4, "vim", "foot", true),
                WindowInfo::new(1, "Inbox", "sicompass", false),
            ]
        );
    }

    #[test]
    fn an_empty_or_blank_command_means_no_superkey() {
        assert!(Superkey::new(None).next_start.is_none());
        assert!(Superkey::new(Some("  ".into())).next_start.is_none());
        assert!(
            Superkey::new(Some("desicompass-superkey".into()))
                .next_start
                .is_some()
        );
    }
}
