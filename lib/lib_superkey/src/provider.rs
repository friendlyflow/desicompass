//! The superkey's list, as a sicompass provider.
//!
//! ```text
//! + Windows               one button per open window, most recent first
//! + Controls              suspend, restart, shut down, log out
//! + Settings              the accessibility settings
//! -b Firefox              then every installed program, by name
//! -b Files
//! ```
//!
//! Every leaf is a `<button>` whose function name is an [`Action`], and pressing
//! one only queues the action: the host (`gui.rs`) owns the compositor channel
//! and the renderer, and carries it out. What the list shows comes from
//! [`Shared`], which the host fills in (the window list, the programs, the
//! current settings).
//!
//! `fetch` is path-scoped: the whole tree at the root, and one level inside a
//! section. The path is kept as [`Node`]s, not labels, so it survives a
//! language change.

use std::sync::{Arc, Mutex};

use desicompass_superkey_protocol::WindowInfo;
use sicompass_sdk::ffon::FfonElement;
use sicompass_sdk::provider::Provider;
use sicompass_sdk::tags;
use sicompass_ui::accessibility::{
    AccessibilitySettings, COLOR_SCHEMES, FONT_SCALES, KEY_COLOR_SCHEME, KEY_FONT_SCALE,
    KEY_LANGUAGE, KEY_SCREEN_READER, KEY_SHOULDER_SURFING, LANGUAGES,
};

use crate::actions::{Action, function_name, parse};
use crate::apps::App;
use crate::i18n::t;
use crate::power;

/// What the host tells the provider, and what the provider hands back.
#[derive(Debug, Default)]
pub struct Shared {
    pub windows: Vec<WindowInfo>,
    pub apps: Vec<App>,
    /// The effective accessibility settings, every key set.
    pub settings: AccessibilitySettings,
    /// Pressed, not yet carried out.
    pub actions: Vec<Action>,
    pub announcement: Option<String>,
    pub error: Option<String>,
    /// Something shown changed: the next tick rebuilds the list.
    pub dirty: bool,
}

pub type SharedState = Arc<Mutex<Shared>>;

/// A level below the root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Node {
    Windows,
    Controls,
    Settings,
    /// The options of one accessibility setting, inside Settings.
    Choice(&'static str),
}

/// The three sections, in the order the root lists them. The index is what
/// the host lands the cursor on.
pub const SECTIONS: [Node; 3] = [Node::Windows, Node::Controls, Node::Settings];

/// The settings with a list of values, as they appear inside Settings.
const CHOICES: [&str; 3] = [KEY_FONT_SCALE, KEY_COLOR_SCHEME, KEY_LANGUAGE];

pub struct SuperkeyProvider {
    shared: SharedState,
    /// One entry per level below the root. `None` for a segment this provider
    /// did not recognise, which then has nothing in it.
    path: Vec<Option<Node>>,
    path_str: String,
}

impl SuperkeyProvider {
    pub fn new(shared: SharedState) -> Self {
        Self {
            shared,
            path: Vec::new(),
            path_str: "/".to_owned(),
        }
    }

