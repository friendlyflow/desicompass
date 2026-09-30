//! The tray: this bar is the session's `org.kde.StatusNotifierWatcher`, and
//! the host that shows what registers with it.
//!
//! A program with a tray icon (Dropbox, a chat client) publishes a
//! `StatusNotifierItem` on the session bus and registers it here. The bar shows
//! its icon, and the superkey's Status section lists it by title, where Enter
//! calls the item's `Activate`. Menus (`com.canonical.dbusmenu`) are not
//! shown.
//!
//! Only programs on this session's bus can register, which in a desicompass
//! session is the private bus `dbus-run-session` started: a program started
//! inside the session (from sicompass or the superkey) is on it, a systemd
//! user service is not.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use desicompass_bar_protocol::status::TrayItem;
use zbus::blocking::Connection;
use zbus::message::Header;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{OwnedValue, Value};

use super::{get_all, prop};
use crate::model::{Picture, TrayEntry, Update};

pub const WATCHER: &str = "org.kde.StatusNotifierWatcher";
pub const WATCHER_PATH: &str = "/StatusNotifierWatcher";
pub const ITEM_IFACE: &str = "org.kde.StatusNotifierItem";
/// Where an item is when it registers by bus name alone.
pub const DEFAULT_ITEM_PATH: &str = "/StatusNotifierItem";

/// The largest icon the bar keeps: more than a line at any font scale.
const MAX_ICON: i32 = 96;

/// One registered item: where it is, and who owns its bus name.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Registered {
    bus: String,
    path: String,
    /// The unique name behind `bus`, which its signals come from.
    owner: String,
}

impl Registered {
    /// How the watcher's `RegisteredStatusNotifierItems` names it.
    fn id(&self) -> String {
        format!("{}{}", self.bus, self.path)
    }
}

/// What a registration names: `service` is a bus name, or an object path on
/// the caller's own connection.
pub fn item_address(service: &str, sender: &str) -> (String, String) {
    if service.starts_with('/') {
        (sender.to_owned(), service.to_owned())
    } else {
        (service.to_owned(), DEFAULT_ITEM_PATH.to_owned())
    }
}

enum Event {
    Registered(Registered),
    /// A signal from an item's owner: look at its items again.
    Changed(String),
    /// A bus name went away.
    Gone(String),
}

struct Watcher {
    items: Arc<Mutex<Vec<Registered>>>,
    events: Sender<Event>,
}

#[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
impl Watcher {
    async fn register_status_notifier_item(
        &self,
        service: String,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        let sender = header.sender().map(|s| s.to_string()).unwrap_or_default();
        let (bus, path) = item_address(&service, &sender);
        let owner = if bus.starts_with(':') {
            bus.clone()
        } else {
            let dbus = zbus::fdo::DBusProxy::new(conn).await?;
            let name = zbus::names::BusName::try_from(bus.as_str())
                .map_err(|e| zbus::fdo::Error::InvalidArgs(e.to_string()))?;
            dbus.get_name_owner(name).await?.to_string()
        };
        let item = Registered { bus, path, owner };
        let id = item.id();
        {
            let mut items = self.items.lock().unwrap_or_else(|e| e.into_inner());
            if items.iter().any(|i| i.id() == id) {
                return Ok(());
            }
            items.push(item.clone());
        }
        tracing::info!("tray: {id} registered");
        let _ = self.events.send(Event::Registered(item));
        let _ = Self::status_notifier_item_registered(&emitter, &id).await;
        Ok(())
    }

    fn register_status_notifier_host(&self, _service: String) {}

    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> Vec<String> {
        self.items
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(Registered::id)
            .collect()
    }

    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn protocol_version(&self) -> i32 {
        0
    }

