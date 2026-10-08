//! The superkey's list, as a sicompass provider.
//!
//! ```text
//! + Notifications (2) [n] one button per notification: Enter dismisses it
//! + Windows [w]           one button per open window, most recent first
//! + Status [b]            what the bar's icons show, in words
//!   + Tray                one button per tray item: Enter activates it
//! + Tutorial [t]          the sicompass tutorial, which the app leaves out in
//!                         a session
//! + Settings [s]
//!   +R color scheme       dark, light
//!   +R language           the four languages, each named in itself
//!   + Accessibility       screen reader, font scale, shoulder-surfing protection
//!   + Bar                 where the bar sits, seconds on its clock, the keys
//!                         being pressed
//! + Controls [c]          suspend, restart, shut down, log out
//! -b Firefox              then every installed program, by name
//! ```
//!
//! A section's label ends in the key that opens it with Super held.
//!
//! Every leaf is a `<button>` whose function name is an [`Action`], and pressing
//! one only queues the action: the host (`gui.rs`) owns the compositor channel
//! and the renderer, and carries it out. What the list shows comes from
//! [`Shared`], which the host fills in (the window list, the programs, the
//! current settings).
//!
//! `fetch` is path-scoped: the whole tree at the root, and one level inside a
//! section. The path is kept as [`Node`]s, not labels, so it survives a
//! language change. Below Tutorial, the levels are the embedded
//! [`TutorialProvider`]'s, which keeps its own path.

use std::sync::{Arc, Mutex};

use desicompass_bar_protocol::settings::{self as bar, BarSettings};
use desicompass_bar_protocol::status::{
    BatteryState, Connectivity, NetworkKind, Notification, StatusSnapshot, TrayItem,
};
use desicompass_superkey_protocol::WindowInfo;
use sicompass_sdk::ffon::FfonElement;
use sicompass_sdk::provider::Provider;
use sicompass_sdk::tags;
use sicompass_tutorial::TutorialProvider;
use sicompass_ui::accessibility::{
    AccessibilitySettings, COLOR_SCHEMES, FONT_SCALES, KEY_COLOR_SCHEME, KEY_FONT_SCALE,
    KEY_LANGUAGE, KEY_SCREEN_READER, KEY_SHOULDER_SURFING, LANGUAGES,
};

use crate::actions::{Action, function_name, parse};
use crate::apps::App;
use crate::i18n::{t, t_with};
use crate::power;

/// What the host tells the provider, and what the provider hands back.
#[derive(Debug, Default)]
pub struct Shared {
    pub windows: Vec<WindowInfo>,
    pub apps: Vec<App>,
    /// The effective accessibility settings, every key set.
    pub settings: AccessibilitySettings,
    /// The bar's settings.
    pub bar: BarSettings,
    /// What the bar last reported.
    pub status: StatusSnapshot,
    /// The date and time, as the Status section reads it. Set by the host,
    /// on the minute.
    pub clock: String,
    /// Pressed, not yet carried out.
    pub actions: Vec<Action>,
    pub announcement: Option<String>,
    pub error: Option<String>,
    /// Something shown changed: the next tick rebuilds the list.
    pub dirty: bool,
    /// The tutorial as the user has it: the boxes ticked, the radios chosen and
    /// the inputs edited. The host copies it from the renderer
    /// (`gui::remember_tutorial`), and the list is rebuilt from it, so an edit
    /// survives leaving the section and hiding the superkey, the way it lasts
    /// in the app. `None` until the tutorial is first entered.
    pub tutorial: Option<TutorialState>,
}

/// The tutorial's rows in one language. Another language starts it afresh:
/// its labels, and the path through them, are that language's.
#[derive(Debug, Clone)]
pub struct TutorialState {
    pub locale: String,
    pub rows: Vec<FfonElement>,
}

pub type SharedState = Arc<Mutex<Shared>>;

/// A level below the root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Node {
    Windows,
    Controls,
    Settings,
    Status,
    /// Inside Settings: the accessibility settings.
    Accessibility,
    /// Inside Settings: the bar's settings.
    Bar,
    /// Inside Status.
    Notifications,
    /// Inside Status.
    Tray,
    /// The options of one setting with a list of values.
    Choice(&'static str),
    /// The sicompass tutorial, which the app leaves out in a session.
    Tutorial,
    /// A level inside the tutorial. Which one is the embedded provider's path.
    TutorialPage,
}