    fn shared(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn update_path_str(&mut self) {
        self.path_str = if self.path.is_empty() {
            "/".to_owned()
        } else {
            let segs: Vec<String> = self
                .path
                .iter()
                .map(|n| match n {
                    Some(Node::Windows) => "windows".to_owned(),
                    Some(Node::Controls) => "controls".to_owned(),
                    Some(Node::Settings) => "settings".to_owned(),
                    Some(Node::Choice(k)) => (*k).to_owned(),
                    None => "?".to_owned(),
                })
                .collect();
            format!("/{}", segs.join("/"))
        };
    }

    // ---- Rows --------------------------------------------------------------

    fn root(&self) -> Vec<FfonElement> {
        let mut out: Vec<FfonElement> = SECTIONS
            .iter()
            .map(|&n| section(&section_label(n), self.rows(n)))
            .collect();
        out.extend(
            self.shared()
                .apps
                .iter()
                .map(|a| button(&Action::Launch(a.id.clone()), &a.name)),
        );
        out
    }

    fn rows(&self, node: Node) -> Vec<FfonElement> {
        match node {
            Node::Windows => self.window_rows(),
            Node::Controls => control_rows(),
            Node::Settings => self.settings_rows(),
            Node::Choice(key) => self.choice_rows(key),
        }
    }

    fn window_rows(&self) -> Vec<FfonElement> {
        let s = self.shared();
        if s.windows.is_empty() {
            // A plain row, not an empty level: the renderer would put an
            // insert placeholder in an empty one.
            return vec![FfonElement::Str(t("superkey-no-windows"))];
        }
        s.windows
            .iter()
            .map(|w| button(&Action::Focus(w.id), &window_label(w)))
            .collect()
    }

    /// The settings as the app's own settings page shows them: a checkbox for
    /// each switch, a radio group for each choice.
    fn settings_rows(&self) -> Vec<FfonElement> {
        let s = self.shared().settings.clone();
        let mut out = vec![checkbox(&s, KEY_SCREEN_READER)];
        for key in CHOICES {
            out.push(section(
                &format!("<radio>{}", setting_label(key)),
                self.choice_rows(key),
            ));
        }
        out.push(checkbox(&s, KEY_SHOULDER_SURFING));
        out
    }

    /// A radio group's options, the current one checked.
    fn choice_rows(&self, key: &'static str) -> Vec<FfonElement> {
        let current = self.shared().settings.get(key).unwrap_or_default();
        options(key)
            .iter()
            .map(|&v| {
                let label = value_label(key, v);
                FfonElement::Str(if v == current {
                    tags::format_checked(&label)
                } else {
                    label
                })
            })
            .collect()
    }

    /// Which node a segment the renderer pushes names, given where it is.
    fn node_for(&self, segment: &str) -> Option<Node> {
        let label = tags::strip_display(segment);
        match self.path.last() {
            None => SECTIONS.into_iter().find(|&n| section_label(n) == label),
            Some(Some(Node::Settings)) => CHOICES
                .into_iter()
                .find(|k| label == setting_label(k))
                .map(Node::Choice),
            _ => None,
        }
    }
}

fn button(action: &Action, label: &str) -> FfonElement {
    FfonElement::Str(format!("<button>{}</button>{label}", function_name(action)))
}

fn section(label: &str, children: Vec<FfonElement>) -> FfonElement {
    let mut obj = FfonElement::new_obj(label);
    let o = obj.as_obj_mut().expect("new_obj is an Obj");
    for c in children {
        o.push(c);
    }
    obj
}

fn checkbox(s: &AccessibilitySettings, key: &'static str) -> FfonElement {
    let label = setting_label(key);
    FfonElement::Str(if s.get(key).as_deref() == Some("true") {
        tags::format_checkbox_checked(&label)
    } else {
        tags::format_checkbox(&label)
    })
}

fn control_rows() -> Vec<FfonElement> {
    let mut out: Vec<FfonElement> = [power::SUSPEND, power::REBOOT, power::POWEROFF]
        .into_iter()
        .map(|w| {
            button(
                &Action::Power(w),
                &t(power::Commands::label_key(w).unwrap_or_default()),
            )
        })
        .collect();
    out.push(button(&Action::Logout, &t("superkey-button-logout")));
    out
}

pub fn section_label(node: Node) -> String {
    t(match node {
        Node::Windows => "superkey-section-windows",
        Node::Controls => "superkey-section-controls",
        Node::Settings | Node::Choice(_) => "superkey-section-settings",
    })
}

/// A window as the list shows it: its title, then which program it is.
pub fn window_label(w: &WindowInfo) -> String {
    match (w.title.trim(), w.app_id.trim()) {
        ("", "") => t("superkey-untitled-window"),
        ("", app) => app.to_owned(),
        (title, "") => title.to_owned(),
        (title, app) if title.eq_ignore_ascii_case(app) => title.to_owned(),
        (title, app) => format!("{title} ({app})"),
    }
}

pub fn setting_label(key: &str) -> String {
    t(match key {
        KEY_SCREEN_READER => "superkey-setting-screen-reader",
        KEY_FONT_SCALE => "superkey-setting-font-scale",
        KEY_COLOR_SCHEME => "superkey-setting-color-scheme",
        KEY_LANGUAGE => "superkey-setting-language",
        KEY_SHOULDER_SURFING => "superkey-setting-shoulder-surfing",
        _ => return key.to_owned(),
    })
}

/// A setting's value as it is read out: "dark", "Nederlands (België)", "1.75".
pub fn value_label(key: &str, value: &str) -> String {
    match key {
        KEY_COLOR_SCHEME => t(&format!("superkey-color-{value}")),
        KEY_LANGUAGE => t(&format!("superkey-language-{value}")),
        KEY_SCREEN_READER | KEY_SHOULDER_SURFING => t(if value == "true" {
            "superkey-on"
        } else {
            "superkey-off"
        }),
        _ => value.to_owned(),
    }
}

fn options(key: &str) -> &'static [&'static str] {
    match key {
        KEY_FONT_SCALE => FONT_SCALES,
        KEY_COLOR_SCHEME => COLOR_SCHEMES,
        KEY_LANGUAGE => LANGUAGES,
        _ => &[],
    }
}

