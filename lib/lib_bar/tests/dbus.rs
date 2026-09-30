//! The bar's two D-Bus services, the notification server and the tray, on a
//! private bus of their own: a `dbus-daemon` started for this test, so nothing
//! reaches the session the tests run in.
//!
//! One test, run top to bottom: the services find the bus through
//! `DBUS_SESSION_BUS_ADDRESS`, which is the process's environment and so
//! cannot differ between parallel tests. Skipped when there is no
//! `dbus-daemon` to start.

#![cfg(target_os = "linux")]

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use desicompass_bar::model::Update;
use desicompass_bar::status::{notifications, tray};
use zbus::blocking::Connection;
use zbus::zvariant::OwnedValue;

struct Bus {
    daemon: Child,
    _dir: tempfile::TempDir,
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
    }
}

/// A bus of our own, or `None` without a `dbus-daemon`.
fn private_bus() -> Option<Bus> {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("bus.conf");
    std::fs::write(
        &config,
        format!(
            r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:dir={}</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"#,
            dir.path().display()
        ),
    )
    .unwrap();
    let mut daemon = match Command::new("dbus-daemon")
        .arg(format!("--config-file={}", config.display()))
        .args(["--nofork", "--nopidfile", "--print-address=1"])
        .stdout(Stdio::piped())
        .spawn()
    {
        Ok(d) => d,
        Err(e) => {
            eprintln!("no dbus-daemon ({e}): skipped");
            return None;
        }
    };
    let mut address = String::new();
    BufReader::new(daemon.stdout.take().unwrap())
        .read_line(&mut address)
        .unwrap();
    // SAFETY: set before any thread of this test binary reads it: the
    // services are started after this, and this is the binary's only test.
    unsafe { std::env::set_var("DBUS_SESSION_BUS_ADDRESS", address.trim()) };
    Some(Bus { daemon, _dir: dir })
}

/// The next update `pick` accepts, skipping others, within five seconds.
fn next<T>(rx: &Receiver<Update>, mut pick: impl FnMut(Update) -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let u = rx.recv_timeout(left).expect("the update never came");
        if let Some(v) = pick(u) {
            return v;
        }
    }
}

fn wait_for_owner(conn: &Connection, name: &'static str) {
    let dbus = zbus::blocking::fdo::DBusProxy::new(conn).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !dbus
        .name_has_owner(name.try_into().unwrap())
        .unwrap_or(false)
    {
        assert!(Instant::now() < deadline, "nobody took {name}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A tray item, the way a program with a tray icon publishes one.
struct FakeItem;

#[zbus::interface(name = "org.kde.StatusNotifierItem")]
impl FakeItem {
    #[zbus(property)]
    fn title(&self) -> String {
        "Test item".into()
    }
    #[zbus(property)]
    fn id(&self) -> String {
        "test".into()
    }
    #[zbus(property)]
    fn status(&self) -> String {
        "Active".into()
    }
    #[zbus(property)]
    fn icon_name(&self) -> String {
        String::new()
    }
    #[zbus(property)]
    fn icon_theme_path(&self) -> String {
        String::new()
    }
    #[zbus(property)]
    fn icon_pixmap(&self) -> Vec<(i32, i32, Vec<u8>)> {
        // 2x1, ARGB: opaque red, half-transparent blue.
        vec![(2, 1, vec![0xFF, 0xFF, 0, 0, 0x80, 0, 0, 0xFF])]
    }
}

#[test]
fn notifications_and_the_tray_over_a_real_bus() {
    let Some(_bus) = private_bus() else {
        return;
    };
    let (tx, rx) = channel();
    notifications::start(tx.clone());
    tray::start(tx);

    let client = Connection::session().unwrap();
    wait_for_owner(&client, "org.freedesktop.Notifications");
    wait_for_owner(&client, "org.kde.StatusNotifierWatcher");

    // ---- A notification arrives, is listed, and is dismissed ----------------
    let closed = zbus::blocking::MessageIterator::for_match_rule(
        "type='signal',interface='org.freedesktop.Notifications',member='NotificationClosed'",
        &client,
        None,
    )
    .unwrap();
    let hints: HashMap<&str, OwnedValue> = HashMap::new();
    let id: u32 = client
        .call_method(
            Some("org.freedesktop.Notifications"),
            "/org/freedesktop/Notifications",
            Some("org.freedesktop.Notifications"),
            "Notify",
            &(
                "Mail",
                0u32,
                "",
                "New message",
                "Hello",
                Vec::<&str>::new(),
                hints,
                -1i32,
            ),
        )
        .unwrap()
        .body()
        .deserialize()
        .unwrap();
    assert_ne!(id, 0);
    let listed = next(&rx, |u| match u {
        Update::Notifications(n) if !n.is_empty() => Some(n),
        _ => None,
    });
    assert_eq!(listed.len(), 1);
    assert_eq!(
        (listed[0].id, listed[0].summary.as_str()),
        (id, "New message")
    );

    client
        .call_method(
            Some("org.freedesktop.Notifications"),
            "/org/freedesktop/Notifications",
            Some("org.freedesktop.Notifications"),
            "CloseNotification",
            &(id,),
        )
        .unwrap();
    next(&rx, |u| match u {
        Update::Notifications(n) if n.is_empty() => Some(()),
        _ => None,
    });
    let signal = closed
        .into_iter()
        .next()
        .expect("NotificationClosed")
        .unwrap();
    let (closed_id, reason): (u32, u32) = signal.body().deserialize().unwrap();
    assert_eq!((closed_id, reason), (id, notifications::REASON_CLOSED));

    // ---- A tray item registers, is shown with its icon, and leaves -------------
    let item = zbus::blocking::connection::Builder::session()
        .unwrap()
        .serve_at(tray::DEFAULT_ITEM_PATH, FakeItem)
        .unwrap()
        .build()
        .unwrap();
    item.call_method(
        Some(tray::WATCHER),
        tray::WATCHER_PATH,
        Some(tray::WATCHER),
        "RegisterStatusNotifierItem",
        &(tray::DEFAULT_ITEM_PATH,),
    )
    .unwrap();
    let entries = next(&rx, |u| match u {
        Update::Tray(e) if !e.is_empty() => Some(e),
        _ => None,
    });
    assert_eq!(entries.len(), 1);
    let unique = item.unique_name().unwrap().to_string();
    assert_eq!(entries[0].item.service, unique);
    assert_eq!(entries[0].item.path, tray::DEFAULT_ITEM_PATH);
    assert_eq!(entries[0].item.title, "Test item");
    let picture = entries[0].picture.as_ref().expect("the pixmap");
    assert_eq!((picture.width, picture.height), (2, 1));
    assert_eq!(picture.rgba, [0xFF, 0, 0, 0xFF, 0, 0, 0xFF, 0x80]);

    let registered: Vec<String> = zbus::blocking::fdo::PropertiesProxy::builder(&client)
        .destination(tray::WATCHER)
        .unwrap()
        .path(tray::WATCHER_PATH)
        .unwrap()
        .build()
        .unwrap()
        .get(
            tray::WATCHER.try_into().unwrap(),
            "RegisteredStatusNotifierItems",
        )
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(registered, [format!("{unique}{}", tray::DEFAULT_ITEM_PATH)]);

    drop(item);
    next(&rx, |u| match u {
        Update::Tray(e) if e.is_empty() => Some(()),
        _ => None,
    });
}
