//! The session's notification server, `org.freedesktop.Notifications`.
//!
//! Nothing pops up. A notification is kept until it is dismissed: the bar
//! shows how many there are, and the superkey's Status section lists them and
//! dismisses one with the standard `CloseNotification` call, which answers
//! the sender with `NotificationClosed`. That makes this a notification
//! centre, so `expire_timeout` (how long a popup stays up) has nothing to
//! act on, and a notification marked `transient` (not worth keeping) is
//! answered but not kept.
//!
//! Only the first server on a bus gets the name: nested in another desktop,
//! that desktop's server keeps it and the bar shows none.

use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use desicompass_bar_protocol::status::Notification;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::OwnedValue;

use crate::model::Update;

pub const NAME: &str = "org.freedesktop.Notifications";
pub const PATH: &str = "/org/freedesktop/Notifications";

/// The most kept at once. Past it the oldest goes, as if it had expired.
pub const MAX_KEPT: usize = 100;

/// `NotificationClosed` reasons, from the specification.
pub const REASON_EXPIRED: u32 = 1;
pub const REASON_CLOSED: u32 = 3;

/// The notifications, oldest first.
#[derive(Debug, Default)]
pub struct Store {
    last_id: u32,
    items: Vec<Notification>,
}

impl Store {
    /// Take in a notification. Returns its id, and the id of one pushed out to
    /// make room.
    pub fn notify(
        &mut self,
        app: &str,
        replaces_id: u32,
        summary: &str,
        body: &str,
        transient: bool,
    ) -> (u32, Option<u32>) {
        if replaces_id != 0
            && let Some(n) = self.items.iter_mut().find(|n| n.id == replaces_id)
        {
            n.app = app.to_owned();
            n.summary = summary.to_owned();
            n.body = body.to_owned();
            return (replaces_id, None);
        }
        let id = self.next_id();
        if transient {
            return (id, None);
        }
        self.items.push(Notification {
            id,
            app: app.to_owned(),
            summary: summary.to_owned(),
            body: body.to_owned(),
        });
        let dropped = (self.items.len() > MAX_KEPT).then(|| self.items.remove(0).id);
        (id, dropped)
    }

    fn next_id(&mut self) -> u32 {
        // Never 0, which means "none" in `replaces_id`.
        self.last_id = self.last_id.wrapping_add(1).max(1);
        self.last_id
    }

    /// Dismiss one. False when there was no such notification.
    pub fn close(&mut self, id: u32) -> bool {
        let before = self.items.len();
        self.items.retain(|n| n.id != id);
        self.items.len() != before
    }

    pub fn list(&self) -> Vec<Notification> {
        self.items.clone()
    }
}

struct Server {
    store: Arc<Mutex<Store>>,
    tx: Sender<Update>,
}

