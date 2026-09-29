//! The superkey as the user meets it: the real provider and hooks, driven
//! through a headless renderer, with a socketpair standing in for desicompass
//! and a temporary directory for the shared accessibility object.
//!
//! The renderer is `AppRenderer::new()` with no window. What the render loop
//! would do each frame is done here by hand: [`frame`] runs the hooks and the
//! provider ticks, the way `view::main_loop` does.

#![cfg(target_os = "linux")]

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use desicompass_superkey::apps::Catalogue;
use desicompass_superkey::gui::{SuperkeyHooks, apply_startup, build};
use desicompass_superkey::ipc::Ipc;
use desicompass_superkey::power;
use desicompass_superkey_protocol::{
    FromSuperkey, LineDecoder, Section, ToSuperkey, WindowInfo, encode,
};
use sdl3::keyboard::{Keycode, Mod};
use sicompass_ui::accessibility::{AccessibilitySettings, POLL_INTERVAL, SharedAccessibility};
use sicompass_ui::app_state::{AppRenderer, Coordinate, PaletteTheme};
use sicompass_ui::registry::{HostHooks, register_provider};

struct Harness {
    r: AppRenderer,
    hooks: SuperkeyHooks,
    compositor: UnixStream,
    decoder: LineDecoder,
    dir: tempfile::TempDir,
}

fn write(dir: &Path, rel: &str, text: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn shared_in(dir: &Path) -> SharedAccessibility {
    SharedAccessibility::new(
        Some(dir.join("config/sicompass/accessibility.json")),
        vec![dir.join("etc/accessibility.json")],
        AccessibilitySettings::builtin(),
    )
}

fn harness() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "apps/foot.desktop",
        "[Desktop Entry]\nType=Application\nName=Foot\nExec=foot %U\n",
    );
    write(
        dir.path(),
        "apps/firefox.desktop",
        "[Desktop Entry]\nType=Application\nName=Firefox\nExec=firefox --new-window %u\n",
    );
    let catalogue = Catalogue::new(vec![dir.path().join("apps")], None, vec![]);
    let (compositor, superkey_end) = UnixStream::pair().unwrap();
    compositor
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let ipc = Ipc::from_stream(superkey_end).unwrap();
    let (provider, hooks, settings) = build(
        shared_in(dir.path()),
        catalogue,
        Some(ipc),
        power::Commands::default(),
        false,
    );
    let mut r = AppRenderer::new();
    register_provider(&mut r, Box::new(provider));
    apply_startup(&mut r, &settings, false);
    let mut h = Harness {
        r,
        hooks,
        compositor,
        decoder: LineDecoder::new(),
        dir,
    };
    assert_eq!(h.next_message(), FromSuperkey::Hello { version: 1 });
    h
}

impl Harness {
    /// One pass of the render loop's non-drawing half.
    fn frame(&mut self) {
        self.hooks.apply_pending_settings(&mut self.r, false);
        if std::mem::take(&mut self.r.dismiss_requested) {
            self.hooks.dismiss(&mut self.r);
        }
        if self.r.providers[0].tick() {
            sicompass_ui::provider::refresh_current_directory(&mut self.r);
            sicompass_ui::list::create_list_current_layer(&mut self.r);
        }
    }

