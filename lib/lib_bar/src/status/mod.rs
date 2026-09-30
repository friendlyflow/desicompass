//! Where the bar's status comes from: one thread per source, each sending
//! [`Update`]s to the loop.
//!
//! | Source | Where | How |
//! |---|---|---|
//! | network | NetworkManager, system bus | its signals, and a look every 30 s |
//! | battery | UPower's display device, system bus | the same |
//! | Bluetooth | BlueZ, system bus | the same |
//! | audio | `wpctl` (WirePlumber) | a look every 2 s |
//! | notifications | this bar is `org.freedesktop.Notifications`, session bus | every call |
//! | tray | this bar is `org.kde.StatusNotifierWatcher`, session bus | every call and item signal |
//!
//! A source that is not there (no battery, no NetworkManager) reports `None`
//! and the bar shows no icon for it, and it keeps looking, since a service can
//! start later.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::Duration;

use zbus::blocking::Connection;
use zbus::zvariant::OwnedValue;

use crate::model::Update;

pub mod audio;
pub mod battery;
pub mod bluetooth;
pub mod network;
pub mod notifications;
pub mod tray;

/// How often a source looks again when nothing has signalled.
const REFRESH: Duration = Duration::from_secs(30);
/// How long a burst of signals is let settle before looking.
const SETTLE: Duration = Duration::from_millis(100);

/// Which bus a source is on.
#[derive(Debug, Clone, Copy)]
pub enum Bus {
    System,
    Session,
}

/// Start every source.
pub fn start(tx: Sender<Update>) {
    network::start(tx.clone());
    battery::start(tx.clone());
    bluetooth::start(tx.clone());
    audio::start(tx.clone());
    notifications::start(tx.clone());
    tray::start(tx);
}

/// A thread that says when a message matching `rule` arrives.
pub fn watch(conn: &Connection, rule: &str) -> Receiver<()> {
    let (tx, rx) = channel();
    match zbus::blocking::MessageIterator::for_match_rule(rule, conn, Some(64)) {
        Ok(messages) => {
            let _ = std::thread::Builder::new()
                .name("bar-watch".into())
                .spawn(move || {
                    for m in messages {
                        if m.is_ok() && tx.send(()).is_err() {
                            return;
                        }
                    }
                });
        }
        Err(e) => tracing::debug!("cannot watch {rule}: {e}"),
    }
    rx
}

/// Follow one source: look now, then again after every signal matching
/// `rule` and every [`REFRESH`], and send what is seen when it changed.
pub fn follow<T>(
    name: &'static str,
    bus: Bus,
    rule: &'static str,
    read: fn(&Connection) -> Option<T>,
    wrap: fn(Option<T>) -> Update,
    tx: Sender<Update>,
) where
    T: PartialEq + Clone + Send + 'static,
{
    let spawned = std::thread::Builder::new()
        .name(format!("bar-{name}"))
        .spawn(move || {
            let conn = match bus {
                Bus::System => Connection::system(),
                Bus::Session => Connection::session(),
            };
            let conn = match conn {
                Ok(c) => c,
                Err(e) => {
                    tracing::info!("{name}: no {bus:?} bus ({e}), not shown");
                    let _ = tx.send(wrap(None));
                    return;
                }
            };
            let pokes = watch(&conn, rule);
            let mut last: Option<Option<T>> = None;
            loop {
                let seen = read(&conn);
                if last.as_ref() != Some(&seen) {
                    if tx.send(wrap(seen.clone())).is_err() {
                        return;
                    }
                    last = Some(seen);
                }
                match pokes.recv_timeout(REFRESH) {
                    Ok(()) => {
                        std::thread::sleep(SETTLE);
                        while pokes.try_recv().is_ok() {}
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => std::thread::sleep(REFRESH),
                }
            }
        });
    if let Err(e) = spawned {
        tracing::warn!("{name}: could not start its thread: {e}");
    }
}

/// Every property of one interface of one object. `None` when the service,
/// the object or the interface is not there.
pub fn get_all(
    conn: &Connection,
    dest: &str,
    path: &str,
    iface: &str,
) -> Option<HashMap<String, OwnedValue>> {
    let proxy = zbus::blocking::fdo::PropertiesProxy::builder(conn)
        .destination(dest.to_owned())
        .ok()?
        .path(path.to_owned())
        .ok()?
        .build()
        .ok()?;
    let iface = zbus::names::InterfaceName::try_from(iface.to_owned()).ok()?;
    proxy.get_all(iface).ok()
}

/// A property's value as `T`, if it is there and of that type.
pub fn prop<'a, T>(props: &'a HashMap<String, OwnedValue>, name: &str) -> Option<T>
where
    T: TryFrom<&'a zbus::zvariant::Value<'a>>,
    <T as TryFrom<&'a zbus::zvariant::Value<'a>>>::Error: Into<zbus::zvariant::Error>,
{
    props.get(name)?.downcast_ref::<T>().ok()
}
