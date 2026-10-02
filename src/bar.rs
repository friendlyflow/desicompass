//! The bar: a strip along the top or bottom of the output (desicompass-bar,
//! drawn by sicompass-ui) with the clock and the session's status icons.
//!
//! Started and restarted like the superkey (see `managed_client.rs`), and
//! known by its `ClientId` the same way. Unlike the superkey it is always on
//! screen, and it never takes the keyboard: there is nothing in it to type
//! into. The windows are tiled in what it leaves of the output
//! ([`usable_area`]).
//!
//! It tells the compositor where it wants to be and how tall it is
//! (`place`), and the compositor tells it to say the time and the date
//! (Super+D). See
//! `docs/bar.md`.

use std::ops::{Deref, DerefMut};
use std::time::Instant;

use desicompass_bar_protocol::{ENV_IPC_FD, Edge, FromBar, ToBar, VERSION};
use smithay::{
    desktop::Window,
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::{backend::ClientId, protocol::wl_surface::WlSurface},
    },
    utils::{Logical, Point, Rectangle, Size},
    wayland::{compositor::with_states, shell::xdg::XdgToplevelSurfaceData},
};
use tracing::{error, info, warn};

use crate::managed_client::ManagedClient;
use crate::state::State;

/// The executable the bar is when nobody names one.
pub const BINARY: &str = "desicompass-bar";

/// The environment variable naming the bar command. Set, even to nothing, it
/// wins over the search: the Nix package points it at its own build, and the
/// login screen's compositor sets it empty (the login screen has a clock of
/// its own, and nothing else in a bar is anyone's before signing in).
pub const ENV_COMMAND: &str = "DESICOMPASS_BAR";

/// How tall the bar is until it says: 1.7 lines at the default font scale.
pub const DEFAULT_HEIGHT: i32 = 48;

/// Everything the compositor keeps about its bar. The process, its client and
/// its channel are the [`ManagedClient`] it derefs to.
pub struct Bar {
    process: ManagedClient,
    window: Option<Window>,
    edge: Edge,
    height: i32,
}

impl Deref for Bar {
    type Target = ManagedClient;
    fn deref(&self) -> &ManagedClient {
        &self.process
    }
}

impl DerefMut for Bar {
    fn deref_mut(&mut self) -> &mut ManagedClient {
        &mut self.process
    }
}

impl Bar {
    pub fn new(cmd: Option<String>) -> Self {
        Self {
            process: ManagedClient::new("bar", cmd),
            window: None,
            edge: Edge::default(),
            height: DEFAULT_HEIGHT,
        }
    }

    pub fn is_bar_client(&self, client: Option<ClientId>) -> bool {
        self.is_client(client)
    }

    pub fn window(&self) -> Option<&Window> {
        self.window.as_ref()
    }

    /// Whether `surface` is the bar's toplevel.
    pub fn owns_surface(&self, surface: &WlSurface) -> bool {
        self.window
            .as_ref()
            .and_then(|w| w.toplevel())
            .is_some_and(|t| t.wl_surface() == surface)
    }

    /// The strip it takes from the output: only once it has a window.
    pub fn strip(&self) -> Option<(Edge, i32)> {
        self.window.as_ref().map(|_| (self.edge, self.height))
    }
}

/// The shell command that starts the bar, or `None` for no bar: the same
/// search as the superkey's (`managed_client::resolve`).
pub fn resolve_command() -> Option<String> {
    crate::managed_client::resolve(ENV_COMMAND, BINARY, "bar")
}

/// A bar height the output can take: at least one pixel (never a 0-high
/// configure), at most half the output.
fn clamp_height(output: Size<i32, Logical>, height: i32) -> i32 {
    height.clamp(1, (output.h / 2).max(1))
}

/// Where the bar goes on an output of `output` size.
pub fn bar_rect(output: Size<i32, Logical>, edge: Edge, height: i32) -> Rectangle<i32, Logical> {
    let h = clamp_height(output, height);
    let y = match edge {
        Edge::Top => 0,
        Edge::Bottom => output.h - h,
    };
    Rectangle::new(Point::from((0, y)), Size::from((output.w, h)))
}

/// What the tiles get: the output, less the bar's strip when there is one.
pub fn usable_area(
    output: Size<i32, Logical>,
    strip: Option<(Edge, i32)>,
) -> Rectangle<i32, Logical> {
    let Some((edge, height)) = strip else {
        return Rectangle::new(Point::from((0, 0)), output);
    };
    let h = clamp_height(output, height);
    let y = match edge {
        Edge::Top => h,
        Edge::Bottom => 0,
    };
    Rectangle::new(Point::from((0, y)), Size::from((output.w, output.h - h)))
}

impl State {
    /// What the tiles get: see [`usable_area`].
    pub fn usable_area(&self) -> Rectangle<i32, Logical> {
        usable_area(self.output_size(), self.bar.strip())
    }

    /// Once per loop pass: start the bar when it is due, notice when it has
    /// died, and write what the socket would not take earlier.
    pub fn maintain_bar(&mut self) {
        let now = Instant::now();
        if self.bar.exited() {
            self.bar_gone(now);
        }
        if self.bar.due(now) {
            self.bar.next_start = None;
            if let Err(e) =
                self.start_managed(|s| &mut s.bar, ENV_IPC_FD, State::handle_bar_message)
            {
                error!("could not start the bar: {e}");
                self.bar_gone(now);
            }
        }
        self.bar.flush();
    }

