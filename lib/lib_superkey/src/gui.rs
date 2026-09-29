//! The superkey window: sicompass-ui with one provider, in superkey mode.
//!
//! The hooks here are the superkey's half of every conversation:
//!
//! * with the compositor, over [`Ipc`]: `show` opens a section and starts
//!   drawing, `hidden` stops it, and the actions pressed in the list go back
//!   as `focus`, `spawn`, `hide` and `quit-session`;
//! * with the rest of the session, over the shared accessibility object
//!   ([`SharedAccessibility`]): a setting changed here is written there, and
//!   one changed by sicompass is followed here.
//!
//! The superkey never starts or stops a screen reader. In the session, that is
//! sicompass's job alone; a second owner would start a second Orca.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use desicompass_superkey_protocol::{FromSuperkey, Section, ToSuperkey, WindowInfo};
use sicompass_sdk::ffon::IdArray;
use sicompass_ui::accessibility::{
    self, AccessibilitySettings, KEY_FONT_SCALE, KEY_LANGUAGE, SharedAccessibility,
};
use sicompass_ui::app_state::{AppConfig, AppRenderer, AppState, PaletteTheme};
use sicompass_ui::handlers::open_in_search;
use sicompass_ui::registry::HostHooks;

use crate::actions::Action;
use crate::apps::Catalogue;
use crate::i18n::{t, t_with};
use crate::ipc::Ipc;
use crate::power;
use crate::provider::{Shared, SharedState, SuperkeyProvider, setting_label, value_label};

/// Where the cursor lands for `section`, as the renderer addresses rows:
/// `[provider, row]` at the root, `[provider, section, row]` inside one.
///
/// In Windows it lands on the second window, the one used before the current:
/// Super+W then Enter goes back to it, the way Alt+Tab does.
pub fn landing(section: Section, windows: &[WindowInfo]) -> IdArray {
    let parts: &[usize] = match section {
        Section::Root => &[0, 0],
        Section::Windows if windows.len() >= 2 => &[0, 0, 1],
        Section::Windows => &[0, 0, 0],
        Section::Controls => &[0, 1, 0],
        Section::Settings => &[0, 2, 0],
    };
    let mut id = IdArray::new();
    for &p in parts {
        id.push(p);
    }
    id
}

/// The superkey's answers to the renderer.
pub struct SuperkeyHooks {
    shared: SharedState,
    ipc: Option<Ipc>,
    access: Mutex<SharedAccessibility>,
    catalogue: Mutex<Catalogue>,
    power: power::Commands,
    font_scale: Mutex<f32>,
    /// Standalone (no compositor): Escape quits instead of hiding.
    standalone: bool,
    quit: AtomicBool,
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

impl SuperkeyHooks {
    pub fn new(
        shared: SharedState,
        ipc: Option<Ipc>,
        access: SharedAccessibility,
        catalogue: Catalogue,
        power: power::Commands,
        standalone: bool,
    ) -> Self {
        let font_scale = accessibility::font_scale_value(access.effective().font_scale.as_deref());
        Self {
            shared,
            ipc,
            access: Mutex::new(access),
            catalogue: Mutex::new(catalogue),
            power,
            font_scale: Mutex::new(font_scale),
            standalone,
            quit: AtomicBool::new(false),
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
    }

    fn on_message(&self, r: &mut AppRenderer, msg: ToSuperkey) {
        match msg {
            ToSuperkey::Show { section, windows } => {
                let id = landing(section, &windows);
                self.shared().windows = windows;
                {
                    let mut cat = self.catalogue.lock().unwrap_or_else(|e| e.into_inner());
                    if cat.refresh() {
                        self.shared().apps = cat.apps().to_vec();
                    }
                }
                r.suspended = false;
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
                let on = self.shared().settings.get(key).as_deref() == Some("true");
                self.change(r, key, if on { "false" } else { "true" });
            }
            Action::Set(key, value) => self.change(r, key, &value),
        }
    }

    /// A setting chosen here: save it to the shared object, apply it here,
    /// and say so.
    fn change(&self, r: &mut AppRenderer, key: &'static str, value: &str) {
        let saved = self
            .access
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set(key, value);
        if let Err(e) = saved {
            self.shared().error = Some(t_with(
                "superkey-setting-failed",
                &[("setting", &setting_label(key)), ("error", &e.to_string())],
            ));
            return;
        }
        self.apply(r, key, value);
        self.refresh_settings();
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
    let id = r.current_id.clone();
    if id.depth() >= 2 {
        open_in_search(r, &id);
    }
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
        ..Shared::default()
    }));
    let provider = SuperkeyProvider::new(Arc::clone(&shared));
    let hooks = SuperkeyHooks::new(shared, ipc, access, catalogue, power, standalone);
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
    let (provider, hooks, settings) =
        build(access, catalogue, opts.ipc, opts.power, opts.standalone);
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
        assert_eq!(parts(&landing(Section::Windows, &two)), [0, 0, 1]);
        assert_eq!(parts(&landing(Section::Windows, &one)), [0, 0, 0]);
        assert_eq!(parts(&landing(Section::Windows, &[])), [0, 0, 0]);
        assert_eq!(parts(&landing(Section::Controls, &two)), [0, 1, 0]);
        assert_eq!(parts(&landing(Section::Settings, &two)), [0, 2, 0]);
    }
}
