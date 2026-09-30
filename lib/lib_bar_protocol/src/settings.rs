//! The bar's settings: where it sits, and whether the clock shows seconds.
//!
//! One JSON object per user, `$XDG_CONFIG_HOME/desicompass/bar.json`. The
//! superkey writes it (Settings > Bar, or Super+B), and the bar looks at it
//! once a second and follows. They are the user's own taste, so they are not
//! in the machine-wide accessibility object the login screen shares.
//!
//! ```json
//! { "barPosition": "bottom", "barSeconds": false }
//! ```
//!
//! A missing file, a missing key and a value this version does not know all
//! read as the default.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use serde_json::{Map, Value};

use crate::Edge;

/// Top or bottom.
pub const KEY_POSITION: &str = "barPosition";
/// Seconds on the clock.
pub const KEY_SECONDS: &str = "barSeconds";

/// Every key, in the order the superkey lists them.
pub const KEYS: &[&str] = &[KEY_POSITION, KEY_SECONDS];

/// The positions, in the order the superkey lists them.
pub const POSITIONS: &[&str] = &["bottom", "top"];

/// How often [`BarSettingsFile::poll`] looks at the file.
pub const POLL_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BarSettings {
    pub position: Edge,
    pub seconds: bool,
}

impl BarSettings {
    /// A key's value as stored: `"top"`, `"true"`.
    pub fn get(&self, key: &str) -> Option<String> {
        match key {
            KEY_POSITION => Some(self.position.as_str().to_owned()),
            KEY_SECONDS => Some(self.seconds.to_string()),
            _ => None,
        }
    }

    /// Set a key from its stored form. False, and nothing changed, for an
    /// unknown key or a value it cannot take.
    pub fn set(&mut self, key: &str, value: &str) -> bool {
        match key {
            KEY_POSITION => match Edge::parse(value) {
                Some(e) => {
                    self.position = e;
                    true
                }
                None => false,
            },
            KEY_SECONDS => match value {
                "true" => {
                    self.seconds = true;
                    true
                }
                "false" => {
                    self.seconds = false;
                    true
                }
                _ => false,
            },
            _ => false,
        }
    }

    /// Whether `key` names one of these settings.
    pub fn is_key(key: &str) -> bool {
        KEYS.contains(&key)
    }

    /// Read from the file's text, tolerating anything.
    pub fn from_json(text: &str) -> Self {
        let mut s = Self::default();
        if let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(text) {
            if let Some(Value::String(p)) = obj.get(KEY_POSITION) {
                s.set(KEY_POSITION, p);
            }
            if let Some(Value::Bool(b)) = obj.get(KEY_SECONDS) {
                s.seconds = *b;
            }
        }
        s
    }

    fn write_into(&self, obj: &mut Map<String, Value>) {
        obj.insert(
            KEY_POSITION.to_owned(),
            Value::String(self.position.as_str().to_owned()),
        );
        obj.insert(KEY_SECONDS.to_owned(), Value::Bool(self.seconds));
    }
}

/// The file's modification time and length: changed when either changed.
type Stamp = (SystemTime, u64);

fn stamp(path: &Path) -> Option<Stamp> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.modified().ok()?, m.len()))
}

/// The settings file, as the bar follows it and the superkey writes it.
#[derive(Debug)]
pub struct BarSettingsFile {
    path: Option<PathBuf>,
    current: BarSettings,
    stamp: Option<Stamp>,
    last_poll: Option<Instant>,
}

impl BarSettingsFile {
    /// The file at `path`, which does not have to exist. `None`: nowhere to
    /// keep them, and every change is refused.
    pub fn open(path: Option<PathBuf>) -> Self {
        let mut f = Self {
            path,
            current: BarSettings::default(),
            stamp: None,
            last_poll: None,
        };
        f.reload();
        f
    }

    /// The user's file: `$XDG_CONFIG_HOME/desicompass/bar.json`, or
    /// `~/.config/desicompass/bar.json`.
    pub fn open_default() -> Self {
        Self::open(default_path(
            std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
            std::env::var_os("HOME").map(PathBuf::from),
        ))
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn get(&self) -> BarSettings {
        self.current
    }

    fn reload(&mut self) {
        let Some(path) = &self.path else {
            return;
        };
        self.stamp = stamp(path);
        self.current = std::fs::read_to_string(path)
            .map(|t| BarSettings::from_json(&t))
            .unwrap_or_default();
    }

    /// Record a choice. The write is a rename, so the bar never reads half a
    /// file, and keys this version does not know are kept.
    pub fn set(&mut self, key: &str, value: &str) -> std::io::Result<()> {
        use std::io::{Error, ErrorKind};
        let mut next = self.current;
        if !next.set(key, value) {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                format!("not a bar setting: {key}={value}"),
            ));
        }
        let Some(path) = self.path.clone() else {
            return Err(Error::new(ErrorKind::NotFound, "no settings file"));
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut obj = match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<Value>(&text) {
                Ok(Value::Object(m)) => m,
                _ => Map::new(),
            },
            Err(_) => Map::new(),
        };
        next.write_into(&mut obj);
        let body = serde_json::to_string_pretty(&Value::Object(obj)).map_err(Error::other)?;
        write_atomic(&path, body.as_bytes())?;
        self.current = next;
        self.stamp = stamp(&path);
        tracing::info!("bar: saved {key}={value} to {}", path.display());
        Ok(())
    }

    /// Look at the file, at most every [`POLL_INTERVAL`]. True when the
    /// settings changed since the last look.
    pub fn poll(&mut self) -> bool {
        let now = Instant::now();
        if self
            .last_poll
            .is_some_and(|t| now.duration_since(t) < POLL_INTERVAL)
        {
            return false;
        }
        self.last_poll = Some(now);
        self.poll_now()
    }

    /// [`poll`](Self::poll) without the throttle.
    pub fn poll_now(&mut self) -> bool {
        let Some(path) = &self.path else {
            return false;
        };
        if stamp(path) == self.stamp {
            return false;
        }
        let before = self.current;
        self.reload();
        self.current != before
    }
}