/// The sections, in the order the root lists them. The index is what the host
/// lands the cursor on.
pub const SECTIONS: [Node; 6] = [
    Node::Notifications,
    Node::Windows,
    Node::Status,
    Node::Tutorial,
    Node::Settings,
    Node::Controls,
];

/// The groups inside Settings, in order, after [`SETTINGS_CHOICES`].
pub const GROUPS: [Node; 2] = [Node::Accessibility, Node::Bar];

/// The choices at the top of Settings. They are the session's, shared with the
/// login screen like the accessibility settings, but not accessibility.
const SETTINGS_CHOICES: [&str; 2] = [KEY_COLOR_SCHEME, KEY_LANGUAGE];

/// The accessibility settings with a list of values.
const CHOICES: [&str; 1] = [KEY_FONT_SCALE];

/// The bar's settings with a list of values.
const BAR_CHOICES: [&str; 1] = [bar::KEY_POSITION];

/// Every on/off setting: the accessibility ones, then the bar's.
const SWITCHES: [&str; 4] = [
    KEY_SCREEN_READER,
    KEY_SHOULDER_SURFING,
    bar::KEY_SECONDS,
    bar::KEY_KEYSTROKES,
];

pub struct SuperkeyProvider {
    shared: SharedState,
    /// One entry per level below the root. `None` for a segment this provider
    /// did not recognise, which then has nothing in it.
    path: Vec<Option<Node>>,
    path_str: String,
    /// Below Tutorial, every level is this provider's: its rows, its button.
    tutorial: TutorialProvider,
    /// The labels pushed below Tutorial, to walk [`Shared::tutorial`] by. Kept
    /// whole rather than read back from a path string, because a label may
    /// hold a `/` ("Insert/edit").
    tutorial_path: Vec<String>,
}

impl SuperkeyProvider {
    pub fn new(shared: SharedState) -> Self {
        // Its strings, and the two files it shows (`asset:tutorial/…`).
        sicompass_tutorial::register_translations();
        sicompass_tutorial::register();
        Self {
            shared,
            path: Vec::new(),
            path_str: "/".to_owned(),
            tutorial: TutorialProvider::new(),
            tutorial_path: Vec::new(),
        }
    }

    /// The tutorial's rows at its root, as the user has them.
    fn tutorial_rows(&self) -> Vec<FfonElement> {
        let locale = sicompass_sdk::localize::current_locale();
        match &self.shared().tutorial {
            Some(t) if t.locale == locale => t.rows.clone(),
            _ => TutorialProvider::new().fetch(),
        }
    }

    /// The rows of the tutorial level the path names, as the user has them.
    ///
    /// Except the programs section, which is always the tutorial's own answer:
    /// it lists the plugins installed now, and the user edits nothing in it.
    fn tutorial_level(&mut self) -> Vec<FfonElement> {
        if self.tutorial.in_programs_section() {
            return self.tutorial.fetch();
        }
        let mut rows = self.tutorial_rows();
        for segment in &self.tutorial_path {
            let found = rows.into_iter().find_map(|e| match e {
                FfonElement::Obj(o) if tags::strip_display(&o.key) == *segment => Some(o.children),
                _ => None,
            });
            match found {
                Some(children) => rows = children,
                // Not in the remembered tree: the tutorial's own answer.
                None => return self.tutorial.fetch(),
            }
        }
        rows
    }