impl Provider for SuperkeyProvider {
    fn name(&self) -> &str {
        "superkey"
    }

    fn display_name(&self) -> String {
        t("superkey-title")
    }

    fn fetch(&mut self) -> Vec<FfonElement> {
        match self.path.last() {
            None => self.root(),
            Some(Some(node)) => self.rows(*node),
            Some(None) => Vec::new(),
        }
    }

    fn push_path(&mut self, segment: &str) {
        let node = self.node_for(segment);
        self.path.push(node);
        self.update_path_str();
    }

    fn pop_path(&mut self) {
        self.path.pop();
        self.update_path_str();
    }

    fn set_current_path(&mut self, path: &str) {
        // Only ever reset to the root: the host reopens from there on every
        // show, and a deeper path is always reached by pushing.
        if path == "/" {
            self.path.clear();
            self.update_path_str();
        }
    }

    fn current_path(&self) -> &str {
        &self.path_str
    }

    /// A settings checkbox, toggled by the renderer: `checked` is the new
    /// state.
    fn on_checkbox_change(&mut self, label: &str, checked: bool) {
        let label = tags::strip_display(label);
        let key = [KEY_SCREEN_READER, KEY_SHOULDER_SURFING]
            .into_iter()
            .find(|k| setting_label(k) == label);
        if let Some(key) = key {
            self.shared()
                .actions
                .push(Action::Set(key, checked.to_string()));
        }
    }

    /// A settings radio option, chosen by the renderer: `group` is the
    /// group's label and `value` the option's, as shown.
    fn on_radio_change(&mut self, group: &str, value: &str) {
        let group = tags::strip_display(group);
        let Some(key) = CHOICES.into_iter().find(|k| setting_label(k) == group) else {
            return;
        };
        if let Some(stored) = options(key).iter().find(|v| value_label(key, v) == value) {
            self.shared()
                .actions
                .push(Action::Set(key, (*stored).to_owned()));
        }
    }

    fn on_button_press(&mut self, function_name: &str) {
        match parse(function_name) {
            Some(action) => self.shared().actions.push(action),
            None => tracing::debug!("not a superkey action: {function_name}"),
        }
    }

    fn tick(&mut self) -> bool {
        std::mem::take(&mut self.shared().dirty)
    }

    fn take_announcement(&mut self) -> Option<String> {
        self.shared().announcement.take()
    }