/// Where the settings live, from `XDG_CONFIG_HOME` and `HOME`.
pub fn default_path(config_home: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    let base = config_home
        .filter(|p| p.is_absolute())
        .or_else(|| home.map(|h| h.join(".config")))?;
    Some(base.join("desicompass").join("bar.json"))
}

/// Write `bytes` to `path` through a temporary file beside it and a rename.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_a_bottom_bar_without_seconds() {
        let s = BarSettings::default();
        assert_eq!(s.position, Edge::Bottom);
        assert!(!s.seconds);
        assert_eq!(s.get(KEY_POSITION).as_deref(), Some("bottom"));
        assert_eq!(s.get(KEY_SECONDS).as_deref(), Some("false"));
        assert_eq!(s.get("wallpaper"), None);
    }

    #[test]
    fn only_known_keys_and_values_are_taken() {
        let mut s = BarSettings::default();
        assert!(s.set(KEY_POSITION, "top"));
        assert!(s.set(KEY_SECONDS, "true"));
        assert!(!s.set(KEY_POSITION, "left"));
        assert!(!s.set(KEY_SECONDS, "yes"));
        assert!(!s.set("wallpaper", "cats"));
        assert_eq!(
            s,
            BarSettings {
                position: Edge::Top,
                seconds: true
            }
        );
        for k in KEYS {
            assert!(BarSettings::is_key(k));
        }
        assert!(!BarSettings::is_key("fontScale"));
    }

    #[test]
    fn a_bad_file_reads_as_the_defaults() {
        for text in [
            "",
            "[]",
            "not json",
            "{\"barPosition\":\"left\",\"barSeconds\":1}",
        ] {
            assert_eq!(
                BarSettings::from_json(text),
                BarSettings::default(),
                "{text}"
            );
        }
        assert_eq!(
            BarSettings::from_json("{\"barPosition\":\"top\",\"barSeconds\":true}"),
            BarSettings {
                position: Edge::Top,
                seconds: true
            }
        );
    }

    #[test]
    fn a_change_is_written_and_read_back_keeping_unknown_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("desicompass").join("bar.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{\"future\":1}").unwrap();

        let mut f = BarSettingsFile::open(Some(path.clone()));
        f.set(KEY_POSITION, "top").unwrap();
        f.set(KEY_SECONDS, "true").unwrap();
        assert!(f.set(KEY_SECONDS, "maybe").is_err());

        let text = std::fs::read_to_string(&path).unwrap();
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["future"], 1);
        assert_eq!(v[KEY_POSITION], "top");
        assert_eq!(v[KEY_SECONDS], true);
        assert_eq!(
            BarSettingsFile::open(Some(path)).get(),
            BarSettings {
                position: Edge::Top,
                seconds: true
            }
        );
    }

    #[test]
    fn a_change_made_elsewhere_is_noticed_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bar.json");
        let mut bar = BarSettingsFile::open(Some(path.clone()));
        let mut superkey = BarSettingsFile::open(Some(path));
        assert!(!bar.poll_now());
        superkey.set(KEY_POSITION, "top").unwrap();
        assert!(bar.poll_now());
        assert_eq!(bar.get().position, Edge::Top);
        assert!(!bar.poll_now());
        // Its own write is not reported back as someone else's.
        assert!(!superkey.poll_now());
    }

    #[test]
    fn nowhere_to_keep_them_refuses_changes() {
        let mut f = BarSettingsFile::open(None);
        assert_eq!(
            f.set(KEY_POSITION, "top").unwrap_err().kind(),
            std::io::ErrorKind::NotFound
        );
        assert!(!f.poll_now());
    }

    #[test]
    fn the_file_is_under_the_config_home() {
        assert_eq!(
            default_path(Some("/c".into()), Some("/h".into())),
            Some(PathBuf::from("/c/desicompass/bar.json"))
        );
        // A relative XDG_CONFIG_HOME is invalid, per the spec, and ignored.
        assert_eq!(
            default_path(Some("rel".into()), Some("/h".into())),
            Some(PathBuf::from("/h/.config/desicompass/bar.json"))
        );
        assert_eq!(default_path(None, None), None);
    }
}