    /// Frames until `done` holds, the way the loop would keep running.
    fn frames_until(&mut self, mut done: impl FnMut(&AppRenderer) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done(&self.r) {
            assert!(Instant::now() < deadline, "timed out");
            self.frame();
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn send(&mut self, msg: &ToSuperkey) {
        self.compositor.write_all(&encode(msg)).unwrap();
    }

    fn show(&mut self, section: Section, windows: Vec<WindowInfo>) {
        self.r.suspended = true;
        self.send(&ToSuperkey::Show { section, windows });
        self.frames_until(|r| !r.suspended);
    }

    fn next_message(&mut self) -> FromSuperkey {
        let mut buf = [0u8; 1024];
        loop {
            let n = self
                .compositor
                .read(&mut buf)
                .expect("a message from the superkey");
            let mut msgs = self.decoder.feed::<FromSuperkey>(&buf[..n]);
            if !msgs.is_empty() {
                assert_eq!(msgs.len(), 1, "one message at a time: {msgs:?}");
                return msgs.remove(0);
            }
        }
    }

    fn key(&mut self, k: Keycode) {
        sicompass_ui::events::dispatch_key(&mut self.r, Some(k), Mod::empty());
        self.frame();
    }

    fn type_text(&mut self, text: &str) {
        sicompass_ui::handlers::handle_input(&mut self.r, text);
    }

    fn rows(&self) -> Vec<String> {
        let r = &self.r;
        if r.search_string.is_empty() {
            r.total_list.iter().map(|i| i.label.clone()).collect()
        } else {
            r.filtered_list_indices
                .iter()
                .map(|&i| r.total_list[i].label.clone())
                .collect()
        }
    }
}

fn windows() -> Vec<WindowInfo> {
    vec![
        WindowInfo::new(7, "vim", "foot", true),
        WindowInfo::new(3, "Inbox", "sicompass", false),
    ]
}

#[test]
fn it_starts_hidden_and_a_show_opens_the_root_in_search() {
    let mut h = harness();
    assert!(h.r.suspended, "off screen until the compositor says show");
    h.show(Section::Root, windows());
    assert_eq!(h.r.coordinate, Coordinate::SimpleSearch);
    assert_eq!(
        h.rows(),
        [
            "+ Windows",
            "+ Controls",
            "+ Settings",
            "-b Firefox",
            "-b Foot"
        ]
    );
}

#[test]
fn super_w_then_enter_goes_back_to_the_previous_window() {
    let mut h = harness();
    h.show(Section::Windows, windows());
    assert_eq!(h.rows(), ["-b vim (foot)", "-b Inbox (sicompass)"]);
    h.key(Keycode::Return);
    assert_eq!(h.next_message(), FromSuperkey::Focus { id: 3 });
    assert!(h.r.suspended, "it stops drawing as it goes");
}

#[test]
fn a_program_is_found_by_typing_and_started_by_the_compositor() {
    let mut h = harness();
    h.show(Section::Root, windows());
    h.type_text("fire");
    assert_eq!(h.rows(), ["-b Firefox"]);
    h.key(Keycode::Return);
    assert_eq!(
        h.next_message(),
        FromSuperkey::Spawn {
            argv: vec!["firefox".into(), "--new-window".into()],
            cwd: None,
        }
    );
}

#[test]
fn escape_asks_the_compositor_to_hide_it() {
    let mut h = harness();
    h.show(Section::Controls, windows());
    h.key(Keycode::Escape);
    assert_eq!(h.next_message(), FromSuperkey::Hide);
    assert!(h.r.suspended);
}

#[test]
fn log_out_ends_the_session() {
    let mut h = harness();
    h.show(Section::Controls, windows());
    h.type_text("log out");
    h.key(Keycode::Return);
    assert_eq!(h.next_message(), FromSuperkey::QuitSession);
}

#[test]
fn a_setting_changed_here_is_written_to_the_shared_object() {
    let mut h = harness();
    h.show(Section::Settings, windows());
    h.type_text("screen");
    h.key(Keycode::Return);
    let saved = shared_in(h.dir.path());
    assert_eq!(saved.saved().screen_reader, Some(true));
    // The checkbox is ticked now, as on the app's settings page.
    assert!(
        h.rows().iter().any(|r| r == "-cc screen reader"),
        "{:?}",
        h.rows()
    );
}

#[test]
fn a_choice_opens_into_its_values_and_applies_at_once() {
    let mut h = harness();
    h.show(Section::Settings, windows());
    h.type_text("color");
    h.key(Keycode::Return);
    assert_eq!(h.rows(), ["-rc dark", "-r light"]);
    h.type_text("light");
    h.key(Keycode::Return);
    assert_eq!(h.r.palette_theme, PaletteTheme::Light);
    assert_eq!(
        shared_in(h.dir.path()).saved().color_scheme.as_deref(),
        Some("light")
    );
    assert_eq!(h.rows(), ["-r dark", "-rc light"]);
}

#[test]
fn a_change_made_by_another_process_is_followed() {
    let mut h = harness();
    h.show(Section::Root, windows());
    // Past the poll's throttle, then sicompass changes the scheme.
    std::thread::sleep(POLL_INTERVAL);
    shared_in(h.dir.path()).set("colorScheme", "light").unwrap();
    h.frames_until(|r| r.palette_theme == PaletteTheme::Light);
}

#[test]
fn a_window_list_update_while_open_is_shown() {
    let mut h = harness();
    h.show(Section::Windows, windows());
    let mut more = windows();
    more.push(WindowInfo::new(9, "", "firefox", false));
    h.send(&ToSuperkey::Windows { windows: more });
    h.frames_until(|r| r.total_list.len() == 3);
    assert_eq!(h.rows()[2], "-b firefox");
}

#[test]
fn hidden_stops_the_drawing() {
    let mut h = harness();
    h.show(Section::Root, windows());
    h.send(&ToSuperkey::Hidden);
    h.frames_until(|r| r.suspended);
}

#[test]
fn the_compositor_going_away_ends_the_superkey() {
    let h = harness();
    let Harness {
        hooks, compositor, ..
    } = h;
    drop(compositor);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !hooks.should_quit() {
        assert!(Instant::now() < deadline, "never noticed");
        std::thread::sleep(Duration::from_millis(5));
    }
}
