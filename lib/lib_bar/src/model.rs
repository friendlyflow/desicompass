//! What the bar knows, and what it shows for it.
//!
//! The status threads send [`Update`]s. The loop folds them into a [`Model`],
//! writes its snapshot for the superkey when it changed, and draws
//! [`items`]: which icon, and which amount after it, nearest the clock first.

use desicompass_bar_protocol::status::{
    Audio, Battery, BatteryState, Bluetooth, Connectivity, Network, NetworkKind, Notification,
    StatusSnapshot, TrayItem,
};

use crate::icons::Icon;

/// A picture someone else drew: a tray item's icon, as RGBA pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// A tray item and what it looks like.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayEntry {
    pub item: TrayItem,
    pub picture: Option<Picture>,
}

/// News from one of the status threads. `None`: that service is not there.
#[derive(Debug, Clone, PartialEq)]
pub enum Update {
    Network(Option<Network>),
    Audio(Option<Audio>),
    Battery(Option<Battery>),
    Bluetooth(Option<Bluetooth>),
    Notifications(Vec<Notification>),
    Tray(Vec<TrayEntry>),
}

#[derive(Debug, Default)]
pub struct Model {
    /// What the superkey is told.
    pub snapshot: StatusSnapshot,
    /// The tray again, with the pictures, which the superkey has no use for.
    pub tray: Vec<TrayEntry>,
}

impl Model {
    /// Take in an update. True when anything changed.
    pub fn apply(&mut self, update: Update) -> bool {
        fn set<T: PartialEq>(slot: &mut T, v: T) -> bool {
            if *slot == v {
                return false;
            }
            *slot = v;
            true
        }
        let s = &mut self.snapshot;
        match update {
            Update::Network(v) => set(&mut s.network, v),
            Update::Audio(v) => set(&mut s.audio, v),
            Update::Battery(v) => set(&mut s.battery, v),
            Update::Bluetooth(v) => set(&mut s.bluetooth, v),
            Update::Notifications(v) => set(&mut s.notifications, v),
            Update::Tray(v) => {
                let items: Vec<TrayItem> = v.iter().map(|e| e.item.clone()).collect();
                let listed = set(&mut s.tray, items);
                set(&mut self.tray, v) || listed
            }
        }
    }
}

/// What one place in the bar shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Glyph {
    Icon(Icon),
    /// The picture of the tray entry at this index.
    Picture(usize),
    /// A tray item without a usable picture: the first letter of its title.
    Letter(char),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub glyph: Glyph,
    /// An amount after the icon: `81%`, `2`.
    pub text: Option<String>,
}

fn item(glyph: Glyph, text: Option<String>) -> Item {
    Item { glyph, text }
}

/// Signal strength as bars, 0 to 3.
pub fn wifi_bars(strength: Option<u8>) -> u8 {
    match strength.unwrap_or(100) {
        75.. => 3,
        50..=74 => 2,
        25..=49 => 1,
        _ => 0,
    }
}

/// A battery's fill, 0 to 4, where only a nearly empty one shows empty.
pub fn battery_level(percent: u8) -> u8 {
    ((f32::from(percent.min(100)) / 25.0).round() as u8).min(4)
}

/// Loudness, 0 to 2.
pub fn volume_level(volume: u16) -> u8 {
    match volume {
        0 => 0,
        1..=49 => 1,
        _ => 2,
    }
}