    /// The bar process is gone: forget it and its window, give the tiles the
    /// whole output back, and schedule the next start.
    fn bar_gone(&mut self, now: Instant) {
        self.managed_gone(|s| &mut s.bar, now);
        self.bar_window_destroyed();
    }

    /// A toplevel from the bar's client: keep it apart from the tiles.
    pub fn adopt_bar_window(&mut self, window: Window) {
        info!("the bar's window is up");
        if let Some(old) = self.bar.window.replace(window) {
            self.space.unmap_elem(&old);
        }
        self.relayout();
    }

    /// The bar's toplevel went away.
    pub fn bar_window_destroyed(&mut self) {
        if let Some(w) = self.bar.window.take() {
            self.space.unmap_elem(&w);
            self.relayout();
        }
    }

    /// Size the bar for the current output and keep it on its edge, above the
    /// tiles. Part of every relayout, before the superkey, which covers it.
    pub fn place_bar(&mut self) {
        self.configure_bar();
        if let Some(w) = self.bar.window.clone() {
            let rect = bar_rect(self.output_size(), self.bar.edge, self.bar.height);
            self.space.map_element(w.clone(), rect.loc, false);
            self.space.raise_element(&w, false);
        }
    }

    /// The bar's size: the output's width, its own height. Never activated,
    /// since it never has the keyboard.
    fn configure_bar(&mut self) {
        let Some(toplevel) = self.bar.window.as_ref().and_then(|w| w.toplevel()) else {
            return;
        };
        let rect = bar_rect(self.output_size(), self.bar.edge, self.bar.height);
        toplevel.with_pending_state(|state| {
            state.size = Some(rect.size);
            state.bounds = Some(rect.size);
            // Its edges are not its own: no decorations, no resize handles.
            state.states.set(xdg_toplevel::State::TiledLeft);
            state.states.set(xdg_toplevel::State::TiledRight);
            state.states.set(xdg_toplevel::State::TiledTop);
            state.states.set(xdg_toplevel::State::TiledBottom);
        });
        let initial_sent = with_states(toplevel.wl_surface(), |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .map(|data| data.lock().unwrap().initial_configure_sent)
                .unwrap_or(false)
        });
        // Before the initial configure, `ensure_initial_configure` sends it.
        if initial_sent {
            toplevel.send_pending_configure();
        }
    }

    /// The bar's size as its first configure has it.
    pub fn bar_initial_size(&self) -> Size<i32, Logical> {
        bar_rect(self.output_size(), self.bar.edge, self.bar.height).size
    }

    /// Super+D.
    pub fn say_time(&mut self) {
        if self.bar.ipc.is_none() {
            warn!("no bar is running to say the time");
            return;
        }
        self.bar.send(&ToBar::SayTime);
    }

    fn handle_bar_message(&mut self, msg: FromBar) {
        match msg {
            FromBar::Hello { version } => {
                if version != VERSION {
                    warn!("the bar speaks protocol {version}, this compositor {VERSION}");
                }
            }
            FromBar::Place { edge, height } => {
                let height = i32::try_from(height).unwrap_or(i32::MAX).max(1);
                if (edge, height) != (self.bar.edge, self.bar.height) {
                    info!("bar: {} edge, {height} px", edge.as_str());
                    self.bar.edge = edge;
                    self.bar.height = height;
                    self.relayout();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size(w: i32, h: i32) -> Size<i32, Logical> {
        Size::from((w, h))
    }

    #[test]
    fn a_bottom_bar_sits_on_the_bottom_edge_and_the_tiles_above_it() {
        let out = size(1920, 1080);
        let bar = bar_rect(out, Edge::Bottom, 56);
        assert_eq!(
            (bar.loc.x, bar.loc.y, bar.size.w, bar.size.h),
            (0, 1024, 1920, 56)
        );
        let area = usable_area(out, Some((Edge::Bottom, 56)));
        assert_eq!(
            (area.loc.x, area.loc.y, area.size.w, area.size.h),
            (0, 0, 1920, 1024)
        );
    }

    #[test]
    fn a_top_bar_pushes_the_tiles_down() {
        let out = size(1920, 1080);
        let bar = bar_rect(out, Edge::Top, 72);
        assert_eq!((bar.loc.y, bar.size.h), (0, 72));
        let area = usable_area(out, Some((Edge::Top, 72)));
        assert_eq!((area.loc.y, area.size.h), (72, 1008));
    }

    #[test]
    fn no_bar_leaves_the_whole_output() {
        let out = size(800, 600);
        let area = usable_area(out, None);
        assert_eq!((area.loc.x, area.loc.y, area.size), (0, 0, out));
    }

    #[test]
    fn the_bar_never_takes_more_than_half_nor_less_than_a_pixel() {
        let out = size(300, 200);
        assert_eq!(bar_rect(out, Edge::Bottom, 5000).size.h, 100);
        assert_eq!(usable_area(out, Some((Edge::Top, 5000))).size.h, 100);
        assert_eq!(bar_rect(out, Edge::Top, 0).size.h, 1);
        assert_eq!(usable_area(out, Some((Edge::Top, -3))).loc.y, 1);
    }

    #[test]
    fn a_bar_process_without_a_window_takes_no_room() {
        let bar = Bar::new(Some("desicompass-bar".into()));
        assert_eq!(bar.strip(), None);
        assert!(Bar::new(None).next_start.is_none());
    }
}
