//! The superkey window: sicompass-ui with one provider, in superkey mode.
//!
//! The hooks here are the superkey's half of every conversation:
//!
//! * with the compositor, over [`Ipc`]: `show` opens a section and starts
//!   drawing, `hidden` stops it, and the actions pressed in the list go back
//!   as `focus`, `spawn`, `hide` and `quit-session`;
//! * with the rest of the session, over the shared accessibility object
//!   ([`SharedAccessibility`]): a setting changed here is written there, and
//!   one changed by sicompass is followed here;
//! * with the bar, over two files and the session bus: its settings
//!   ([`BarSettingsFile`]) are written here, its status ([`StatusFile`]) is
//!   read here for the Status section, and dismissing a notification or
//!   activating a tray item is a D-Bus call ([`StatusActions`]).
//!
//! The superkey never starts or stops a screen reader. In the session, that is
//! sicompass's job alone; a second owner would start a second Orca.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use desicompass_bar_protocol::clock::{LocalTime, long_text};
use desicompass_bar_protocol::settings::{BarSettings, BarSettingsFile};
use desicompass_bar_protocol::status::StatusFile;
use desicompass_superkey_protocol::{FromSuperkey, Section, ToSuperkey, WindowInfo};
use sicompass_sdk::ffon::IdArray;
use sicompass_store::StoreProvider;
use sicompass_ui::accessibility::{
    self, AccessibilitySettings, KEY_FONT_SCALE, KEY_LANGUAGE, SharedAccessibility,
};
use sicompass_ui::app_state::{AppConfig, AppRenderer, AppState, Coordinate, PaletteTheme};
use sicompass_ui::handlers::{self, open_in_search};
use sicompass_ui::registry::HostHooks;

use crate::actions::Action;
use crate::apps::Catalogue;
use crate::i18n::{t, t_with};
use crate::ipc::Ipc;
use crate::power;
use crate::provider::{
    Node, PROGRAMS, SECTIONS, Shared, SharedState, SuperkeyProvider, TutorialState, setting_label,
    value_label,
};
use crate::status::StatusActions;

/// Where the cursor lands for `section`, as the renderer addresses rows:
/// `[provider, row]` at the root, `[provider, section, row]` inside one.
///
/// In Windows it lands on the second window, the one used before the current:
/// Super+W then Enter goes back to it, the way Alt+Tab does.
pub fn landing(section: Section, windows: &[WindowInfo]) -> IdArray {
    let node = match section {
        Section::Root => {
            let mut id = IdArray::new();
            id.push(0);
            id.push(0);
            return id;
        }
        Section::Notifications => Node::Notifications,
        Section::Windows => Node::Windows,
        Section::Status => Node::Status,
        Section::Tutorial => Node::Tutorial,
        Section::Store => Node::Store,
        Section::Settings => Node::Settings,
        Section::Controls => Node::Controls,
    };
    let row = usize::from(section == Section::Windows && windows.len() >= 2);
    let mut id = IdArray::new();
    id.push(0);
    id.push(section_index(node));
    id.push(row);
    id
}

/// The program (Tutorial or Store) `id` is a row inside of, at any depth.
fn in_program(id: &IdArray) -> Option<Node> {
    if id.depth() <= 2 {
        return None;
    }
    let at = id.get(1)?;
    PROGRAMS.into_iter().find(|&n| section_index(n) == at)
}

/// Super+S is the Store, and pressed again within the double-tap window it is
/// Settings, which has no key of its own. `now` is in [`handlers::sdl_ticks`]
/// milliseconds, and `last` the Store press a second one would pair with: a
/// pair is used up, so a third press is a first one again, the way Ctrl+A
/// twice is in the app.
pub fn cycled(section: Section, now: u64, last: &mut Option<u64>) -> Section {
    if section != Section::Store {
        return section;
    }
    if last
        .take()
        .is_some_and(|t| now.saturating_sub(t) <= handlers::DELTA_MS)
    {
        Section::Settings
    } else {
        *last = Some(now);
        Section::Store
    }
}

/// Where `node` sits among the root's sections.
fn section_index(node: Node) -> usize {
    SECTIONS
        .iter()
        .position(|&n| n == node)
        .expect("every section the compositor names is in SECTIONS")
}