    /// Whether the level shown is the Tutorial section or one inside it.
    fn in_tutorial(&self) -> bool {
        matches!(
            self.path.last(),
            Some(Some(Node::Tutorial | Node::TutorialPage))
        )
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
                .filter(|n| **n != Some(Node::TutorialPage))
                .map(|n| match n {
                    Some(Node::Tutorial) => "tutorial".to_owned(),
                    Some(Node::TutorialPage) => unreachable!("filtered out"),
                    Some(Node::Windows) => "windows".to_owned(),
                    Some(Node::Controls) => "controls".to_owned(),
                    Some(Node::Settings) => "settings".to_owned(),
                    Some(Node::Status) => "status".to_owned(),
                    Some(Node::Accessibility) => "accessibility".to_owned(),
                    Some(Node::Bar) => "bar".to_owned(),
                    Some(Node::Notifications) => "notifications".to_owned(),
                    Some(Node::Tray) => "tray".to_owned(),
                    Some(Node::Choice(k)) => (*k).to_owned(),
                    None => "?".to_owned(),
                })
                .collect();
            let below = self.tutorial.current_path().trim_start_matches('/');
            if below.is_empty() {
                format!("/{}", segs.join("/"))
            } else {
                format!("/{}/{below}", segs.join("/"))
            }
        };
    }

    // ---- Rows --------------------------------------------------------------

    fn root(&self) -> Vec<FfonElement> {
        let mut out: Vec<FfonElement> = SECTIONS
            .iter()
            .map(|&n| section(&self.label(n), self.rows(n)))
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
            Node::Settings => SETTINGS_CHOICES
                .iter()
                .map(|&k| self.radio(k))
                .chain(
                    GROUPS
                        .iter()
                        .map(|&g| section(&self.label(g), self.rows(g))),
                )
                .collect(),
            Node::Accessibility => self.settings_rows(),
            Node::Bar => self.bar_rows(),
            Node::Status => self.status_rows(),
            Node::Notifications => self.notification_rows(),
            Node::Tray => self.tray_rows(),
            Node::Choice(key) => self.choice_rows(key),
            // At its root: `rows` is the nested tree the root fetch hands
            // out, whatever level the live provider is at.
            Node::Tutorial => self.tutorial_rows(),
            // Only reached through `fetch`, which asks the live one.
            Node::TutorialPage => Vec::new(),
        }
    }

    /// A node's label, with the notification count where it has one, and
    /// the key that opens it where there is one: `Notifications (2) [n]`.
    fn label(&self, node: Node) -> String {
        let base = match node {
            Node::Notifications => notifications_label(self.shared().status.notifications.len()),
            n => section_label(n),
        };
        match shortcut(node) {
            Some(key) => format!("{base} [{key}]"),
            None => base,
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
        let on = |key| s.get(key).as_deref() == Some("true");
        let mut out = vec![checkbox(on(KEY_SCREEN_READER), KEY_SCREEN_READER)];
        for key in CHOICES {
            out.push(self.radio(key));
        }
        out.push(checkbox(on(KEY_SHOULDER_SURFING), KEY_SHOULDER_SURFING));
        out
    }

    /// The bar's settings: where it sits, seconds on the clock, and the keys
    /// being pressed.
    fn bar_rows(&self) -> Vec<FfonElement> {
        let settings = self.shared().bar;
        let mut out: Vec<FfonElement> = BAR_CHOICES.iter().map(|&k| self.radio(k)).collect();
        out.push(checkbox(settings.seconds, bar::KEY_SECONDS));
        out.push(checkbox(settings.keystrokes, bar::KEY_KEYSTROKES));
        out
    }

    fn radio(&self, key: &'static str) -> FfonElement {
        section(
            &format!("<radio>{}", setting_label(key)),
            self.choice_rows(key),
        )
    }

    /// What the bar shows, in words: the clock, then each service there is,
    /// then the tray. The notifications have a section of their own.
    fn status_rows(&self) -> Vec<FfonElement> {
        let (clock, status) = {
            let s = self.shared();
            (s.clock.clone(), s.status.clone())
        };
        let mut out: Vec<FfonElement> = [Some(clock).filter(|c| !c.is_empty())]
            .into_iter()
            .chain([
                status.network.map(|n| {
                    let text = t_with(
                        match n.kind {
                            NetworkKind::Wired => "superkey-network-wired",
                            NetworkKind::Wireless if n.strength.is_some() => {
                                "superkey-network-wireless-signal"
                            }
                            NetworkKind::Wireless => "superkey-network-wireless",
                            NetworkKind::Other => "superkey-network-other",
                            NetworkKind::None => "superkey-network-none",
                        },
                        &[("strength", &n.strength.unwrap_or(0).to_string())],
                    );
                    let cut_off = n.kind != NetworkKind::None
                        && matches!(n.connectivity, Connectivity::None | Connectivity::Limited);
                    if cut_off {
                        format!("{text}, {}", t("superkey-network-no-internet"))
                    } else {
                        text
                    }
                }),
                status.audio.map(|a| {
                    if a.muted {
                        t("superkey-volume-muted")
                    } else {
                        t_with("superkey-volume", &[("percent", &a.volume.to_string())])
                    }
                }),
                status.battery.map(|b| {
                    let percent = b.percent.to_string();
                    match b.state {
                        BatteryState::Full => t("superkey-battery-full"),
                        BatteryState::Charging => {
                            t_with("superkey-battery-charging", &[("percent", &percent)])
                        }
                        _ => t_with("superkey-battery", &[("percent", &percent)]),
                    }
                }),
                status.bluetooth.map(|b| match (b.powered, b.connected) {
                    (false, _) => t("superkey-bluetooth-off"),
                    (true, 0) => t("superkey-bluetooth-on"),
                    (true, n) => {
                        t_with("superkey-bluetooth-connected", &[("count", &n.to_string())])
                    }
                }),
            ])
            .flatten()
            .map(FfonElement::Str)
            .collect();
        out.push(section(&section_label(Node::Tray), tray_rows(&status.tray)));
        out
    }

    fn notification_rows(&self) -> Vec<FfonElement> {
        notification_rows(&self.shared().status.notifications.clone())
    }

    fn tray_rows(&self) -> Vec<FfonElement> {
        tray_rows(&self.shared().status.tray.clone())
    }

    /// A radio group's options, the current one checked.
    fn choice_rows(&self, key: &'static str) -> Vec<FfonElement> {
        let current = {
            let s = self.shared();
            if BarSettings::is_key(key) {
                s.bar.get(key)
            } else {
                s.settings.get(key)
            }
        }
        .unwrap_or_default();
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
        let choice = |keys: &[&'static str]| {
            keys.iter()
                .copied()
                .find(|k| label == setting_label(k))
                .map(Node::Choice)
        };
        // Compared without the key and the count: the count in
        // "Notifications (2) [n]" changes while it is open.
        let named = |nodes: &[Node]| {
            nodes
                .iter()
                .copied()
                .find(|&n| plain_label(&section_label(n)) == plain_label(&label))
        };
        match self.path.last() {
            None => named(&SECTIONS),
            Some(Some(Node::Settings)) => choice(&SETTINGS_CHOICES).or_else(|| named(&GROUPS)),
            Some(Some(Node::Accessibility)) => choice(&CHOICES),
            Some(Some(Node::Bar)) => choice(&BAR_CHOICES),
            Some(Some(Node::Status)) => named(&[Node::Tray]),
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

fn checkbox(on: bool, key: &'static str) -> FfonElement {
    let label = setting_label(key);
    FfonElement::Str(if on {
        tags::format_checkbox_checked(&label)
    } else {
        tags::format_checkbox(&label)
    })
}

/// The notifications, oldest first, each a button that dismisses it; "Dismiss
/// all" first when there is more than one.
fn notification_rows(list: &[Notification]) -> Vec<FfonElement> {
    if list.is_empty() {
        return vec![FfonElement::Str(t("superkey-no-notifications"))];
    }
    let mut out = Vec::new();
    if list.len() > 1 {
        out.push(button(&Action::DismissAll, &t("superkey-dismiss-all")));
    }
    out.extend(
        list.iter()
            .map(|n| button(&Action::Dismiss(n.id), &notification_label(n))),
    );
    out
}

fn tray_rows(items: &[TrayItem]) -> Vec<FfonElement> {
    if items.is_empty() {
        return vec![FfonElement::Str(t("superkey-tray-empty"))];
    }
    items
        .iter()
        .map(|i| {
            button(
                &Action::Activate {
                    service: i.service.clone(),
                    path: i.path.clone(),
                },
                &one_line(&i.title),
            )
        })
        .collect()
}

/// A notification as one row: `Mail: New message, Hello there`.
pub fn notification_label(n: &Notification) -> String {
    let parts: Vec<String> = [&n.summary, &n.body]
        .into_iter()
        .map(|p| one_line(p))
        .filter(|p| !p.is_empty())
        .collect();
    let text = parts.join(", ");
    match one_line(&n.app) {
        app if app.is_empty() => text,
        app if text.is_empty() => app,
        app => format!("{app}: {text}"),
    }
}

/// Text from another program, as one row: its lines joined, and no markup,
/// which some senders use although this server does not offer it, and which
/// the list would read as its own tags.
fn one_line(s: &str) -> String {
    let mut plain = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            c if !in_tag => plain.push(c),
            _ => {}
        }
    }
    plain.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn notifications_label(count: usize) -> String {
    t_with(
        "superkey-status-notifications",
        &[("count", &count.to_string())],
    )
}

/// The key that opens a section with Super held, as the compositor binds it.
pub fn shortcut(node: Node) -> Option<char> {
    match node {
        Node::Windows => Some('w'),
        Node::Controls => Some('c'),
        Node::Settings => Some('s'),
        Node::Status => Some('b'),
        Node::Notifications => Some('n'),
        Node::Tutorial => Some('t'),
        _ => None,
    }
}

/// A label without its key and its count: `Notifications (2) [n]` is
/// `Notifications`, whatever the language.
fn plain_label(label: &str) -> &str {
    let mut l = label;
    for (open, close) in [(" [", ']'), (" (", ')')] {
        if l.ends_with(close)
            && let Some(i) = l.rfind(open)
        {
            l = &l[..i];
        }
    }
    l
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
    match node {
        Node::Notifications => notifications_label(0),
        n => t(match n {
            Node::Windows => "superkey-section-windows",
            Node::Controls => "superkey-section-controls",
            Node::Status => "superkey-section-status",
            Node::Accessibility => "superkey-group-accessibility",
            Node::Bar => "superkey-group-bar",
            Node::Tray => "superkey-status-tray",
            Node::Tutorial => "superkey-section-tutorial",
            _ => "superkey-section-settings",
        }),
    }
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
        bar::KEY_POSITION => "superkey-setting-bar-position",
        bar::KEY_SECONDS => "superkey-setting-bar-seconds",
        bar::KEY_KEYSTROKES => "superkey-setting-bar-keystrokes",
        _ => return key.to_owned(),
    })
}

