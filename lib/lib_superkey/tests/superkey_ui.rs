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
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use desicompass_bar_protocol::Edge;
use desicompass_bar_protocol::settings::BarSettingsFile;
use desicompass_bar_protocol::status::{self, Notification, StatusFile, StatusSnapshot, TrayItem};
use desicompass_superkey::apps::Catalogue;
use desicompass_superkey::gui::{BarLink, SuperkeyHooks, apply_startup, build};
use desicompass_superkey::ipc::Ipc;
use desicompass_superkey::power;
use desicompass_superkey::status::RecordedActions;
use desicompass_superkey_protocol::{
    FromSuperkey, LineDecoder, Section, ToSuperkey, WindowInfo, encode,
};
use sdl3::keyboard::{Keycode, Mod};
use sicompass_store::StoreProvider;
use sicompass_ui::accessibility::{AccessibilitySettings, POLL_INTERVAL, SharedAccessibility};
use sicompass_ui::app_state::{AppRenderer, Coordinate, PaletteTheme};
use sicompass_ui::registry::{HostHooks, register_provider};

struct Harness {
    r: AppRenderer,
    hooks: SuperkeyHooks,
    compositor: UnixStream,
    decoder: LineDecoder,
    /// What the Status section asked of the bar over D-Bus.
    calls: Arc<RecordedActions>,
    dir: tempfile::TempDir,
}

fn bar_settings_path(dir: &Path) -> PathBuf {
    dir.join("config/desicompass/bar.json")
}

fn status_path(dir: &Path) -> PathBuf {
    dir.join("run/desicompass/status-wayland-1.json")
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

/// A Store kept in `dir`, offline: never the developer's plugins folder, their
/// settings.json or their trash, and never the network.
fn store_in(dir: &Path) -> StoreProvider {
    sicompass_store::_set_test_no_trash(true);
    StoreProvider::new()
        .with_sources(
            Arc::new(|_: &str| Err("offline".to_owned())),
            "http://store.invalid/",
            "http://releases.invalid",
            &[],
            dir.join("plugins"),
        )
        .with_settings_path(dir.join("config/sicompass/settings.json"))
        .with_data_dir(dir.join("data"))
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
    let calls = Arc::new(RecordedActions::default());
    let bar = BarLink {
        settings: BarSettingsFile::open(Some(bar_settings_path(dir.path()))),
        status: StatusFile::open(Some(status_path(dir.path()))),
        actions: Box::new(Arc::clone(&calls)),
    };
    let (provider, hooks, settings) = build(
        shared_in(dir.path()),
        catalogue,
        Some(ipc),
        power::Commands::default(),
        bar,
        store_in(dir.path()),
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
        calls,
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
            "+ Notifications (0) [n]",
            "+ Windows [w]",
            "+ Status [b]",
            "+ Tutorial [t]",
            "+ Store [s]",
            "+ Settings [s]",
            "+ Controls [c]",
            "-b Firefox",
            "-b Foot"
        ]
    );
}

/// The tutorial teaches the app's keys, so inside it the superkey is the app:
/// General mode, with the whole keymap.
fn assert_tutorial_in_general(h: &Harness) {
    assert_eq!(h.r.coordinate, Coordinate::General);
    assert!(!h.r.launcher_mode, "the app's keymap, not the launcher's");
    let sections = h.rows();
    assert_eq!(sections.len(), 7, "{sections:?}");
    assert!(sections[0].starts_with("+ Getting Started"), "{sections:?}");
}

#[test]
fn super_t_opens_the_tutorial_in_general_mode() {
    let mut h = harness();
    h.show(Section::Tutorial, windows());
    h.frame();
    assert_tutorial_in_general(&h);

    let sections = h.rows();
    h.key(Keycode::Right);
    assert_eq!(h.r.coordinate, Coordinate::General);
    let inside = h.rows();
    assert!(!inside.is_empty());
    assert_ne!(inside, sections, "Right went into Getting Started");
}