/// The superkey's answers to the renderer.
pub struct SuperkeyHooks {
    shared: SharedState,
    ipc: Option<Ipc>,
    access: Mutex<SharedAccessibility>,
    catalogue: Mutex<Catalogue>,
    power: power::Commands,
    bar: Mutex<BarSettingsFile>,
    status: Mutex<StatusFile>,
    actions: Box<dyn StatusActions>,
    /// The minute the Status section's clock was last set for.
    clock_minute: Mutex<Option<(u32, u32)>>,
    font_scale: Mutex<f32>,
    /// Standalone (no compositor): Escape quits instead of hiding.
    standalone: bool,
    quit: AtomicBool,
    /// When Super+S last asked for the Store, unpaired ([`cycled`]).
    last_store_show: Mutex<Option<u64>>,
    /// The program the cursor was last inside, where Home twice lands.
    last_program: Mutex<Node>,
}

impl HostHooks for SuperkeyHooks {
    /// Called every frame, hidden or not: the superkey always has a settings
    /// queue, which is what makes the renderer call it.
    fn apply_pending_settings(&self, r: &mut AppRenderer, _initial: bool) {
        self.frame(r);
    }

    fn read_font_scale(&self) -> f32 {
        *self.font_scale.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn should_quit(&self) -> bool {
        self.quit.load(Ordering::Relaxed) || self.ipc.as_ref().is_some_and(Ipc::is_closed)
    }

    fn dismiss(&self, r: &mut AppRenderer) {
        if self.standalone {
            self.quit.store(true, Ordering::Relaxed);
            return;
        }
        self.send(&FromSuperkey::Hide);
        r.suspended = true;
    }
}

/// The superkey's view of the bar: its settings file, its status file, and
/// the calls the Status section makes.
pub struct BarLink {
    pub settings: BarSettingsFile,
    pub status: StatusFile,
    pub actions: Box<dyn StatusActions>,
}

impl BarLink {
    /// The session's: the user's settings file, this session's status file,
    /// and the session bus.
    pub fn session() -> Self {
        Self {
            settings: BarSettingsFile::open_default(),
            status: StatusFile::open(desicompass_bar_protocol::status::default_path()),
            actions: Box::new(crate::status::DbusActions::new()),
        }
    }
}

impl SuperkeyHooks {
    pub fn new(
        shared: SharedState,
        ipc: Option<Ipc>,
        access: SharedAccessibility,
        catalogue: Catalogue,
        power: power::Commands,
        bar: BarLink,
        standalone: bool,
    ) -> Self {
        let font_scale = accessibility::font_scale_value(access.effective().font_scale.as_deref());
        Self {
            shared,
            ipc,
            access: Mutex::new(access),
            catalogue: Mutex::new(catalogue),
            power,
            bar: Mutex::new(bar.settings),
            status: Mutex::new(bar.status),
            actions: bar.actions,
            clock_minute: Mutex::new(None),
            font_scale: Mutex::new(font_scale),
            standalone,
            quit: AtomicBool::new(false),
            last_store_show: Mutex::new(None),
            last_program: Mutex::new(Node::Tutorial),
        }
    }