/// Everything the bar shows, nearest the clock first: notifications, battery,
/// audio, Bluetooth, network, then the tray.
pub fn items(model: &Model) -> Vec<Item> {
    let s = &model.snapshot;
    let mut out = Vec::new();

    let n = s.notifications.len();
    out.push(item(
        Glyph::Icon(Icon::Bell { unread: n > 0 }),
        (n > 0).then(|| n.to_string()),
    ));

    if let Some(b) = s.battery {
        out.push(item(
            Glyph::Icon(Icon::Battery {
                level: battery_level(b.percent),
                charging: b.state == BatteryState::Charging,
            }),
            Some(format!("{}%", b.percent)),
        ));
    }

    if let Some(a) = s.audio {
        out.push(if a.muted {
            item(Glyph::Icon(Icon::VolumeMuted), None)
        } else {
            item(
                Glyph::Icon(Icon::Volume {
                    level: volume_level(a.volume),
                }),
                Some(format!("{}%", a.volume)),
            )
        });
    }

    if let Some(b) = s.bluetooth {
        out.push(if !b.powered {
            item(Glyph::Icon(Icon::BluetoothOff), None)
        } else {
            item(
                Glyph::Icon(Icon::Bluetooth {
                    connected: b.connected > 0,
                }),
                (b.connected > 1).then(|| b.connected.to_string()),
            )
        });
    }

    if let Some(net) = s.network {
        let limited = matches!(net.connectivity, Connectivity::Limited | Connectivity::None);
        let icon = match net.kind {
            NetworkKind::None => Icon::NetworkOff,
            NetworkKind::Wireless => Icon::Wifi {
                bars: wifi_bars(net.strength),
                limited,
            },
            NetworkKind::Wired | NetworkKind::Other => Icon::Wired { limited },
        };
        out.push(item(Glyph::Icon(icon), None));
    }

    for (i, e) in model.tray.iter().enumerate() {
        let glyph = match &e.picture {
            Some(_) => Glyph::Picture(i),
            None => Glyph::Letter(
                e.item
                    .title
                    .chars()
                    .find(|c| c.is_alphanumeric())
                    .map(|c| c.to_uppercase().next().unwrap_or(c))
                    .unwrap_or('?'),
            ),
        };
        out.push(item(glyph, None));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tray(title: &str, picture: bool) -> TrayEntry {
        TrayEntry {
            item: TrayItem {
                service: ":1.9".into(),
                path: "/StatusNotifierItem".into(),
                title: title.into(),
            },
            picture: picture.then(|| Picture {
                width: 1,
                height: 1,
                rgba: vec![0; 4],
            }),
        }
    }

    #[test]
    fn nothing_known_is_just_an_empty_bell() {
        assert_eq!(
            items(&Model::default()),
            vec![item(Glyph::Icon(Icon::Bell { unread: false }), None)]
        );
    }

    #[test]
    fn everything_in_order_with_its_amount() {
        let mut m = Model::default();
        m.apply(Update::Notifications(vec![
            Notification {
                id: 1,
                app: "a".into(),
                summary: "s".into(),
                body: String::new(),
            };
            2
        ]));
        m.apply(Update::Battery(Some(Battery {
            percent: 81,
            state: BatteryState::Charging,
        })));
        m.apply(Update::Audio(Some(Audio {
            volume: 45,
            muted: false,
        })));
        m.apply(Update::Bluetooth(Some(Bluetooth {
            powered: true,
            connected: 1,
        })));
        m.apply(Update::Network(Some(Network {
            kind: NetworkKind::Wireless,
            connectivity: Connectivity::Full,
            strength: Some(60),
        })));
        m.apply(Update::Tray(vec![
            tray("dropbox", true),
            tray("  x", false),
        ]));
        assert_eq!(
            items(&m),
            vec![
                item(Glyph::Icon(Icon::Bell { unread: true }), Some("2".into())),
                item(
                    Glyph::Icon(Icon::Battery {
                        level: 3,
                        charging: true
                    }),
                    Some("81%".into())
                ),
                item(Glyph::Icon(Icon::Volume { level: 1 }), Some("45%".into())),
                item(Glyph::Icon(Icon::Bluetooth { connected: true }), None),
                item(
                    Glyph::Icon(Icon::Wifi {
                        bars: 2,
                        limited: false
                    }),
                    None
                ),
                item(Glyph::Picture(0), None),
                item(Glyph::Letter('X'), None),
            ]
        );
    }

    #[test]
    fn bad_states_have_their_own_icons() {
        let mut m = Model::default();
        m.apply(Update::Audio(Some(Audio {
            volume: 45,
            muted: true,
        })));
        m.apply(Update::Bluetooth(Some(Bluetooth {
            powered: false,
            connected: 0,
        })));
        m.apply(Update::Network(Some(Network {
            kind: NetworkKind::Wired,
            connectivity: Connectivity::Limited,
            strength: None,
        })));
        let got = items(&m);
        assert_eq!(got[1], item(Glyph::Icon(Icon::VolumeMuted), None));
        assert_eq!(got[2], item(Glyph::Icon(Icon::BluetoothOff), None));
        assert_eq!(
            got[3],
            item(Glyph::Icon(Icon::Wired { limited: true }), None)
        );
        m.apply(Update::Network(Some(Network {
            kind: NetworkKind::None,
            connectivity: Connectivity::None,
            strength: None,
        })));
        assert_eq!(items(&m)[3], item(Glyph::Icon(Icon::NetworkOff), None));
    }

    #[test]
    fn an_update_reports_a_change_once() {
        let mut m = Model::default();
        let u = Update::Battery(Some(Battery {
            percent: 50,
            state: BatteryState::Discharging,
        }));
        assert!(m.apply(u.clone()));
        assert!(!m.apply(u));
        assert!(m.apply(Update::Tray(vec![tray("a", false)])));
        assert_eq!(m.snapshot.tray.len(), 1);
        // A new picture alone is a change for the bar.
        assert!(m.apply(Update::Tray(vec![tray("a", true)])));
    }

    #[test]
    fn levels() {
        assert_eq!(
            [0, 10, 13, 50, 88, 100].map(battery_level),
            [0, 0, 1, 2, 4, 4]
        );
        assert_eq!([0, 1, 49, 50, 150].map(volume_level), [0, 1, 1, 2, 2]);
        assert_eq!(
            [Some(0), Some(30), Some(60), Some(90), None].map(wifi_bars),
            [0, 1, 2, 3, 3]
        );
    }
}