/// A setting's value as it is read out: "dark", "Nederlands (België)", "1.75".
pub fn value_label(key: &str, value: &str) -> String {
    match key {
        KEY_COLOR_SCHEME => t(&format!("superkey-color-{value}")),
        KEY_LANGUAGE => t(&format!("superkey-language-{value}")),
        bar::KEY_POSITION => t(&format!("superkey-bar-{value}")),
        KEY_SCREEN_READER | KEY_SHOULDER_SURFING | bar::KEY_SECONDS | bar::KEY_KEYSTROKES => {
            t(if value == "true" {
                "superkey-on"
            } else {
                "superkey-off"
            })
        }
        _ => value.to_owned(),
    }
}

fn options(key: &str) -> &'static [&'static str] {
    match key {
        KEY_FONT_SCALE => FONT_SCALES,
        KEY_COLOR_SCHEME => COLOR_SCHEMES,
        KEY_LANGUAGE => LANGUAGES,
        bar::KEY_POSITION => bar::POSITIONS,
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
            Some(Some(Node::TutorialPage)) => self.tutorial_level(),
            Some(Some(node)) => self.rows(*node),
            Some(None) => Vec::new(),
        }
    }

    fn push_path(&mut self, segment: &str) {
        if self.in_tutorial() {
            self.tutorial.push_path(segment);
            self.tutorial_path.push(segment.to_owned());
            self.path.push(Some(Node::TutorialPage));
        } else {
            let node = self.node_for(segment);
            self.path.push(node);
        }
        self.update_path_str();
    }

    fn pop_path(&mut self) {
        match self.path.pop() {
            Some(Some(Node::TutorialPage)) => {
                self.tutorial.pop_path();
                self.tutorial_path.pop();
            }
            Some(Some(Node::Tutorial)) => {
                self.tutorial.set_current_path("/");
                self.tutorial_path.clear();
            }
            _ => {}
        }
        self.update_path_str();
    }

    fn set_current_path(&mut self, path: &str) {
        // Only ever reset to the root: the host reopens from there on every
        // show, and a deeper path is always reached by pushing.
        if path == "/" {
            self.path.clear();
            self.tutorial.set_current_path("/");
            self.tutorial_path.clear();
            self.update_path_str();
        }
    }

    fn current_path(&self) -> &str {
        &self.path_str
    }

    /// A settings checkbox, toggled by the renderer: `checked` is the new
    /// state.
    fn on_checkbox_change(&mut self, label: &str, checked: bool) {
        // The tutorial's practice boxes are not settings.
        if self.in_tutorial() {
            return;
        }
        let label = tags::strip_display(label);
        let key = SWITCHES.into_iter().find(|k| setting_label(k) == label);
        if let Some(key) = key {
            self.shared()
                .actions
                .push(Action::Set(key, checked.to_string()));
        }
    }

    /// A settings radio option, chosen by the renderer: `group` is the
    /// group's label and `value` the option's, as shown.
    fn on_radio_change(&mut self, group: &str, value: &str) {
        if self.in_tutorial() {
            return;
        }
        let group = tags::strip_display(group);
        let Some(key) = SETTINGS_CHOICES
            .into_iter()
            .chain(CHOICES)
            .chain(BAR_CHOICES)
            .find(|k| setting_label(k) == group)
        else {
            return;
        };
        if let Some(stored) = options(key).iter().find(|v| value_label(key, v) == value) {
            self.shared()
                .actions
                .push(Action::Set(key, (*stored).to_owned()));
        }
    }

    fn on_button_press(&mut self, function_name: &str) {
        // The tutorial's playground button, which only says it was pressed.
        if self.in_tutorial() {
            self.tutorial.on_button_press(function_name);
            return;
        }
        match parse(function_name) {
            Some(action) => self.shared().actions.push(action),
            None => tracing::debug!("not a superkey action: {function_name}"),
        }
    }

    fn tick(&mut self) -> bool {
        // Nothing the host changes is shown inside the tutorial, and a rebuild
        // there would land in the middle of an edit. The change waits for the
        // first tick outside it. Only the tutorial itself may ask, and it asks
        // only in its programs section, after a plugin is installed or removed.
        if self.in_tutorial() {
            return self.tutorial.tick();
        }
        std::mem::take(&mut self.shared().dirty)
    }

    fn take_announcement(&mut self) -> Option<String> {
        self.shared().announcement.take()
    }

    fn take_error(&mut self) -> Option<String> {
        let error = self.shared().error.take();
        error.or_else(|| self.tutorial.take_error())
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
    fn the_root_is_the_sections_then_the_programs() {
        let (mut p, _) = provider();
        assert_eq!(
            texts(&p.fetch()),
            [
                "+Notifications (0) [n]",
                "+Windows [w]",
                "+Status [b]",
                "+Tutorial [t]",
                "+Settings [s]",
                "+Controls [c]",
                "<button>app:foot</button>Foot"
            ]
        );
    }

    /// The tutorial's own levels, at its root and one level down.
    fn tutorial_at(path: &[&str]) -> Vec<String> {
        let mut t = TutorialProvider::new();
        for seg in path {
            t.push_path(seg);
        }
        texts(&t.fetch())
    }

    #[test]
    fn the_tutorial_section_is_the_tutorials_root() {
        let (mut p, _) = provider();
        let root = p.fetch();
        let nested = root[3].as_obj().unwrap();
        assert_eq!(nested.key, "Tutorial [t]");
        let sections = texts(&nested.children);
        assert_eq!(sections, tutorial_at(&[]));
        assert_eq!(sections.len(), 7, "{sections:?}");
        assert!(sections[0].starts_with("+Getting Started"), "{sections:?}");
    }

    /// A tutorial section's label, as the renderer pushes it.
    fn tutorial_section(starts: &str) -> String {
        tutorial_at(&[])
            .into_iter()
            .find_map(|s| {
                s.strip_prefix('+')
                    .filter(|l| l.starts_with(starts))
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| panic!("no tutorial section {starts}"))
    }

    #[test]
    fn inside_the_tutorial_the_levels_are_the_tutorials() {
        let (mut p, _) = provider();
        p.push_path("Tutorial [t]");
        assert_eq!(p.current_path(), "/tutorial");
        assert_eq!(texts(&p.fetch()), tutorial_at(&[]));

        let getting_started = tutorial_section("Getting Started");
        p.push_path(&getting_started);
        assert_eq!(p.current_path(), format!("/tutorial/{getting_started}"));
        assert_eq!(texts(&p.fetch()), tutorial_at(&[&getting_started]));
        assert!(!p.fetch().is_empty());

        p.pop_path();
        assert_eq!(p.current_path(), "/tutorial");
        assert_eq!(texts(&p.fetch()), tutorial_at(&[]));
        p.pop_path();
        assert_eq!(p.current_path(), "/");
        assert_eq!(p.fetch().len(), 7);
    }

    #[test]
    fn leaving_the_tutorial_from_deep_inside_starts_it_over() {
        let (mut p, _) = provider();
        p.push_path("Tutorial");
        p.push_path(&tutorial_section("Getting Started"));
        p.set_current_path("/");
        p.push_path("Tutorial");
        assert_eq!(texts(&p.fetch()), tutorial_at(&[]));
    }

    /// The programs section lists the plugins installed now, not the ones the
    /// remembered tutorial was built with, and asks to be read again after an
    /// install.
    #[test]
    fn the_tutorials_programs_follow_the_plugins_folder() {
        let (mut p, shared) = provider();
        let plugins = tempfile::tempdir().unwrap();
        p.tutorial = TutorialProvider::with_plugins_dir(Some(plugins.path().to_owned()));
        shared.lock().unwrap().tutorial = Some(TutorialState {
            locale: sicompass_sdk::localize::current_locale(),
            rows: TutorialProvider::with_plugins_dir(Some(plugins.path().to_owned())).fetch(),
        });
        p.push_path("Tutorial");
        p.push_path(&tutorial_section("The programs"));
        assert!(
            !texts(&p.fetch())
                .iter()
                .any(|s| s.contains("during the session"))
        );

        let plugin = plugins.path().join("skfake");
        std::fs::create_dir_all(plugin.join("locales")).unwrap();
        std::fs::write(
            plugin.join("plugin.json"),
            r#"{"name":"skfake","displayName":"fake","entry":"plugin.wasm"}"#,
        )
        .unwrap();
        std::fs::write(
            plugin.join("locales").join("en-US.ftl"),
            "skfake-tutorial = Installed during the session\n",
        )
        .unwrap();

        assert!(p.tick(), "an install must have the section read again");
        assert!(
            texts(&p.fetch()).contains(&"Installed during the session".to_owned()),
            "{:?}",
            texts(&p.fetch())
        );
    }

    #[test]
    fn the_tutorials_button_speaks_and_queues_nothing() {
        let (mut p, shared) = provider();
        p.push_path("Tutorial");
        p.push_path(&tutorial_section("Interactive playground"));
        p.on_button_press("demo");
        assert!(shared.lock().unwrap().actions.is_empty());
        let said = p.take_error();
        assert!(said.is_some_and(|s| !s.is_empty()));
        assert_eq!(p.take_error(), None);
    }

    #[test]
    fn a_section_is_reached_by_its_label_with_or_without_its_key() {
        let (mut p, _) = provider();
        for label in ["Status [b]", "Status"] {
            p.push_path(label);
            assert_eq!(p.current_path(), "/status", "{label}");
            p.pop_path();
        }
        // Only the key's own brackets come off.
        p.push_path("Status [x");
        assert!(p.fetch().is_empty());
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
                "+<radio>color scheme",
                "+<radio>language",
                "+Accessibility",
                "+Bar"
            ]
        );
        p.push_path("Accessibility");
        assert_eq!(p.current_path(), "/settings/accessibility");
        assert_eq!(
            texts(&p.fetch()),
            [
                "<checkbox>screen reader",
                "+<radio>font scale",
                "<checkbox>shoulder-surfing protection (blank screen)",
            ]
        );
        p.pop_path();
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
        p.push_path("Accessibility");
        assert_eq!(
            texts(&p.fetch())[0],
            tags::format_checkbox_checked("screen reader")
        );
        p.pop_path();
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
        for (i, label) in [
            "Notifications",
            "Windows",
            "Status",
            "Tutorial",
            "Settings",
            "Controls",
        ]
        .iter()
        .enumerate()
        {
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
        assert_eq!(p.fetch().len(), 7);
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
    fn the_bar_settings_are_a_position_a_seconds_switch_and_a_keystrokes_switch() {
        let (mut p, shared) = provider();
        p.push_path("Settings");
        p.push_path("Bar");
        assert_eq!(p.current_path(), "/settings/bar");
        assert_eq!(
            texts(&p.fetch()),
            [
                "+<radio>bar position",
                "<checkbox>show seconds",
                "<checkbox>show key strokes"
            ]
        );
        p.push_path("<radio>bar position");
        assert_eq!(p.current_path(), "/settings/bar/barPosition");
        assert_eq!(
            texts(&p.fetch()),
            [tags::format_checked("bottom"), "top".to_owned()]
        );
        shared.lock().unwrap().bar = BarSettings {
            position: desicompass_bar_protocol::Edge::Top,
            seconds: true,
            keystrokes: true,
        };
        assert_eq!(
            texts(&p.fetch()),
            ["bottom".to_owned(), tags::format_checked("top")]
        );
        p.pop_path();
        assert_eq!(
            texts(&p.fetch())[1],
            tags::format_checkbox_checked("show seconds")
        );
        assert_eq!(
            texts(&p.fetch())[2],
            tags::format_checkbox_checked("show key strokes")
        );
    }

    #[test]
    fn a_bar_setting_is_queued_like_the_others() {
        let (mut p, shared) = provider();
        p.on_radio_change("<radio>bar position", "top");
        p.on_checkbox_change("show seconds", true);
        p.on_checkbox_change("<checkbox>show key strokes", true);
        p.on_radio_change("bar position", "left");
        assert_eq!(
            shared.lock().unwrap().actions,
            [
                Action::Set(bar::KEY_POSITION, "top".into()),
                Action::Set(bar::KEY_SECONDS, "true".into()),
                Action::Set(bar::KEY_KEYSTROKES, "true".into()),
            ]
        );
    }

    fn status() -> StatusSnapshot {
        use desicompass_bar_protocol::status::{Audio, Battery, Bluetooth, Network};
        StatusSnapshot {
            network: Some(Network {
                kind: NetworkKind::Wireless,
                connectivity: Connectivity::Limited,
                strength: Some(72),
            }),
            audio: Some(Audio {
                volume: 45,
                muted: false,
            }),
            battery: Some(Battery {
                percent: 81,
                state: BatteryState::Charging,
            }),
            bluetooth: Some(Bluetooth {
                powered: true,
                connected: 2,
            }),
            notifications: vec![
                Notification {
                    id: 7,
                    app: "Mail".into(),
                    summary: "New message".into(),
                    body: "Hello\nthere <b>you</b>".into(),
                },
                Notification {
                    id: 9,
                    app: String::new(),
                    summary: "Backup done".into(),
                    body: String::new(),
                },
            ],
            tray: vec![TrayItem {
                service: ":1.42".into(),
                path: "/org/ayatana/NotificationItem/dropbox".into(),
                title: "Dropbox".into(),
            }],
        }
    }

    #[test]
    fn status_says_in_words_what_the_bar_shows() {
        let (mut p, shared) = provider();
        {
            let mut s = shared.lock().unwrap();
            s.status = status();
            s.clock = "Wednesday 30 September 2026, 14:05".into();
        }
        p.push_path("Status");
        assert_eq!(p.current_path(), "/status");
        assert_eq!(
            texts(&p.fetch()),
            [
                "Wednesday 30 September 2026, 14:05",
                "Network: wireless, signal 72%, no internet",
                "Volume: 45%",
                "Battery: 81%, charging",
                "Bluetooth: on, 2 connected",
                "+Tray",
            ]
        );
    }

    #[test]
    fn a_service_that_is_not_there_has_no_row() {
        let (mut p, _) = provider();
        p.push_path("Status");
        assert_eq!(
            texts(&p.fetch()),
            ["+Tray"],
            "no clock yet, no services, nothing to list"
        );
        p.push_path("Tray");
        assert_eq!(texts(&p.fetch()), ["Nothing in the tray"]);
        p.set_current_path("/");
        p.push_path("Notifications (0) [n]");
        assert_eq!(texts(&p.fetch()), ["No notifications"]);
    }

    #[test]
    fn notifications_are_buttons_that_dismiss_them() {
        let (mut p, shared) = provider();
        shared.lock().unwrap().status = status();
        assert_eq!(texts(&p.fetch())[0], "+Notifications (2) [n]");
        // Entered by the label it had, even after the count has changed.
        p.push_path("Notifications (5) [n]");
        assert_eq!(p.current_path(), "/notifications");
        assert_eq!(
            texts(&p.fetch()),
            [
                "<button>notifications:dismiss-all</button>Dismiss all",
                "<button>notification:7</button>Mail: New message, Hello there you",
                "<button>notification:9</button>Backup done",
            ]
        );
        p.pop_path();
        p.push_path("Status");
        p.push_path("Tray");
        assert_eq!(
            texts(&p.fetch()),
            ["<button>tray::1.42 /org/ayatana/NotificationItem/dropbox</button>Dropbox"]
        );
    }

    #[test]
    fn a_nested_status_carries_the_same_rows_as_its_fetch() {
        let (mut p, shared) = provider();
        shared.lock().unwrap().status = status();
        let root = p.fetch();
        let nested = root[2].as_obj().unwrap().children.clone();
        p.push_path("Status");
        assert_eq!(texts(&nested), texts(&p.fetch()));
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
