//! The Status section's two actions, which go to the bar over the session bus:
//! dismissing a notification (`CloseNotification`, to the notification server
//! the bar is) and activating a tray item (`Activate`, to the item itself).
//!
//! Behind a trait so the UI tests can see what was asked without a bus. The
//! real calls run on a thread of their own: a tray item that hangs must not
//! freeze the superkey for D-Bus's 25-second timeout.

use std::sync::{Arc, Mutex};

pub trait StatusActions: Send + Sync {
    fn close_notification(&self, id: u32);
    fn activate(&self, service: &str, path: &str);
    /// What went wrong since the last call, to show the user.
    fn take_error(&self) -> Option<String> {
        None
    }
}

/// The actions, done over the session bus.
#[derive(Default)]
pub struct DbusActions {
    error: Arc<Mutex<Option<String>>>,
}

impl DbusActions {
    pub fn new() -> Self {
        Self::default()
    }

    fn call(
        &self,
        dest: String,
        path: String,
        iface: &'static str,
        method: &'static str,
        body: impl serde::Serialize + zbus::zvariant::DynamicType + Send + 'static,
    ) {
        let error = Arc::clone(&self.error);
        let spawned = std::thread::Builder::new()
            .name("superkey-status".into())
            .spawn(move || {
                let result = zbus::blocking::Connection::session().and_then(|conn| {
                    conn.call_method(
                        Some(dest.as_str()),
                        path.as_str(),
                        Some(iface),
                        method,
                        &body,
                    )
                });
                if let Err(e) = result {
                    tracing::warn!("{iface}.{method} on {dest}{path} failed: {e}");
                    *error.lock().unwrap_or_else(|e| e.into_inner()) = Some(e.to_string());
                }
            });
        if let Err(e) = spawned {
            tracing::warn!("could not call {method}: {e}");
        }
    }
}

impl StatusActions for DbusActions {
    fn close_notification(&self, id: u32) {
        self.call(
            "org.freedesktop.Notifications".to_owned(),
            "/org/freedesktop/Notifications".to_owned(),
            "org.freedesktop.Notifications",
            "CloseNotification",
            (id,),
        );
    }

    fn activate(&self, service: &str, path: &str) {
        // Where to open, for items that care: nowhere in particular, since
        // there is no pointer.
        self.call(
            service.to_owned(),
            path.to_owned(),
            "org.kde.StatusNotifierItem",
            "Activate",
            (0i32, 0i32),
        );
    }

    fn take_error(&self) -> Option<String> {
        self.error.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
}

/// Records what was asked, for tests.
#[derive(Default)]
pub struct RecordedActions {
    pub calls: Mutex<Vec<String>>,
}

impl StatusActions for RecordedActions {
    fn close_notification(&self, id: u32) {
        self.calls.lock().unwrap().push(format!("close {id}"));
    }

    fn activate(&self, service: &str, path: &str) {
        self.calls
            .lock()
            .unwrap()
            .push(format!("activate {service}{path}"));
    }
}

impl<T: StatusActions + ?Sized> StatusActions for Arc<T> {
    fn close_notification(&self, id: u32) {
        (**self).close_notification(id)
    }
    fn activate(&self, service: &str, path: &str) {
        (**self).activate(service, path)
    }
    fn take_error(&self) -> Option<String> {
        (**self).take_error()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recorder_remembers_in_order() {
        let r = Arc::new(RecordedActions::default());
        let a: Box<dyn StatusActions> = Box::new(Arc::clone(&r));
        a.close_notification(3);
        a.activate(":1.2", "/StatusNotifierItem");
        assert_eq!(
            *r.calls.lock().unwrap(),
            ["close 3", "activate :1.2/StatusNotifierItem"]
        );
        assert_eq!(a.take_error(), None);
    }
}