impl Server {
    fn publish(&self, store: &Store) {
        let _ = self.tx.send(Update::Notifications(store.list()));
    }
}

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Server {
    fn get_capabilities(&self) -> Vec<String> {
        vec!["body".to_owned(), "persistence".to_owned()]
    }

    #[allow(clippy::too_many_arguments)]
    async fn notify(
        &self,
        app_name: String,
        replaces_id: u32,
        _app_icon: String,
        summary: String,
        body: String,
        _actions: Vec<String>,
        hints: HashMap<String, OwnedValue>,
        _expire_timeout: i32,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> u32 {
        let transient = hints
            .get("transient")
            .and_then(|v| v.downcast_ref::<bool>().ok())
            .unwrap_or(false);
        let (id, dropped) = {
            let mut store = self.store.lock().unwrap_or_else(|e| e.into_inner());
            let r = store.notify(&app_name, replaces_id, &summary, &body, transient);
            self.publish(&store);
            r
        };
        if let Some(old) = dropped {
            let _ = Self::notification_closed(&emitter, old, REASON_EXPIRED).await;
        }
        if transient {
            let _ = Self::notification_closed(&emitter, id, REASON_EXPIRED).await;
        }
        id
    }

    async fn close_notification(
        &self,
        id: u32,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        let closed = {
            let mut store = self.store.lock().unwrap_or_else(|e| e.into_inner());
            let closed = store.close(id);
            if closed {
                self.publish(&store);
            }
            closed
        };
        if closed {
            Self::notification_closed(&emitter, id, REASON_CLOSED).await?;
        }
        Ok(())
    }

    fn get_server_information(&self) -> (String, String, String, String) {
        (
            "desicompass-bar".to_owned(),
            "friendlyflow".to_owned(),
            env!("CARGO_PKG_VERSION").to_owned(),
            "1.2".to_owned(),
        )
    }

    #[zbus(signal)]
    async fn notification_closed(
        emitter: &SignalEmitter<'_>,
        id: u32,
        reason: u32,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn action_invoked(
        emitter: &SignalEmitter<'_>,
        id: u32,
        action_key: String,
    ) -> zbus::Result<()>;
}

/// Serve the notifications, from a thread of their own.
pub fn start(tx: Sender<Update>) {
    let spawned = std::thread::Builder::new()
        .name("bar-notifications".into())
        .spawn(move || {
            let _ = tx.send(Update::Notifications(Vec::new()));
            let server = Server {
                store: Arc::new(Mutex::new(Store::default())),
                tx,
            };
            let conn = match zbus::blocking::connection::Builder::session()
                .and_then(|b| b.serve_at(PATH, server))
                .and_then(|b| b.build())
            {
                Ok(conn) => conn,
                Err(e) => {
                    tracing::warn!("no session bus for notifications: {e}");
                    return;
                }
            };
            match conn.request_name_with_flags(NAME, zbus::fdo::RequestNameFlags::DoNotQueue.into())
            {
                Ok(zbus::fdo::RequestNameReply::PrimaryOwner) => {
                    tracing::info!("serving {NAME}")
                }
                Ok(reply) => {
                    tracing::info!("{NAME} is taken ({reply:?}); another server has them");
                    return;
                }
                Err(zbus::Error::NameTaken) => {
                    tracing::info!("{NAME} is taken; another server has them");
                    return;
                }
                Err(e) => {
                    tracing::warn!("could not take {NAME}: {e}");
                    return;
                }
            }
            // The connection serves from its own executor; this thread only
            // keeps it alive.
            loop {
                std::thread::park();
            }
        });
    if let Err(e) = spawned {
        tracing::warn!("notifications: could not start their thread: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notifications_are_kept_in_order_until_dismissed() {
        let mut s = Store::default();
        let (a, _) = s.notify("Mail", 0, "One", "", false);
        let (b, _) = s.notify("Chat", 0, "Two", "hi", false);
        assert_ne!(a, b);
        assert_eq!(
            s.list()
                .iter()
                .map(|n| n.summary.as_str())
                .collect::<Vec<_>>(),
            ["One", "Two"]
        );
        assert!(s.close(a));
        assert!(!s.close(a), "already gone");
        assert_eq!(s.list().len(), 1);
    }

    #[test]
    fn a_replacement_keeps_its_id_and_place() {
        let mut s = Store::default();
        let (a, _) = s.notify("Mail", 0, "One", "", false);
        s.notify("Mail", 0, "Two", "", false);
        assert_eq!(s.notify("Mail", a, "One, updated", "x", false), (a, None));
        assert_eq!(s.list()[0].summary, "One, updated");
        assert_eq!(s.list().len(), 2);
        // Replacing one that is gone is a new notification.
        s.close(a);
        let (c, _) = s.notify("Mail", a, "Three", "", false);
        assert_ne!(c, a);
    }

    #[test]
    fn a_transient_notification_gets_an_id_but_is_not_kept() {
        let mut s = Store::default();
        let (id, _) = s.notify("Volume", 0, "50%", "", true);
        assert_ne!(id, 0);
        assert!(s.list().is_empty());
    }

    #[test]
    fn past_the_limit_the_oldest_goes() {
        let mut s = Store::default();
        let (first, _) = s.notify("a", 0, "0", "", false);
        for i in 1..MAX_KEPT {
            assert_eq!(s.notify("a", 0, &i.to_string(), "", false).1, None);
        }
        let (_, dropped) = s.notify("a", 0, "last", "", false);
        assert_eq!(dropped, Some(first));
        assert_eq!(s.list().len(), MAX_KEPT);
    }

    #[test]
    fn ids_are_never_zero() {
        let mut s = Store {
            last_id: u32::MAX,
            items: Vec::new(),
        };
        assert_eq!(s.notify("a", 0, "x", "", false).0, 1);
    }
}