    #[zbus(signal)]
    async fn status_notifier_item_registered(
        emitter: &SignalEmitter<'_>,
        service: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_item_unregistered(
        emitter: &SignalEmitter<'_>,
        service: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_host_registered(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

/// Be the watcher and the host, from threads of their own.
pub fn start(tx: Sender<Update>) {
    let spawned = std::thread::Builder::new()
        .name("bar-tray".into())
        .spawn(move || run(tx));
    if let Err(e) = spawned {
        tracing::warn!("tray: could not start its thread: {e}");
    }
}

fn run(tx: Sender<Update>) {
    let _ = tx.send(Update::Tray(Vec::new()));
    let items = Arc::new(Mutex::new(Vec::new()));
    let (events, rx) = channel();
    let watcher = Watcher {
        items: Arc::clone(&items),
        events: events.clone(),
    };
    let conn = match zbus::blocking::connection::Builder::session()
        .and_then(|b| b.serve_at(WATCHER_PATH, watcher))
        .and_then(|b| b.build())
    {
        Ok(conn) => conn,
        Err(e) => {
            tracing::warn!("no session bus for the tray: {e}");
            return;
        }
    };
    match conn.request_name_with_flags(WATCHER, zbus::fdo::RequestNameFlags::DoNotQueue.into()) {
        Ok(zbus::fdo::RequestNameReply::PrimaryOwner) => tracing::info!("serving {WATCHER}"),
        Ok(reply) => {
            tracing::info!("{WATCHER} is taken ({reply:?}); another tray has the items");
            return;
        }
        Err(zbus::Error::NameTaken) => {
            tracing::info!("{WATCHER} is taken; another tray has the items");
            return;
        }
        Err(e) => {
            tracing::warn!("could not take {WATCHER}: {e}");
            return;
        }
    }
    // Some programs wait for a host before they register.
    let _ = conn.request_name(format!("org.kde.StatusNotifierHost-{}", std::process::id()));
    let _ = conn.emit_signal(
        None::<&str>,
        WATCHER_PATH,
        WATCHER,
        "StatusNotifierHostRegistered",
        &(),
    );

    forward_item_signals(&conn, events.clone());
    forward_departures(&conn, events);
    host(&conn, &items, rx, &tx);
}

/// Every signal an item sends means something about it changed.
fn forward_item_signals(conn: &Connection, events: Sender<Event>) {
    let rule = format!("type='signal',interface='{ITEM_IFACE}'");
    let Ok(messages) =
        zbus::blocking::MessageIterator::for_match_rule(rule.as_str(), conn, Some(64))
    else {
        return;
    };
    let _ = std::thread::Builder::new()
        .name("bar-tray-items".into())
        .spawn(move || {
            for m in messages.flatten() {
                if let Some(sender) = m.header().sender()
                    && events.send(Event::Changed(sender.to_string())).is_err()
                {
                    return;
                }
            }
        });
}

/// A program that leaves the bus takes its items with it.
fn forward_departures(conn: &Connection, events: Sender<Event>) {
    let Ok(dbus) = zbus::blocking::fdo::DBusProxy::new(conn) else {
        return;
    };
    let Ok(changes) = dbus.receive_name_owner_changed() else {
        return;
    };
    let _ = std::thread::Builder::new()
        .name("bar-tray-owners".into())
        .spawn(move || {
            for change in changes {
                let Ok(args) = change.args() else {
                    continue;
                };
                if args.new_owner().is_none()
                    && events.send(Event::Gone(args.name().to_string())).is_err()
                {
                    return;
                }
            }
        });
}

/// Keep the entries current and send them whenever they change.
fn host(
    conn: &Connection,
    items: &Mutex<Vec<Registered>>,
    rx: Receiver<Event>,
    tx: &Sender<Update>,
) {
    let mut shown: HashMap<String, Option<TrayEntry>> = HashMap::new();
    let mut last: Option<Vec<TrayEntry>> = None;
    for event in rx {
        let registered = items.lock().unwrap_or_else(|e| e.into_inner()).clone();
        match event {
            Event::Registered(item) => {
                shown.insert(item.id(), read_item(conn, &item));
            }
            Event::Changed(sender) => {
                for item in registered.iter().filter(|i| i.owner == sender) {
                    shown.insert(item.id(), read_item(conn, item));
                }
            }
            Event::Gone(name) => {
                let gone: Vec<Registered> = registered
                    .iter()
                    .filter(|i| i.owner == name || i.bus == name)
                    .cloned()
                    .collect();
                if gone.is_empty() {
                    continue;
                }
                items
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .retain(|i| !gone.contains(i));
                for item in gone {
                    let id = item.id();
                    tracing::info!("tray: {id} left");
                    shown.remove(&id);
                    let _ = conn.emit_signal(
                        None::<&str>,
                        WATCHER_PATH,
                        WATCHER,
                        "StatusNotifierItemUnregistered",
                        &(id.as_str(),),
                    );
                }
            }
        }
        // In the order they registered.
        let entries: Vec<TrayEntry> = items
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter_map(|i| shown.get(&i.id()).cloned().flatten())
            .collect();
        if last.as_ref() != Some(&entries) {
            if tx.send(Update::Tray(entries.clone())).is_err() {
                return;
            }
            last = Some(entries);
        }
    }
}

/// An item as the bar shows it. `None` for one that asks not to be shown
/// (`Passive`) or cannot be read.
fn read_item(conn: &Connection, item: &Registered) -> Option<TrayEntry> {
    let props = get_all(conn, &item.bus, &item.path, ITEM_IFACE)?;
    let status = prop::<&str>(&props, "Status")
        .unwrap_or("Active")
        .to_owned();
    if status == "Passive" {
        return None;
    }
    let attention = status == "NeedsAttention";
    let text = |name: &str| prop::<&str>(&props, name).unwrap_or_default().to_owned();
    let title = [text("Title"), text("Id"), item.bus.clone()]
        .into_iter()
        .find(|t| !t.trim().is_empty())
        .unwrap_or_default();
    let (pixmap, name) = if attention {
        ("AttentionIconPixmap", "AttentionIconName")
    } else {
        ("IconPixmap", "IconName")
    };
    let theme_path = text("IconThemePath");
    let picture = pixmaps(props.get(pixmap))
        .and_then(|p| best_pixmap(&p))
        .or_else(|| find_icon(&text(name), &theme_path));
    Some(TrayEntry {
        item: TrayItem {
            service: item.bus.clone(),
            path: item.path.clone(),
            title,
        },
        picture,
    })
}

/// An `a(iiay)` property: width, height, ARGB32 pixels.
fn pixmaps(v: Option<&OwnedValue>) -> Option<Vec<(i32, i32, Vec<u8>)>> {
    let v: Value<'static> = v?.try_clone().ok()?.into();
    Vec::<(i32, i32, Vec<u8>)>::try_from(v).ok()
}

/// The largest pixmap no bigger than [`MAX_ICON`] (or the smallest there
/// is), as RGBA.
pub fn best_pixmap(pixmaps: &[(i32, i32, Vec<u8>)]) -> Option<Picture> {
    let valid = pixmaps
        .iter()
        .filter(|(w, h, px)| *w > 0 && *h > 0 && px.len() == (*w as usize) * (*h as usize) * 4);
    let (w, h, px) = valid
        .clone()
        .filter(|(w, h, _)| *w.max(h) <= MAX_ICON)
        .max_by_key(|(w, h, _)| w * h)
        .or_else(|| valid.min_by_key(|(w, h, _)| w * h))?;
    Some(Picture {
        width: *w as u32,
        height: *h as u32,
        rgba: argb_to_rgba(px),
    })
}

/// ARGB32 in network byte order (the tray's wire format) to RGBA.
pub fn argb_to_rgba(argb: &[u8]) -> Vec<u8> {
    argb.as_chunks::<4>()
        .0
        .iter()
        .flat_map(|&[a, r, g, b]| [r, g, b, a])
        .collect()
}

/// A named icon, as a PNG found in the item's own theme directory, then in
/// the hicolor theme of each data directory, then in `pixmaps`. SVG icons are
/// not drawn: the item then shows as its first letter.
fn find_icon(name: &str, theme_path: &str) -> Option<Picture> {
    if name.is_empty() {
        return None;
    }
    // A full path is allowed too.
    if name.starts_with('/') {
        return load_png(Path::new(name));
    }
    let mut roots = Vec::new();
    if !theme_path.is_empty() {
        roots.push(PathBuf::from(theme_path));
    }
    roots.extend(
        data_dirs()
            .into_iter()
            .map(|d| d.join("icons").join("hicolor")),
    );
    for root in roots {
        if let Some(p) = find_in(&root, &format!("{name}.png"), 3) {
            return load_png(&p);
        }
    }
    data_dirs()
        .into_iter()
        .map(|d| d.join("pixmaps").join(format!("{name}.png")))
        .find(|p| p.is_file())
        .and_then(|p| load_png(&p))
}

/// The data directories, most important first.
fn data_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    match std::env::var_os("XDG_DATA_HOME") {
        Some(h) => dirs.push(PathBuf::from(h)),
        None => {
            if let Some(home) = std::env::var_os("HOME") {
                dirs.push(PathBuf::from(home).join(".local/share"));
            }
        }
    }
    let system =
        std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".to_owned());
    dirs.extend(
        system
            .split(':')
            .filter(|d| !d.is_empty())
            .map(PathBuf::from),
    );
    dirs
}

/// `file` in `root` or up to `depth` directories below it, from the largest
/// size directory (`48x48` beats `16x16`), which sorts last by its number.
pub fn find_in(root: &Path, file: &str, depth: usize) -> Option<PathBuf> {
    let direct = root.join(file);
    if direct.is_file() {
        return Some(direct);
    }
    if depth == 0 {
        return None;
    }
    let mut subdirs: Vec<PathBuf> = std::fs::read_dir(root)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    subdirs.sort_by_key(|p| std::cmp::Reverse(size_of_dir(p)));
    subdirs.iter().find_map(|d| find_in(d, file, depth - 1))
}

/// `48` for `48x48` or `48x48@2`, 0 for anything else (`scalable`, `status`).
fn size_of_dir(p: &Path) -> u32 {
    p.file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.split('x').next())
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

fn load_png(path: &Path) -> Option<Picture> {
    let img = image::open(path).ok()?.to_rgba8();
    Some(Picture {
        width: img.width(),
        height: img.height(),
        rgba: img.into_raw(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_registration_names_a_bus_or_a_path() {
        assert_eq!(
            item_address("org.kde.StatusNotifierItem-12-1", ":1.5"),
            (
                "org.kde.StatusNotifierItem-12-1".to_owned(),
                DEFAULT_ITEM_PATH.to_owned()
            )
        );
        // libappindicator registers its object path.
        assert_eq!(
            item_address("/org/ayatana/NotificationItem/dropbox", ":1.5"),
            (
                ":1.5".to_owned(),
                "/org/ayatana/NotificationItem/dropbox".to_owned()
            )
        );
    }

    #[test]
    fn argb_becomes_rgba() {
        assert_eq!(
            argb_to_rgba(&[0xFF, 1, 2, 3, 0x80, 4, 5, 6]),
            [1, 2, 3, 0xFF, 4, 5, 6, 0x80]
        );
    }

    #[test]
    fn the_largest_pixmap_that_fits_is_taken() {
        let px = |s: i32| (s, s, vec![0u8; (s * s * 4) as usize]);
        let got = best_pixmap(&[px(16), px(48), px(256)]).unwrap();
        assert_eq!((got.width, got.height), (48, 48));
        // Only too big: the smallest of those.
        let got = best_pixmap(&[px(512), px(128)]).unwrap();
        assert_eq!(got.width, 128);
        // A pixmap whose bytes do not add up is skipped.
        assert!(best_pixmap(&[(4, 4, vec![0; 3])]).is_none());
        assert!(best_pixmap(&[]).is_none());
    }

    #[test]
    fn a_named_icon_is_found_in_the_largest_size_directory() {
        let dir = tempfile::tempdir().unwrap();
        for size in ["16x16", "48x48", "scalable"] {
            let d = dir.path().join(size).join("status");
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("dropbox-idle.png"), size).unwrap();
        }
        let found = find_in(dir.path(), "dropbox-idle.png", 3).unwrap();
        assert!(found.starts_with(dir.path().join("48x48")), "{found:?}");
        assert!(find_in(dir.path(), "nothing.png", 3).is_none());
        assert!(
            find_in(dir.path(), "dropbox-idle.png", 1).is_none(),
            "too deep"
        );
    }

    #[test]
    fn a_png_on_disk_is_read_as_rgba() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("i.png");
        let bytes = crate::icons::png(2, 1, vec![1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
        std::fs::write(&path, bytes).unwrap();
        let p = load_png(&path).unwrap();
        assert_eq!(
            (p.width, p.height, p.rgba),
            (2, 1, vec![1, 2, 3, 4, 5, 6, 7, 8])
        );
        assert!(
            find_icon(path.to_str().unwrap(), "").is_some(),
            "a full path"
        );
        assert!(find_icon("", "").is_none());
    }
}
