//! What a superkey row does, from the `<button>` function name it carries.
//!
//! Every leaf of the superkey's list is a button, and the function name is the
//! whole instruction: the provider parses it into an [`Action`] and queues it
//! for the host, which is the one that can talk to the compositor and the
//! renderer.

use desicompass_bar_protocol::settings::{self as bar, BarSettings};
use sicompass_ui::accessibility::{self, ALL_KEYS};

use crate::power;

/// Something the user asked for by pressing a row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Give this window the keyboard (a compositor window id).
    Focus(u64),
    /// Start the program with this desktop file id.
    Launch(String),
    /// `power::SUSPEND`, `power::REBOOT` or `power::POWEROFF`.
    Power(&'static str),
    /// End the session.
    Logout,
    /// Flip an on/off setting: accessibility, or the bar's.
    Toggle(&'static str),
    /// Choose a value for a setting: accessibility, or the bar's.
    Set(&'static str, String),
    /// Dismiss one notification, by its id.
    Dismiss(u32),
    /// Dismiss every notification.
    DismissAll,
    /// Activate a tray item: its bus name and object path.
    Activate { service: String, path: String },
}

/// Build a button's function name. The inverse of [`parse`].
pub fn function_name(action: &Action) -> String {
    match action {
        Action::Focus(id) => format!("window:{id}"),
        Action::Launch(id) => format!("app:{id}"),
        Action::Power(which) => format!("power:{which}"),
        Action::Logout => "session:logout".to_owned(),
        Action::Toggle(key) => format!("toggle:{key}"),
        Action::Set(key, value) => format!("set:{key}:{value}"),
        Action::Dismiss(id) => format!("notification:{id}"),
        Action::DismissAll => "notifications:dismiss-all".to_owned(),
        // Neither a bus name nor an object path has a space in it.
        Action::Activate { service, path } => format!("tray:{service} {path}"),
    }
}

/// Read a button's function name. `None` for anything this superkey did not
/// write, which is then simply not acted on.
pub fn parse(name: &str) -> Option<Action> {
    let (kind, rest) = name.split_once(':')?;
    match kind {
        "window" => rest.parse().ok().map(Action::Focus),
        "app" if !rest.is_empty() => Some(Action::Launch(rest.to_owned())),
        "power" => [power::SUSPEND, power::REBOOT, power::POWEROFF]
            .into_iter()
            .find(|w| *w == rest)
            .map(Action::Power),
        "session" if rest == "logout" => Some(Action::Logout),
        "toggle" => setting_key(rest)
            .filter(|k| is_switch(k))
            .map(Action::Toggle),
        "set" => {
            let (key, value) = rest.split_once(':')?;
            let key = setting_key(key)?;
            // Only a value the settings would accept: the files are shared,
            // and nothing else may land in them.
            let valid = if BarSettings::is_key(key) {
                BarSettings::default().set(key, value)
            } else {
                accessibility::AccessibilitySettings::default().set(key, value)
            };
            valid.then(|| Action::Set(key, value.to_owned()))
        }
        "notification" => rest.parse().ok().map(Action::Dismiss),
        "notifications" if rest == "dismiss-all" => Some(Action::DismissAll),
        "tray" => {
            let (service, path) = rest.split_once(' ')?;
            (!service.is_empty() && path.starts_with('/')).then(|| Action::Activate {
                service: service.to_owned(),
                path: path.to_owned(),
            })
        }
        _ => None,
    }
}

fn setting_key(s: &str) -> Option<&'static str> {
    ALL_KEYS.iter().chain(bar::KEYS).copied().find(|k| *k == s)
}

/// The settings that are a switch rather than a choice.
pub fn is_switch(key: &str) -> bool {
    key == accessibility::KEY_SCREEN_READER
        || key == accessibility::KEY_SHOULDER_SURFING
        || key == bar::KEY_SECONDS
}

#[cfg(test)]
mod tests {
    use super::*;
    use sicompass_ui::accessibility::{
        KEY_COLOR_SCHEME, KEY_FONT_SCALE, KEY_LANGUAGE, KEY_SCREEN_READER, KEY_SHOULDER_SURFING,
    };

    #[test]
    fn every_action_round_trips() {
        for a in [
            Action::Focus(12),
            Action::Launch("org.gnome.Nautilus".into()),
            Action::Power(power::SUSPEND),
            Action::Power(power::REBOOT),
            Action::Power(power::POWEROFF),
            Action::Logout,
            Action::Toggle(KEY_SCREEN_READER),
            Action::Toggle(KEY_SHOULDER_SURFING),
            Action::Set(KEY_FONT_SCALE, "2.00".into()),
            Action::Set(KEY_COLOR_SCHEME, "light".into()),
            Action::Set(KEY_LANGUAGE, "nl-BE".into()),
            Action::Toggle(bar::KEY_SECONDS),
            Action::Set(bar::KEY_POSITION, "top".into()),
            Action::Dismiss(7),
            Action::DismissAll,
            Action::Activate {
                service: ":1.42".into(),
                path: "/org/ayatana/NotificationItem/dropbox".into(),
            },
        ] {
            assert_eq!(parse(&function_name(&a)), Some(a));
        }
    }

    #[test]
    fn a_desktop_id_may_contain_colons() {
        assert_eq!(parse("app:a:b"), Some(Action::Launch("a:b".into())));
    }

    #[test]
    fn nonsense_is_not_an_action() {
        for name in [
            "",
            "window:",
            "window:abc",
            "app:",
            "power:hibernate",
            "session:reboot",
            "toggle:fontScale",
            "toggle:wallpaper",
            "set:fontScale:huge",
            "set:colorScheme",
            "set:wallpaper:cats",
            "launch:foot",
            "reboot",
            "set:barPosition:left",
            "toggle:barPosition",
            "notification:",
            "notification:x",
            "notifications:all",
            "tray:",
            "tray::1.42",
            "tray::1.42 relative",
            "tray: /path",
        ] {
            assert_eq!(parse(name), None, "{name:?}");
        }
    }
}