    fn take_error(&mut self) -> Option<String> {
        self.shared().error.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider() -> (SuperkeyProvider, SharedState) {
        crate::i18n::init();
        sicompass_sdk::localize::set_locale("en-US");
        let shared: SharedState = Arc::new(Mutex::new(Shared {
            windows: vec![
                WindowInfo::new(4, "vim", "foot", true),
                WindowInfo::new(1, "Inbox", "sicompass", false),
            ],
            apps: vec![App {
                id: "foot".into(),
                name: "Foot".into(),
                exec: vec!["foot".into()],
                path: None,
            }],
            settings: AccessibilitySettings::builtin(),
            ..Shared::default()
        }));
        (SuperkeyProvider::new(Arc::clone(&shared)), shared)
    }

    fn texts(v: &[FfonElement]) -> Vec<String> {
        v.iter()
            .map(|e| match e {
                FfonElement::Str(s) => s.clone(),
                FfonElement::Obj(o) => format!("+{}", o.key),
            })
            .collect()
    }

    #[test]
    fn the_root_is_three_sections_then_the_programs() {
        let (mut p, _) = provider();
        assert_eq!(
            texts(&p.fetch()),
            [
                "+Windows",
                "+Controls",
                "+Settings",
                "<button>app:foot</button>Foot"
            ]
        );
    }

    #[test]
    fn every_section_is_reached_by_its_label_and_holds_only_buttons() {
        let (mut p, _) = provider();
        for (label, want) in [
            (
                "Windows",
                vec![
                    "<button>window:4</button>vim (foot)",
                    "<button>window:1</button>Inbox (sicompass)",
                ],
            ),
            (
                "Controls",
                vec![
                    "<button>power:suspend</button>Suspend",
                    "<button>power:reboot</button>Restart",
                    "<button>power:poweroff</button>Shut down",
                    "<button>session:logout</button>Log out",
                ],
            ),
        ] {
            p.push_path(label);
            assert_eq!(texts(&p.fetch()), want);
            p.pop_path();
        }
        assert_eq!(p.current_path(), "/");
    }

    #[test]
    fn settings_are_checkboxes_and_radio_groups_like_the_apps() {
        let (mut p, _) = provider();
        p.push_path("Settings");
        assert_eq!(p.current_path(), "/settings");
        assert_eq!(
            texts(&p.fetch()),
            [
                "<checkbox>screen reader",
                "+<radio>font scale",
                "+<radio>color scheme",
                "+<radio>language",
                "<checkbox>shoulder-surfing protection (blank screen)",
            ]
        );
        p.push_path("<radio>color scheme");
        assert_eq!(p.current_path(), "/settings/colorScheme");
        assert_eq!(
            texts(&p.fetch()),
            [tags::format_checked("dark"), "light".to_owned()]
        );
    }

    #[test]
    fn a_checked_setting_shows_as_checked() {
        let (mut p, shared) = provider();
        {
            let mut s = shared.lock().unwrap();
            s.settings.set(KEY_SCREEN_READER, "true");
            s.settings.set(KEY_LANGUAGE, "nl-BE");
        }
        p.push_path("Settings");
        assert_eq!(
            texts(&p.fetch())[0],
            tags::format_checkbox_checked("screen reader")
        );
        p.push_path("<radio>language");
        assert_eq!(
            texts(&p.fetch())[1],
            tags::format_checked("Nederlands (België)")
        );
    }

    #[test]
    fn a_toggle_or_a_choice_is_queued_as_the_setting_it_names() {
        let (mut p, shared) = provider();
        p.on_checkbox_change(&tags::format_checkbox_checked("screen reader"), true);
        p.on_checkbox_change("shoulder-surfing protection (blank screen)", false);
        p.on_radio_change("<radio>font scale", "2.00");
        p.on_radio_change("color scheme", "light");
        p.on_radio_change("language", "Deutsch (Belgien)");
        // Not a setting of ours: ignored.
        p.on_checkbox_change("wallpaper", true);
        p.on_radio_change("color scheme", "purple");
        assert_eq!(
            shared.lock().unwrap().actions,
            [
                Action::Set(KEY_SCREEN_READER, "true".into()),
                Action::Set(KEY_SHOULDER_SURFING, "false".into()),
                Action::Set(KEY_FONT_SCALE, "2.00".into()),
                Action::Set(KEY_COLOR_SCHEME, "light".into()),
                Action::Set(KEY_LANGUAGE, "de-BE".into()),
            ]
        );
    }

    #[test]
    fn a_nested_section_carries_the_same_rows_as_its_fetch() {
        // The renderer may descend into the children it already has, or push
        // and fetch: both must show the same thing.
        let (mut p, _) = provider();
        let root = p.fetch();
        for (i, label) in ["Windows", "Controls", "Settings"].iter().enumerate() {
            let nested = root[i].as_obj().unwrap().children.clone();
            p.push_path(label);
            assert_eq!(texts(&nested), texts(&p.fetch()), "{label}");
            p.pop_path();
        }
    }

    #[test]
    fn no_windows_is_a_plain_row_not_an_empty_level() {
        let (mut p, shared) = provider();
        shared.lock().unwrap().windows.clear();
        p.push_path("Windows");
        assert_eq!(texts(&p.fetch()), ["No open windows"]);
    }

    #[test]
    fn an_unknown_segment_is_an_empty_level() {
        let (mut p, _) = provider();
        p.push_path("Elsewhere");
        assert!(p.fetch().is_empty());
        p.set_current_path("/");
        assert_eq!(p.fetch().len(), 4);
    }

    #[test]
    fn a_press_queues_the_action_and_nothing_else() {
        let (mut p, shared) = provider();
        p.on_button_press("window:4");
        p.on_button_press("nonsense");
        p.on_button_press("set:language:nl-BE");
        assert_eq!(
            shared.lock().unwrap().actions,
            [Action::Focus(4), Action::Set(KEY_LANGUAGE, "nl-BE".into())]
        );
    }

    #[test]
    fn tick_reports_a_change_once() {
        let (mut p, shared) = provider();
        assert!(!p.tick());
        shared.lock().unwrap().dirty = true;
        assert!(p.tick());
        assert!(!p.tick());
    }

    #[test]
    fn window_labels() {
        crate::i18n::init();
        sicompass_sdk::localize::set_locale("en-US");
        let w = |t: &str, a: &str| window_label(&WindowInfo::new(1, t, a, false));
        assert_eq!(w("vim", "foot"), "vim (foot)");
        assert_eq!(w("", "foot"), "foot");
        assert_eq!(w("notes", ""), "notes");
        assert_eq!(w("Foot", "foot"), "Foot");
        assert_eq!(w(" ", ""), "Untitled window");
    }
}