    fn shared(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn send(&self, msg: &FromSuperkey) {
        match &self.ipc {
            Some(ipc) => ipc.send(msg),
            None => tracing::info!("standalone, not sent: {msg:?}"),
        }
    }

    /// One frame's worth of work, in order: what the compositor said, what
    /// the user pressed, what another process changed.
    pub fn frame(&self, r: &mut AppRenderer) {
        if let Some(ipc) = &self.ipc {
            for msg in ipc.drain() {
                self.on_message(r, msg);
            }
        }
        let actions = std::mem::take(&mut self.shared().actions);
        for action in actions {
            self.on_action(r, action);
        }
        let changed = self.access.lock().unwrap_or_else(|e| e.into_inner()).poll();
        if !changed.is_empty() {
            for (key, value) in &changed {
                self.apply(r, key, value);
            }
            self.refresh_settings();
        }
        if self.bar.lock().unwrap_or_else(|e| e.into_inner()).poll() {
            self.refresh_bar();
        }
        let status = {
            let mut f = self.status.lock().unwrap_or_else(|e| e.into_inner());
            f.poll().then(|| f.get().clone())
        };
        if let Some(status) = status {
            let mut s = self.shared();
            s.status = status;
            s.dirty = true;
        }
        if let Some(e) = self.actions.take_error() {
            self.shared().error = Some(t_with("superkey-status-failed", &[("error", &e)]));
        }
        self.tick_clock();
        self.follow_program_mode(r);
        self.remember_tutorial(r);
    }

    /// Inside a program (the tutorial, the Store) the superkey is the app:
    /// General mode and the app's whole keymap, because those are the keys the
    /// tutorial teaches, and the Store's tier pages have inputs to edit in
    /// Insert mode. Everywhere else it is a launcher, in simple search. Run
    /// every frame, after the keys, so however a program was entered (Super+T,
    /// Super+S, Enter or Right on its row) or left (Left at its top), the next
    /// frame is drawn and announced in the right mode.
    fn follow_program_mode(&self, r: &mut AppRenderer) {
        let program = in_program(&r.current_id);
        if let Some(p) = program {
            *self.last_program.lock().unwrap_or_else(|e| e.into_inner()) = p;
        }
        if program.is_some() && r.launcher_mode {
            r.launcher_mode = false;
            if r.coordinate == Coordinate::SimpleSearch {
                // Escape out of search, the app's own way into General: it
                // rebuilds the list on the row search started from and says so.
                r.previous_coordinate = Coordinate::General;
                handlers::handle_escape(r);
            }
        } else if program.is_none() && !r.launcher_mode {
            r.launcher_mode = true;
            // Home twice goes to the app's root, the list of providers, which in
            // a launcher is above anything its user can do: land on the row of
            // the program it was in instead.
            if r.current_id.depth() < 2 {
                let last = *self.last_program.lock().unwrap_or_else(|e| e.into_inner());
                let mut at = IdArray::new();
                at.push(0);
                at.push(section_index(last));
                r.current_id = at;
                sicompass_ui::provider::set_provider_path(r, "/");
                sicompass_ui::provider::refresh_current_directory(r);
                r.coordinate = Coordinate::General;
            }
            if r.coordinate.is_general() {
                // Tab, as `open_in_search` does it: search, announced.
                handlers::handle_tab(r);
            }
        }
    }

    /// Copy the tutorial, as the user has it, from the renderer into
    /// [`Shared::tutorial`]. Every frame inside it, after the keys and before
    /// the providers tick, so no edit is ever missing from a rebuild.
    fn remember_tutorial(&self, r: &AppRenderer) {
        if in_program(&r.current_id) != Some(Node::Tutorial) {
            return;
        }
        let at = section_index(Node::Tutorial);
        let mut id = IdArray::new();
        id.push(0);
        id.push(at);
        let rows = sicompass_sdk::ffon::get_ffon_at_id(&r.ffon, &id)
            .and_then(|a| a.get(at))
            .and_then(|e| e.as_obj())
            .map(|o| o.children.clone());
        if let Some(rows) = rows {
            self.shared().tutorial = Some(TutorialState {
                locale: sicompass_sdk::localize::current_locale(),
                rows,
            });
        }
    }

    /// The Status section's clock, moved on the minute: a row that changed
    /// every second would be read out again every second.
    fn tick_clock(&self) {
        let now = LocalTime::now();
        let minute = (now.hour, now.minute);
        let mut last = self.clock_minute.lock().unwrap_or_else(|e| e.into_inner());
        if *last == Some(minute) {
            return;
        }
        *last = Some(minute);
        let language = self
            .shared()
            .settings
            .language
            .clone()
            .unwrap_or_else(|| "en-US".to_owned());
        let mut s = self.shared();
        s.clock = long_text(&now, &language);
        s.dirty = true;
    }

    fn on_message(&self, r: &mut AppRenderer, msg: ToSuperkey) {
        match msg {
            ToSuperkey::Show { section, windows } => {
                let section = cycled(
                    section,
                    handlers::sdl_ticks(),
                    &mut self
                        .last_store_show
                        .lock()
                        .unwrap_or_else(|e| e.into_inner()),
                );
                let id = landing(section, &windows);
                self.shared().windows = windows;
                {
                    let mut cat = self.catalogue.lock().unwrap_or_else(|e| e.into_inner());
                    if cat.refresh() {
                        self.shared().apps = cat.apps().to_vec();
                    }
                }
                r.suspended = false;
                // A launcher again, whatever it was when it was hidden (a
                // program turns that off): `follow_program_mode` turns it off
                // again if this lands inside one.
                r.launcher_mode = true;
                open_in_search(r, &id);
            }
            ToSuperkey::Windows { windows } => {
                let mut s = self.shared();
                s.windows = windows;
                s.dirty = true;
            }
            ToSuperkey::Hidden => r.suspended = true,
        }
    }

    fn on_action(&self, r: &mut AppRenderer, action: Action) {
        match action {
            Action::Focus(id) => {
                self.send(&FromSuperkey::Focus { id });
                r.suspended = !self.standalone;
            }
            Action::Launch(id) => {
                let app = self
                    .catalogue
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .find(&id)
                    .cloned();
                match app {
                    Some(app) => {
                        self.send(&FromSuperkey::Spawn {
                            argv: app.exec,
                            cwd: app.path,
                        });
                        r.suspended = !self.standalone;
                    }
                    None => {
                        self.shared().error =
                            Some(t_with("superkey-launch-failed", &[("name", &id)]));
                    }
                }
            }
            Action::Power(which) => {
                if let Some(key) = power::Commands::announcement_key(which) {
                    self.shared().announcement = Some(t(key));
                }
                // Suspending leaves the session as it was, so the superkey
                // should not be the first thing on screen after waking.
                if which == power::SUSPEND {
                    self.send(&FromSuperkey::Hide);
                    r.suspended = !self.standalone;
                }
                if let Err(e) = self.power.run(which) {
                    let action = power::Commands::label_key(which).map(t).unwrap_or_default();
                    self.shared().error = Some(t_with(
                        "superkey-power-failed",
                        &[("action", &action), ("error", &e.to_string())],
                    ));
                }
            }
            Action::Logout => self.send(&FromSuperkey::QuitSession),
            Action::Toggle(key) => {
                let on = {
                    let s = self.shared();
                    if BarSettings::is_key(key) {
                        s.bar.get(key)
                    } else {
                        s.settings.get(key)
                    }
                }
                .as_deref()
                    == Some("true");
                self.change(r, key, if on { "false" } else { "true" });
            }
            Action::Set(key, value) => self.change(r, key, &value),
            Action::Dismiss(id) => self.dismiss_notifications(&[id]),
            Action::DismissAll => {
                let ids: Vec<u32> = self
                    .shared()
                    .status
                    .notifications
                    .iter()
                    .map(|n| n.id)
                    .collect();
                self.dismiss_notifications(&ids);
            }
            Action::Activate { service, path } => {
                self.actions.activate(&service, &path);
                // What it opens should be seen, and it waits behind the
                // superkey until the superkey closes.
                self.send(&FromSuperkey::Hide);
                r.suspended = !self.standalone;
            }
        }
    }

    /// Ask the bar to dismiss these, and take them off the list at once
    /// rather than when the bar's next status arrives.
    fn dismiss_notifications(&self, ids: &[u32]) {
        for &id in ids {
            self.actions.close_notification(id);
        }
        let mut s = self.shared();
        s.status.notifications.retain(|n| !ids.contains(&n.id));
        s.announcement = Some(t("superkey-notification-dismissed"));
        s.dirty = true;
    }

    /// A setting chosen here: save it to the shared object, apply it here,
    /// and say so.
    fn change(&self, r: &mut AppRenderer, key: &'static str, value: &str) {
        let is_bar = BarSettings::is_key(key);
        let saved = if is_bar {
            self.bar
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .set(key, value)
        } else {
            self.access
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .set(key, value)
        };
        if let Err(e) = saved {
            self.shared().error = Some(t_with(
                "superkey-setting-failed",
                &[("setting", &setting_label(key)), ("error", &e.to_string())],
            ));
            return;
        }
        if is_bar {
            // The bar follows the file; nothing changes in this window.
            self.refresh_bar();
        } else {
            self.apply(r, key, value);
            self.refresh_settings();
        }
        // A language change is announced by the renderer, in the new voice.
        if key != KEY_LANGUAGE {
            let v = value_label(key, value);
            self.shared().announcement = Some(t_with(
                "superkey-setting-changed",
                &[("setting", &setting_label(key)), ("value", &v)],
            ));
        }
    }

    /// Put a setting into effect in this window. The screen reader is not
    /// one of them: sicompass owns it.
    fn apply(&self, r: &mut AppRenderer, key: &str, value: &str) {
        if key == KEY_FONT_SCALE {
            // Stored first: the rebuild reads it back through read_font_scale.
            *self.font_scale.lock().unwrap_or_else(|e| e.into_inner()) =
                accessibility::font_scale_value(Some(value));
        }
        if accessibility::apply_display(r, key, value) {
            return;
        }
        if key == KEY_LANGUAGE {
            sicompass_sdk::localize::set_locale(value);
            {
                let mut cat = self.catalogue.lock().unwrap_or_else(|e| e.into_inner());
                cat.set_locale(Some(Catalogue::locale_for(value)));
                self.shared().apps = cat.apps().to_vec();
            }
            relocalize(r);
            if !r.suspended {
                r.speak_language_change();
            }
        }
    }

    /// The bar's settings as the list shows them, after a change.
    fn refresh_bar(&self) {
        let current = self.bar.lock().unwrap_or_else(|e| e.into_inner()).get();
        let mut s = self.shared();
        s.bar = current;
        s.dirty = true;
    }

    /// The settings as the list shows them, after a change.
    fn refresh_settings(&self) {
        let effective = self
            .access
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .effective();
        let mut s = self.shared();
        s.settings = effective;
        s.dirty = true;
    }
}

/// Rebuild the whole list in the new language, at the level the cursor is on.
///
/// Reopening is the one way to re-key every level at once: the section and
/// choice labels above the cursor are in the old language too, and the
/// provider recognises a level by its current label.
fn relocalize(r: &mut AppRenderer) {
    sicompass_ui::provider::refresh_all_provider_root_keys(r);
    let id = relocalized_id(&r.current_id);
    if id.depth() >= 2 {
        open_in_search(r, &id);
    }
}

/// Where a language change reopens. Inside a program (the tutorial, the
/// Store), at its row: the embedded provider knows its levels only by their
/// labels, which are in the old language. sicompass collapses its tutorial the
/// same way.
fn relocalized_id(id: &IdArray) -> IdArray {
    if let Some(program) = in_program(id) {
        let mut at = IdArray::new();
        at.push(0);
        at.push(section_index(program));
        return at;
    }
    id.clone()
}

/// What the superkey needs from the command line.
pub struct Options {
    pub ipc: Option<Ipc>,
    pub power: power::Commands,
    pub app_dirs: Vec<std::path::PathBuf>,
    /// No compositor: start on screen, and quit on Escape.
    pub standalone: bool,
}

/// The superkey's provider and hooks, wired to each other, and the settings it
/// starts with. Split from [`run`] so tests can drive the same pair through a
/// headless renderer.
pub fn build(
    access: SharedAccessibility,
    catalogue: Catalogue,
    ipc: Option<Ipc>,
    power: power::Commands,
    bar: BarLink,
    store: StoreProvider,
    standalone: bool,
) -> (SuperkeyProvider, SuperkeyHooks, AccessibilitySettings) {
    crate::i18n::init();
    let settings = access.effective();
    let language = settings
        .language
        .clone()
        .unwrap_or_else(|| "en-US".to_owned());
    sicompass_sdk::localize::set_locale(&language);
    let shared: SharedState = Arc::new(Mutex::new(Shared {
        apps: catalogue.apps().to_vec(),
        settings: settings.clone(),
        bar: bar.settings.get(),
        status: bar.status.get().clone(),
        ..Shared::default()
    }));
    let provider = SuperkeyProvider::with_store(Arc::clone(&shared), store);
    let hooks = SuperkeyHooks::new(shared, ipc, access, catalogue, power, bar, standalone);
    (provider, hooks, settings)
}

/// Build the renderer and run it until the compositor closes the channel (or,
/// standalone, until Escape).
pub fn run(opts: Options) -> Result<(), String> {
    crate::i18n::init();
    let access = SharedAccessibility::open_session();
    let language = access
        .effective()
        .language
        .unwrap_or_else(|| "en-US".to_owned());
    let catalogue = Catalogue::new(
        opts.app_dirs,
        Some(Catalogue::locale_for(&language)),
        Catalogue::current_desktops(),
    );
    let (provider, hooks, settings) = build(
        access,
        catalogue,
        opts.ipc,
        opts.power,
        BarLink::session(),
        StoreProvider::new(),
        opts.standalone,
    );
    let font_scale = hooks.read_font_scale();

    let cfg = AppConfig {
        title: t("superkey-title"),
        app_name: "Desicompass superkey".to_owned(),
        app_id: "desicompass-superkey".to_owned(),
        vulkan_app_name: "desicompass-superkey".to_owned(),
        // Always full screen: a screen of its own over the windows. The
        // compositor sizes it too; this is what it asks for when run alone.
        width: 960,
        height: 640,
        // No pointer to click window controls with.
        custom_titlebar: false,
        maximized: false,
        fullscreen: true,
        window_icon: false,
        font_scale,
    };

    let mut app = AppState::with_providers(&cfg, vec![Box::new(provider)], Box::new(hooks))
        .map_err(|e| format!("could not start the superkey window: {e}"))?;
    apply_startup(&mut app.renderer, &settings, opts.standalone);
    app.run();
    Ok(())
}

/// The renderer state the superkey starts in.
pub fn apply_startup(r: &mut AppRenderer, settings: &AccessibilitySettings, standalone: bool) {
    // Any queue: its presence is what makes the renderer call the hooks every
    // frame. Nothing is ever put in it.
    r.settings_queue = Some(Arc::new(Mutex::new(Vec::new())));
    r.launcher_mode = true;
    // Stays on inside the tutorial, where `launcher_mode` goes off: Escape in
    // General still closes the superkey, and the app's tabs, undo and timeline
    // keys do nothing.
    r.launcher_window = true;
    r.palette_theme = if settings.color_scheme.as_deref() == Some("light") {
        PaletteTheme::Light
    } else {
        PaletteTheme::Dark
    };
    r.privacy_blank = settings.shoulder_surfing_protection == Some(true);
    open_in_search(r, &landing(Section::Root, &[]));
    // Off screen until the compositor says `show`.
    r.suspended = !standalone;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts(id: &IdArray) -> Vec<usize> {
        (0..id.depth()).map(|i| id.get(i).unwrap()).collect()
    }

    #[test]
    fn each_section_lands_where_its_rows_are() {
        let one = [WindowInfo::new(1, "a", "a", true)];
        let two = [one[0].clone(), WindowInfo::new(2, "b", "b", false)];
        assert_eq!(parts(&landing(Section::Root, &two)), [0, 0]);
        assert_eq!(parts(&landing(Section::Notifications, &two)), [0, 0, 0]);
        assert_eq!(parts(&landing(Section::Windows, &two)), [0, 1, 1]);
        assert_eq!(parts(&landing(Section::Windows, &one)), [0, 1, 0]);
        assert_eq!(parts(&landing(Section::Windows, &[])), [0, 1, 0]);
        assert_eq!(parts(&landing(Section::Status, &two)), [0, 2, 0]);
        assert_eq!(parts(&landing(Section::Tutorial, &two)), [0, 3, 0]);
        assert_eq!(parts(&landing(Section::Store, &two)), [0, 4, 0]);
        assert_eq!(parts(&landing(Section::Settings, &two)), [0, 5, 0]);
        assert_eq!(parts(&landing(Section::Controls, &two)), [0, 6, 0]);
    }

    #[test]
    fn super_s_is_the_store_and_twice_quickly_the_settings() {
        let mut last = None;
        let t = 1_000_000;
        assert_eq!(cycled(Section::Store, t, &mut last), Section::Store);
        assert_eq!(
            cycled(Section::Store, t + handlers::DELTA_MS, &mut last),
            Section::Settings,
            "a second press within the window"
        );
        // The pair is used up: a third press is the Store again.
        assert_eq!(
            cycled(Section::Store, t + handlers::DELTA_MS + 10, &mut last),
            Section::Store
        );
        // Too slow for a pair: the Store again, which a quick next one pairs.
        let later = t + 10 * handlers::DELTA_MS;
        assert_eq!(cycled(Section::Store, later, &mut last), Section::Store);
        assert_eq!(
            cycled(Section::Store, later + 1, &mut last),
            Section::Settings
        );
        // Other sections pass through and leave the pairing alone.
        assert_eq!(cycled(Section::Store, t, &mut last), Section::Store);
        assert_eq!(cycled(Section::Windows, t + 1, &mut last), Section::Windows);
        assert_eq!(
            cycled(Section::Settings, t + 2, &mut last),
            Section::Settings
        );
        assert_eq!(cycled(Section::Store, t + 3, &mut last), Section::Settings);
    }

    #[test]
    fn a_language_change_inside_the_store_reopens_at_its_row() {
        let store = section_index(Node::Store);
        let mut deep = IdArray::new();
        for p in [0, store, 1, 0] {
            deep.push(p);
        }
        assert_eq!(parts(&relocalized_id(&deep)), [0, store]);
    }

    #[test]
    fn a_language_change_inside_the_tutorial_reopens_at_its_row() {
        let tutorial = section_index(Node::Tutorial);
        let mut deep = IdArray::new();
        for p in [0, tutorial, 0, 3] {
            deep.push(p);
        }
        assert_eq!(parts(&relocalized_id(&deep)), [0, tutorial]);

        let mut settings = IdArray::new();
        for p in [0, section_index(Node::Settings), 1] {
            settings.push(p);
        }
        assert_eq!(relocalized_id(&settings), settings);
    }
}
