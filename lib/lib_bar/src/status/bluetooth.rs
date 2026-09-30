//! Bluetooth, from BlueZ: whether an adapter is on, and how many devices are
//! connected. Absent on a machine without an adapter.

use std::sync::mpsc::Sender;

use desicompass_bar_protocol::status::Bluetooth;
use zbus::blocking::Connection;

use super::{Bus, follow, prop};
use crate::model::Update;

pub fn start(tx: Sender<Update>) {
    follow(
        "bluetooth",
        Bus::System,
        "type='signal',sender='org.bluez'",
        read,
        Update::Bluetooth,
        tx,
    );
}

fn read(conn: &Connection) -> Option<Bluetooth> {
    let objects = zbus::blocking::fdo::ObjectManagerProxy::builder(conn)
        .destination("org.bluez")
        .ok()?
        .path("/")
        .ok()?
        .build()
        .ok()?
        .get_managed_objects()
        .ok()?;
    let mut adapters = Vec::new();
    let mut connected = 0;
    for ifaces in objects.values() {
        for (iface, props) in ifaces {
            match iface.as_str() {
                "org.bluez.Adapter1" => {
                    adapters.push(prop::<bool>(props, "Powered").unwrap_or(false))
                }
                "org.bluez.Device1" if prop::<bool>(props, "Connected") == Some(true) => {
                    connected += 1
                }
                _ => {}
            }
        }
    }
    bluetooth_from(&adapters, connected)
}

/// Whether each adapter is powered, and how many devices are connected.
pub fn bluetooth_from(adapters: &[bool], connected: u32) -> Option<Bluetooth> {
    if adapters.is_empty() {
        return None;
    }
    let powered = adapters.iter().any(|&p| p);
    Some(Bluetooth {
        powered,
        connected: if powered { connected } else { 0 },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_adapter_no_icon() {
        assert_eq!(bluetooth_from(&[], 0), None);
    }

    #[test]
    fn any_powered_adapter_is_on() {
        assert_eq!(
            bluetooth_from(&[false, true], 2),
            Some(Bluetooth {
                powered: true,
                connected: 2
            })
        );
        assert_eq!(
            bluetooth_from(&[false], 3),
            Some(Bluetooth {
                powered: false,
                connected: 0
            })
        );
    }
}
