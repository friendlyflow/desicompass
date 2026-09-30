//! The battery, from UPower's display device: the one figure a desktop shows
//! for however many batteries there are. Absent on a machine without one.

use std::sync::mpsc::Sender;

use desicompass_bar_protocol::status::{Battery, BatteryState};
use zbus::blocking::Connection;

use super::{Bus, follow, get_all, prop};
use crate::model::Update;

pub fn start(tx: Sender<Update>) {
    follow(
        "battery",
        Bus::System,
        "type='signal',sender='org.freedesktop.UPower'",
        read,
        Update::Battery,
        tx,
    );
}

fn read(conn: &Connection) -> Option<Battery> {
    let d = get_all(
        conn,
        "org.freedesktop.UPower",
        "/org/freedesktop/UPower/devices/DisplayDevice",
        "org.freedesktop.UPower.Device",
    )?;
    battery_from(
        prop::<bool>(&d, "IsPresent").unwrap_or(false),
        prop::<f64>(&d, "Percentage").unwrap_or(0.0),
        prop::<u32>(&d, "State").unwrap_or(0),
    )
}

/// UPower's figures, as the bar sees them. `state` is UPower's device state
/// (1 charging, 2 discharging, 3 empty, 4 fully charged, 5 and 6 pending).
pub fn battery_from(present: bool, percentage: f64, state: u32) -> Option<Battery> {
    if !present {
        return None;
    }
    let state = match state {
        1 => BatteryState::Charging,
        2 | 6 => BatteryState::Discharging,
        3 => BatteryState::Empty,
        4 => BatteryState::Full,
        _ => BatteryState::Unknown,
    };
    Some(Battery {
        percent: percentage.round().clamp(0.0, 100.0) as u8,
        state,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_battery_no_icon() {
        assert_eq!(battery_from(false, 80.0, 2), None);
    }

    #[test]
    fn the_percentage_and_the_state() {
        assert_eq!(
            battery_from(true, 80.6, 1),
            Some(Battery {
                percent: 81,
                state: BatteryState::Charging
            })
        );
        assert_eq!(
            battery_from(true, 100.0, 4).unwrap().state,
            BatteryState::Full
        );
        assert_eq!(
            battery_from(true, 55.0, 6).unwrap().state,
            BatteryState::Discharging
        );
        assert_eq!(battery_from(true, 140.0, 0).unwrap().percent, 100);
        assert_eq!(battery_from(true, -3.0, 0).unwrap().percent, 0);
    }
}