#[test]
fn enter_or_right_on_the_tutorial_row_lands_in_general_mode() {
    for k in [Keycode::Return, Keycode::Right] {
        let mut h = harness();
        h.show(Section::Root, windows());
        h.type_text("Tutorial");
        h.key(k);
        assert_tutorial_in_general(&h);
    }
}

#[test]
fn left_out_of_the_tutorial_is_the_launcher_again() {
    let mut h = harness();
    h.show(Section::Tutorial, windows());
    h.frame();
    h.key(Keycode::Left);
    assert_eq!(h.r.coordinate, Coordinate::SimpleSearch);
    assert!(h.r.launcher_mode);
    assert_eq!(
        h.r.current_list_item().map(|i| i.label.clone()).as_deref(),
        Some("+ Tutorial [t]")
    );
    // And Escape there still hides it.
    h.key(Keycode::Escape);
    assert_eq!(h.next_message(), FromSuperkey::Hide);
}

#[test]
fn a_show_after_leaving_from_inside_the_tutorial_is_the_launcher() {
    let mut h = harness();
    h.show(Section::Tutorial, windows());
    h.frame();
    h.show(Section::Windows, windows());
    h.frame();
    assert_eq!(h.r.coordinate, Coordinate::SimpleSearch);
    assert!(h.r.launcher_mode);
    assert_eq!(h.rows(), ["-b vim (foot)", "-b Inbox (sicompass)"]);
}

#[test]
fn the_tutorials_inputs_are_edited_in_insert_mode() {
    let mut h = harness();
    h.show(Section::Tutorial, windows());
    h.frame();
    h.key(Keycode::Right); // Getting Started
    let row = h
        .rows()
        .iter()
        .position(|l| l.contains("Edit me"))
        .expect("Getting Started has an input");
    for _ in 0..row {
        h.key(Keycode::Down);
    }
    h.key(Keycode::I);
    assert_eq!(h.r.coordinate, Coordinate::Insert);
    h.key(Keycode::Escape);
    assert_eq!(h.r.coordinate, Coordinate::General);
}

/// The Store's tier pages have inputs, edited in Insert mode, so inside it the
/// superkey is the app too.
fn assert_store_in_general(h: &Harness) {
    assert_eq!(h.r.coordinate, Coordinate::General);
    assert!(!h.r.launcher_mode, "the app's keymap, not the launcher's");
    assert_eq!(h.rows(), ["+ programs", "+ tiers"]);
}

#[test]
fn super_s_lands_on_the_store_in_general_mode() {
    let mut h = harness();
    h.show(Section::Store, windows());
    h.frame();
    assert_store_in_general(&h);
}

#[test]
fn super_s_twice_quickly_lands_on_settings() {
    let mut h = harness();
    h.show(Section::Store, windows());
    h.frame();
    // The compositor asks for the Store again; the superkey pairs the two.
    h.send(&ToSuperkey::Show {
        section: Section::Store,
        windows: windows(),
    });
    h.frames_until(|r| r.launcher_mode);
    h.frame();
    assert_eq!(h.r.coordinate, Coordinate::SimpleSearch);
    let rows = h.rows();
    assert!(
        rows.iter().any(|r| r.contains("Accessibility")),
        "not the settings: {rows:?}"
    );

    // Once Super+S is slower than a double tap, it is the Store again.
    std::thread::sleep(Duration::from_millis(sicompass_ui::handlers::DELTA_MS + 50));
    h.show(Section::Store, windows());
    h.frame();
    assert_store_in_general(&h);
}

