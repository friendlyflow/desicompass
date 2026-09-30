//! The network, from NetworkManager: connected or not, how, whether it reaches
//! the internet, and the signal of a wireless connection.

use std::sync::mpsc::Sender;

use desicompass_bar_protocol::status::{Connectivity, Network, NetworkKind};
use zbus::blocking::Connection;
use zbus::zvariant::ObjectPath;

use super::{Bus, follow, get_all, prop};
use crate::model::Update;

const NM: &str = "org.freedesktop.NetworkManager";

pub fn start(tx: Sender<Update>) {
    follow(
        "network",
        Bus::System,
        "type='signal',sender='org.freedesktop.NetworkManager'",
        read,
        Update::Network,
        tx,
    );
}

fn read(conn: &Connection) -> Option<Network> {
    let nm = get_all(conn, NM, "/org/freedesktop/NetworkManager", NM)?;
    let kind: String = prop::<&str>(&nm, "PrimaryConnectionType")
        .unwrap_or_default()
        .to_owned();
    let state = prop::<u32>(&nm, "State").unwrap_or(0);
    let connectivity = prop::<u32>(&nm, "Connectivity").unwrap_or(0);
    let strength = if kind == "802-11-wireless" {
        prop::<ObjectPath>(&nm, "PrimaryConnection")
            .and_then(|p| {
                get_all(
                    conn,
                    NM,
                    p.as_str(),
                    "org.freedesktop.NetworkManager.Connection.Active",
                )
            })
            .and_then(|a| prop::<ObjectPath>(&a, "SpecificObject").map(|p| p.to_string()))
            .and_then(|ap| get_all(conn, NM, &ap, "org.freedesktop.NetworkManager.AccessPoint"))
            .and_then(|ap| prop::<u8>(&ap, "Strength"))
    } else {
        None
    };
    Some(network_from(&kind, state, connectivity, strength))
}

/// NetworkManager's numbers, as the bar sees them.
///
/// `state` is `NMState` (70 connected globally, 60 site, 50 local, below that
/// not connected), `connectivity` is `NMConnectivityState` (1 none, 2 portal,
/// 3 limited, 4 full), and `kind` the primary connection's type.
pub fn network_from(kind: &str, state: u32, connectivity: u32, strength: Option<u8>) -> Network {
    let kind = if state < 50 {
        NetworkKind::None
    } else {
        match kind {
            "802-3-ethernet" => NetworkKind::Wired,
            "802-11-wireless" => NetworkKind::Wireless,
            "" => NetworkKind::None,
            _ => NetworkKind::Other,
        }
    };
    let connectivity = match connectivity {
        1 => Connectivity::None,
        2 | 3 => Connectivity::Limited,
        4 => Connectivity::Full,
        _ => Connectivity::Unknown,
    };
    Network {
        kind,
        connectivity,
        strength: strength.filter(|_| kind == NetworkKind::Wireless),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wireless_connection_with_its_signal() {
        assert_eq!(
            network_from("802-11-wireless", 70, 4, Some(64)),
            Network {
                kind: NetworkKind::Wireless,
                connectivity: Connectivity::Full,
                strength: Some(64)
            }
        );
    }

    #[test]
    fn a_cable_behind_a_captive_portal_is_limited() {
        let n = network_from("802-3-ethernet", 70, 2, Some(10));
        assert_eq!(n.kind, NetworkKind::Wired);
        assert_eq!(n.connectivity, Connectivity::Limited);
        assert_eq!(n.strength, None, "only wireless has a signal");
    }

    #[test]
    fn not_connected_whatever_the_last_type_was() {
        for state in [0, 10, 20, 40] {
            assert_eq!(
                network_from("802-11-wireless", state, 1, Some(80)).kind,
                NetworkKind::None
            );
        }
        assert_eq!(network_from("", 70, 4, None).kind, NetworkKind::None);
        assert_eq!(network_from("vpn", 70, 4, None).kind, NetworkKind::Other);
        assert_eq!(
            network_from("gsm", 60, 0, None).connectivity,
            Connectivity::Unknown
        );
    }
}
