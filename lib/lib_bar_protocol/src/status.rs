//! What the bar shows, as a file the superkey can read.
//!
//! The bar is where the session's status is gathered: the network, audio,
//! battery and Bluetooth state from their services, the notifications (the bar
//! is the notification server) and the tray items (the bar is the tray). It
//! writes all of it to `$XDG_RUNTIME_DIR/desicompass/status-<WAYLAND_DISPLAY>.json`
//! on every change, and the superkey's Status section reads it back, which is
//! how a screen reader user reaches what the icons show.
//!
//! `None` for a service means it is not there (no battery, no Bluetooth
//! adapter, no NetworkManager): the bar shows no icon for it.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};

/// How often [`StatusFile::poll`] looks at the file.
pub const POLL_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusSnapshot {
    #[serde(default)]
    pub network: Option<Network>,
    #[serde(default)]
    pub audio: Option<Audio>,
    #[serde(default)]
    pub battery: Option<Battery>,
    #[serde(default)]
    pub bluetooth: Option<Bluetooth>,
    /// Oldest first.
    #[serde(default)]
    pub notifications: Vec<Notification>,
    #[serde(default)]
    pub tray: Vec<TrayItem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NetworkKind {
    Wired,
    Wireless,
    /// Connected some other way: a VPN, a phone, Bluetooth.
    Other,
    /// Not connected.
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Connectivity {
    Unknown,
    None,
    /// Behind a captive portal, or reaching the network but not the internet.
    Limited,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Network {
    pub kind: NetworkKind,
    pub connectivity: Connectivity,
    /// The access point's signal, 0 to 100, for a wireless connection.
    #[serde(default)]
    pub strength: Option<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Audio {
    /// Percent, above 100 when boosted.
    pub volume: u16,
    pub muted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BatteryState {
    Charging,
    Discharging,
    Full,
    Empty,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Battery {
    pub percent: u8,
    pub state: BatteryState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bluetooth {
    pub powered: bool,
    /// Devices connected right now.
    pub connected: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notification {
    /// The id the sender got back, which `CloseNotification` takes.
    pub id: u32,
    pub app: String,
    pub summary: String,
    #[serde(default)]
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrayItem {
    /// The item's bus name, and its object path on it: where `Activate` goes.
    pub service: String,
    pub path: String,
    pub title: String,
}

/// Where the status of the session on `wayland_display` is kept, from
/// `XDG_RUNTIME_DIR`. `None` without a runtime directory.
pub fn path_for(runtime_dir: Option<PathBuf>, wayland_display: Option<&str>) -> Option<PathBuf> {
    let dir = runtime_dir.filter(|p| p.is_absolute())?;
    // One file per session: a nested desicompass must not overwrite the
    // status of the one it runs in.
    let display: String = wayland_display
        .unwrap_or("wayland-0")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    Some(
        dir.join("desicompass")
            .join(format!("status-{display}.json")),
    )
}

/// [`path_for`] from this process's environment.
pub fn default_path() -> Option<PathBuf> {
    path_for(
        std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from),
        std::env::var("WAYLAND_DISPLAY").ok().as_deref(),
    )
}

/// Write the snapshot, atomically, creating the directory.
pub fn write(path: &Path, snapshot: &StatusSnapshot) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let body = serde_json::to_vec(snapshot).map_err(std::io::Error::other)?;
    crate::settings::write_atomic(path, &body)
}

/// Read a snapshot. `None` when there is no file or it does not parse.
pub fn read(path: &Path) -> Option<StatusSnapshot> {
    let text = std::fs::read(path).ok()?;
    serde_json::from_slice(&text).ok()
}

/// The status file as the superkey follows it.
#[derive(Debug)]
pub struct StatusFile {
    path: Option<PathBuf>,
    current: StatusSnapshot,
    stamp: Option<(SystemTime, u64)>,
    last_poll: Option<Instant>,
}

impl StatusFile {
    pub fn open(path: Option<PathBuf>) -> Self {
        let mut f = Self {
            path,
            current: StatusSnapshot::default(),
            stamp: None,
            last_poll: None,
        };
        f.poll_now();
        f
    }

    pub fn get(&self) -> &StatusSnapshot {
        &self.current
    }

    /// Look at the file, at most every [`POLL_INTERVAL`]. True when the
    /// status changed since the last look.
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
        let stamp = std::fs::metadata(path)
            .ok()
            .and_then(|m| Some((m.modified().ok()?, m.len())));
        if stamp == self.stamp {
            return false;
        }
        self.stamp = stamp;
        // Gone (the bar is restarting) reads as nothing known.
        let next = read(path).unwrap_or_default();
        if next == self.current {
            return false;
        }
        self.current = next;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> StatusSnapshot {
        StatusSnapshot {
            network: Some(Network {
                kind: NetworkKind::Wireless,
                connectivity: Connectivity::Full,
                strength: Some(72),
            }),
            audio: Some(Audio {
                volume: 45,
                muted: false,
            }),
            battery: Some(Battery {
                percent: 81,
                state: BatteryState::Discharging,
            }),
            bluetooth: Some(Bluetooth {
                powered: true,
                connected: 1,
            }),
            notifications: vec![Notification {
                id: 7,
                app: "Mail".into(),
                summary: "New message".into(),
                body: "Hi".into(),
            }],
            tray: vec![TrayItem {
                service: ":1.42".into(),
                path: "/StatusNotifierItem".into(),
                title: "Dropbox".into(),
            }],
        }
    }

    #[test]
    fn a_snapshot_round_trips_through_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("desicompass").join("status-wayland-1.json");
        write(&path, &sample()).unwrap();
        assert_eq!(read(&path), Some(sample()));
    }

    #[test]
    fn missing_fields_read_as_unknown() {
        let s: StatusSnapshot = serde_json::from_str("{}").unwrap();
        assert_eq!(s, StatusSnapshot::default());
    }

    #[test]
    fn the_reader_notices_a_change_once_and_a_removal_as_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("status.json");
        let mut f = StatusFile::open(Some(path.clone()));
        assert_eq!(f.get(), &StatusSnapshot::default());
        write(&path, &sample()).unwrap();
        assert!(f.poll_now());
        assert_eq!(f.get(), &sample());
        assert!(!f.poll_now());
        std::fs::remove_file(&path).unwrap();
        assert!(f.poll_now());
        assert_eq!(f.get(), &StatusSnapshot::default());
    }

    #[test]
    fn one_file_per_session_under_the_runtime_dir() {
        assert_eq!(
            path_for(Some("/run/user/1000".into()), Some("wayland-1")),
            Some(PathBuf::from(
                "/run/user/1000/desicompass/status-wayland-1.json"
            ))
        );
        // A display name is a file name here, never a path.
        assert_eq!(
            path_for(Some("/r".into()), Some("../x/y")),
            Some(PathBuf::from("/r/desicompass/status-___x_y.json"))
        );
        assert_eq!(path_for(None, Some("wayland-1")), None);
        assert_eq!(path_for(Some("rel".into()), None), None);
    }
}