#[test]
fn left_out_of_the_store_is_the_launcher_again() {
    let mut h = harness();
    h.show(Section::Store, windows());
    h.frame();
    h.key(Keycode::Left);
    assert_eq!(h.r.coordinate, Coordinate::SimpleSearch);
    assert!(h.r.launcher_mode);
    assert_eq!(
        h.r.current_list_item().map(|i| i.label.clone()).as_deref(),
        Some("+ Store [s]")
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
    h.type_text("accessibility");
    h.key(Keycode::Return);
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
    // The colour scheme is in Settings itself, not in Accessibility.
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

#[test]
fn settings_are_the_scheme_and_language_then_accessibility_then_the_bar() {
    let mut h = harness();
    h.show(Section::Settings, windows());
    assert_eq!(
        h.rows(),
        [
            "+R color scheme [dark]",
            "+R language [English]",
            "+ Accessibility",
            "+ Bar"
        ]
    );
}

#[test]
fn the_bar_settings_are_in_settings_and_a_choice_is_saved_for_the_bar() {
    let mut h = harness();
    h.show(Section::Settings, windows());
    h.type_text("bar");
    h.key(Keycode::Return);
    assert_eq!(
        h.rows(),
        [
            "+R bar position [bottom]",
            "-c show seconds",
            "-c show key strokes"
        ]
    );
    h.key(Keycode::Return);
    assert_eq!(h.rows(), ["-rc bottom", "-r top"]);
    h.type_text("top");
    h.key(Keycode::Return);
    let saved = BarSettingsFile::open(Some(bar_settings_path(h.dir.path()))).get();
    assert_eq!(saved.position, Edge::Top);
    assert_eq!(h.rows(), ["-r bottom", "-rc top"]);
}

#[test]
fn seconds_are_switched_on_for_the_bar() {
    let mut h = harness();
    h.show(Section::Settings, windows());
    h.type_text("bar");
    h.key(Keycode::Return);
    h.type_text("seconds");
    h.key(Keycode::Return);
    let saved = BarSettingsFile::open(Some(bar_settings_path(h.dir.path()))).get();
    assert!(saved.seconds);
    assert!(
        h.rows().iter().any(|r| r == "-cc show seconds"),
        "{:?}",
        h.rows()
    );
}

#[test]
fn key_strokes_are_switched_on_for_the_bar() {
    let mut h = harness();
    h.show(Section::Settings, windows());
    h.type_text("bar");
    h.key(Keycode::Return);
    h.type_text("key strokes");
    h.key(Keycode::Return);
    let saved = BarSettingsFile::open(Some(bar_settings_path(h.dir.path()))).get();
    assert!(saved.keystrokes);
    assert!(!saved.seconds);
    assert!(
        h.rows().iter().any(|r| r == "-cc show key strokes"),
        "{:?}",
        h.rows()
    );
}

fn some_status() -> StatusSnapshot {
    StatusSnapshot {
        notifications: vec![
            Notification {
                id: 4,
                app: "Mail".into(),
                summary: "New message".into(),
                body: String::new(),
            },
            Notification {
                id: 5,
                app: "Chat".into(),
                summary: "Hi".into(),
                body: String::new(),
            },
        ],
        tray: vec![TrayItem {
            service: ":1.42".into(),
            path: "/StatusNotifierItem".into(),
            title: "Dropbox".into(),
        }],
        ..StatusSnapshot::default()
    }
}

/// The harness with `some_status` written where the bar writes it.
fn harness_with_status() -> Harness {
    let h = harness();
    // Past the poll's throttle, so the first frame reads it.
    std::thread::sleep(status::POLL_INTERVAL);
    status::write(&status_path(h.dir.path()), &some_status()).unwrap();
    h
}

#[test]
fn super_b_opens_the_status_the_bar_writes() {
    let mut h = harness_with_status();
    h.show(Section::Status, windows());
    h.frames_until(|r| r.total_list.iter().any(|i| i.label == "+ Tray"));
    let rows = h.rows();
    assert_eq!(rows.len(), 2, "the clock, then the tray: {rows:?}");
    assert!(rows[0].contains(", "), "the date and time first: {rows:?}");
    assert_eq!(rows[1], "+ Tray");
}

#[test]
fn the_notification_count_is_in_the_root() {
    let mut h = harness_with_status();
    h.show(Section::Root, windows());
    h.frames_until(|r| {
        r.total_list
            .first()
            .is_some_and(|i| i.label == "+ Notifications (2) [n]")
    });
}

#[test]
fn super_n_opens_the_notifications_and_enter_dismisses_one() {
    let mut h = harness_with_status();
    h.show(Section::Notifications, windows());
    h.frames_until(|r| r.total_list.len() == 3);
    assert_eq!(
        h.rows(),
        ["-b Dismiss all", "-b Mail: New message", "-b Chat: Hi"]
    );
    h.type_text("chat");
    h.key(Keycode::Return);
    assert_eq!(*h.calls.calls.lock().unwrap(), ["close 5"]);
    h.frames_until(|r| r.total_list.len() == 1);
    assert_eq!(h.rows(), ["-b Mail: New message"]);
}

#[test]
fn enter_on_a_tray_item_activates_it_and_gets_out_of_the_way() {
    let mut h = harness_with_status();
    h.show(Section::Status, windows());
    h.frames_until(|r| r.total_list.iter().any(|i| i.label == "+ Tray"));
    h.type_text("tray");
    h.key(Keycode::Return);
    assert_eq!(h.rows(), ["-b Dropbox"]);
    h.key(Keycode::Return);
    assert_eq!(
        *h.calls.calls.lock().unwrap(),
        ["activate :1.42/StatusNotifierItem"]
    );
    assert_eq!(h.next_message(), FromSuperkey::Hide);
}

impl Harness {
    /// Put the cursor on the first row containing `text`, at this level.
    fn go_to(&mut self, text: &str) {
        let row = self
            .rows()
            .iter()
            .position(|l| l.contains(text))
            .unwrap_or_else(|| panic!("no row with {text:?}: {:?}", self.rows()));
        // Not Home: twice in a row, from anywhere, is the way to the root.
        while self.r.list_index > row {
            self.key(Keycode::Up);
        }
        while self.r.list_index < row {
            self.key(Keycode::Down);
        }
    }

    fn row_with(&self, text: &str) -> String {
        self.rows()
            .into_iter()
            .find(|l| l.contains(text))
            .unwrap_or_else(|| panic!("no row with {text:?}: {:?}", self.rows()))
    }

    /// The bar writes its status, as it does all the time in a session: the
    /// superkey has something new to show.
    fn status_changes(&mut self) {
        std::thread::sleep(status::POLL_INTERVAL);
        status::write(&status_path(self.dir.path()), &some_status()).unwrap();
        for _ in 0..3 {
            self.frame();
            std::thread::sleep(status::POLL_INTERVAL);
        }
    }

    /// Into Getting Started, in General mode.
    fn getting_started(&mut self) {
        self.show(Section::Tutorial, windows());
        self.frame();
        self.go_to("Getting Started");
        self.key(Keycode::Right);
    }

    /// Out of the tutorial, hidden, and back in with Super+T.
    fn hide_and_back(&mut self) {
        self.key(Keycode::Left);
        self.key(Keycode::Left);
        self.key(Keycode::Escape);
        assert_eq!(self.next_message(), FromSuperkey::Hide);
        self.getting_started();
    }
}

/// What a session does to a tutorial edit: the bar's status changes under it,
/// the user leaves the section and the superkey, and comes back.
fn assert_tutorial_edit_kept(h: &mut Harness, text: &str, edited: &str) {
    assert_eq!(h.row_with(text), edited, "applied");
    h.status_changes();
    assert_eq!(h.row_with(text), edited, "kept through a status change");
    h.hide_and_back();
    assert_eq!(h.row_with(text), edited, "kept through hiding the superkey");
}

#[test]
fn a_tutorial_checkbox_stays_ticked() {
    let mut h = harness();
    h.getting_started();
    h.go_to("Practice checkbox");
    h.key(Keycode::Return);
    assert_tutorial_edit_kept(
        &mut h,
        "Practice checkbox",
        "-cc Practice checkbox, press Enter to toggle me",
    );
}

#[test]
fn a_tutorial_input_keeps_what_was_typed() {
    let mut h = harness();
    h.getting_started();
    h.go_to("Edit me");
    h.key(Keycode::A);
    h.type_text(" again");
    h.key(Keycode::Return);
    assert_eq!(h.r.coordinate, Coordinate::General);
    assert_tutorial_edit_kept(
        &mut h,
        "Edit me",
        "-i Edit me, press i or a then Enter: hello world again",
    );
}

#[test]
fn a_tutorial_radio_keeps_its_choice() {
    let mut h = harness();
    h.show(Section::Tutorial, windows());
    h.frame();
    h.go_to("playground");
    h.key(Keycode::Right);
    h.go_to("Pick a color");
    h.key(Keycode::Right);
    h.go_to("green");
    h.key(Keycode::Return);
    assert_eq!(h.rows(), ["-r blue", "-rc green", "-r red"]);
    h.status_changes();
    assert_eq!(h.rows(), ["-r blue", "-rc green", "-r red"]);
}

#[test]
fn home_twice_in_the_tutorial_is_the_superkeys_root() {
    let mut h = harness();
    h.getting_started();
    h.key(Keycode::Home);
    h.key(Keycode::Home);
    assert_eq!(h.r.coordinate, Coordinate::SimpleSearch);
    assert!(h.r.launcher_mode);
    assert_eq!(
        h.r.current_list_item().map(|i| i.label.clone()).as_deref(),
        Some("+ Tutorial [t]")
    );
}

#[test]
fn super_t_after_hiding_inside_the_tutorial_is_general_mode_again() {
    let mut h = harness();
    h.getting_started();
    h.key(Keycode::Tab);
    assert_eq!(
        h.r.coordinate,
        Coordinate::SimpleSearch,
        "the app's own search"
    );
    h.show(Section::Tutorial, windows());
    h.frame();
    assert_tutorial_in_general(&h);
}

#[test]
fn escape_in_the_tutorial_closes_the_superkey() {
    let mut h = harness();
    h.getting_started();
    assert_eq!(h.r.coordinate, Coordinate::General);
    h.key(Keycode::Escape);
    assert_eq!(h.next_message(), FromSuperkey::Hide);
}

#[test]
fn the_tutorial_has_no_tabs_undo_or_timeline_in_the_superkey() {
    let mut h = harness();
    h.getting_started();
    h.go_to("Practice checkbox");
    h.key(Keycode::Return);
    let ticked = h.row_with("Practice checkbox");
    let ctrl = |h: &mut Harness, k: Keycode, m: Mod| {
        sicompass_ui::events::dispatch_key(&mut h.r, Some(k), m);
        h.frame();
    };
    ctrl(&mut h, Keycode::Z, Mod::LCTRLMOD);
    ctrl(&mut h, Keycode::Z, Mod::LCTRLMOD | Mod::LSHIFTMOD);
    assert_eq!(h.row_with("Practice checkbox"), ticked, "no undo or redo");
    for (k, m) in [
        (Keycode::T, Mod::LCTRLMOD),
        (Keycode::T, Mod::LCTRLMOD | Mod::LSHIFTMOD),
        (Keycode::Tab, Mod::LCTRLMOD),
        (Keycode::_1, Mod::LCTRLMOD),
        (Keycode::_9, Mod::LCTRLMOD),
    ] {
        ctrl(&mut h, k, m);
    }
    h.key(Keycode::T);
    h.key(Keycode::Z);
    assert_eq!(h.r.tabs.len(), 1);
    assert_eq!(h.r.coordinate, Coordinate::General);
    assert_eq!(h.row_with("Practice checkbox"), ticked);
}
