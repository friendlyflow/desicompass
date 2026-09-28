//! `org.freedesktop.a11y.KeyboardMonitor`: how Orca hears the keyboard.
//!
//! On Wayland a screen reader cannot see key presses; the compositor has to
//! hand them over. Orca asks for them through libatspi, over this D-Bus
//! interface on the session bus. Without it Orca never learns a key was
//! pressed, and a key press is what makes Orca stop talking: it interrupts its
//! speech on every one (`input_event.py`). Under desicompass every row passed
//! while arrowing quickly was read out in turn, where under COSMIC or GNOME
//! only the row landed on is. Orca's own key commands, its typing echo and
//! Ctrl-to-stop depend on it too.
//!
//! Ported from cosmic-comp (`src/dbus/a11y_keyboard_monitor.rs` and
//! `name_owners.rs`, GPL-3.0-only, System76), which follows Mutter's
//! `data/dbus-interfaces/org.freedesktop.a11y.xml`. What differs:
//!
//! - The D-Bus side runs on a thread of its own, with the blocking zbus
//!   desicompass already uses for localed, instead of on the event loop's
//!   executor. The compositor hands it signals through a channel.
//! - The caller check asks the bus who owns Orca's name on each call, instead
//!   of cosmic-comp's cached name-owner tracker. These calls are rare (once
//!   per Orca start, and on a settings change).
//! - Held keys are tracked here, since desicompass has no suppressed-key set.
//! - cosmic-comp re-sends Caps Lock after using it as the Orca key, to undo
//!   the lock it toggled. Not ported: Orca's default (desktop) layout uses
//!   Insert, which toggles nothing.
//!
//! Only Orca may use it: a caller must own `org.gnome.Orca.KeyboardMonitor`,
//! the name libatspi takes for Orca. Everything typed goes to it, passwords
//! included; sicompass-ui gives a password row the password-text role, which
//! is what stops Orca echoing those keys.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, mpsc};

use smithay::backend::input::KeyState;
use smithay::input::keyboard::{Keysym, KeysymHandle, ModifiersState};
use tracing::{debug, info, warn};
use zbus::message::Header;
use zbus::names::{BusName, OwnedUniqueName, UniqueName};

const MANAGER_NAME: &str = "org.freedesktop.a11y.Manager";
const MANAGER_PATH: &str = "/org/freedesktop/a11y/Manager";
const INTERFACE: &str = "org.freedesktop.a11y.KeyboardMonitor";
/// The only name allowed to call: the one libatspi takes for Orca.
const ALLOWED_NAME: &str = "org.gnome.Orca.KeyboardMonitor";

/// As defined in at-spi2-core: bits from here up in a grab's modifier mask
/// are the client's virtual modifiers (Orca's own "Orca key"), in the order
/// it listed them.
const ATSPI_DEVICE_A11Y_MANAGER_VIRTUAL_MOD_START: u32 = 15;

#[derive(PartialEq, Eq, Debug)]
struct KeyGrab {
    mods: u32,
    virtual_mods: HashSet<Keysym>,
    key: Keysym,
}

impl KeyGrab {
    fn new(virtual_mods: &[Keysym], key: Keysym, raw_mods: u32) -> Self {
        let mods = raw_mods & ((1 << ATSPI_DEVICE_A11Y_MANAGER_VIRTUAL_MOD_START) - 1);
        let virtual_mods = virtual_mods
            .iter()
            .copied()
            .enumerate()
            .filter(|(i, _)| {
                raw_mods & (1 << (ATSPI_DEVICE_A11Y_MANAGER_VIRTUAL_MOD_START + *i as u32)) != 0
            })
            .map(|(_, x)| x)
            .collect();
        Self {
            mods,
            virtual_mods,
            key,
        }
    }
}

#[derive(Debug, Default)]
struct Client {
    grabbed: bool,
    watched: bool,
    virtual_mods: HashSet<Keysym>,
    key_grabs: Vec<KeyGrab>,
}

#[derive(Debug, Default)]
struct Clients(HashMap<OwnedUniqueName, Client>);

impl Clients {
    fn get(&mut self, name: &UniqueName<'_>) -> &mut Client {
        self.0.entry(name.to_owned().into()).or_default()
    }

    fn has_virtual_mod(&self, keysym: Keysym) -> bool {
        self.0.values().any(|c| c.virtual_mods.contains(&keysym))
    }

    fn has_keyboard_grab(&self) -> bool {
        self.0.values().any(|c| c.grabbed)
    }

    /// A grab for `key` with exactly these modifiers and virtual modifiers.
    fn has_key_grab(&self, depressed: u32, active: &HashSet<Keysym>, key: Keysym) -> bool {
        self.0
            .values()
            .flat_map(|c| &c.key_grabs)
            .any(|g| g.mods == depressed && &g.virtual_mods == active && g.key == key)
    }
}

/// One `KeyEvent` signal, for the D-Bus thread to send.
struct Signal {
    destination: OwnedUniqueName,
    released: bool,
    state: u32,
    keysym: u32,
    unichar: u32,
    keycode: u16,
}

/// The compositor's half: decides what the client may see, and queues the
/// signals for Orca.
pub struct A11yKeyboardMonitor {
    clients: Arc<Mutex<Clients>>,
    active_virtual_mods: HashSet<Keysym>,
    /// Raw keycodes whose press went to the screen reader only, so that their
    /// release is kept from the client too.
    suppressed: HashSet<u32>,
    signals: mpsc::Sender<Signal>,
}

impl A11yKeyboardMonitor {
    /// Serve the interface on the session bus, on a thread of its own.
    ///
    /// Never fails the compositor: with no session bus, or with the name
    /// already taken (a desicompass nested in a desktop that serves it
    /// itself), it logs why and every key simply goes to the client.
    pub fn start() -> Self {
        let clients = Arc::new(Mutex::new(Clients::default()));
        let (signals, rx) = mpsc::channel();
        let for_thread = Arc::clone(&clients);
        let _ = std::thread::Builder::new()
            .name("a11y-keyboard".into())
            .spawn(move || serve(for_thread, rx));
        Self {
            clients,
            active_virtual_mods: HashSet::new(),
            suppressed: HashSet::new(),
            signals,
        }
    }

    /// Tell the screen reader about a key, and decide whether the client may
    /// see it. Returns true when the key is the screen reader's alone.
    ///
    /// Call it first in the keyboard filter, before the compositor's own
    /// bindings, as cosmic-comp does.
    pub fn filter(
        &mut self,
        modifiers: &ModifiersState,
        keysym: &KeysymHandle<'_>,
        state: KeyState,
    ) -> bool {
        let sym = keysym.modified_sym();
        let code = keysym.raw_code().raw();
        let clients = self.clients.lock().unwrap_or_else(|e| e.into_inner());

        self.notify(&clients, modifiers, keysym, sym, state);

        decide(
            &clients,
            &mut self.active_virtual_mods,
            &mut self.suppressed,
            modifiers.serialized.depressed,
            sym,
            code,
            state,
        )
    }

    /// Queue a `KeyEvent` for every client watching the keyboard, or holding a
    /// grab for this key.
    fn notify(
        &self,
        clients: &Clients,
        modifiers: &ModifiersState,
        keysym: &KeysymHandle<'_>,
        sym: Keysym,
        state: KeyState,
    ) {
        let wanted: Vec<&OwnedUniqueName> = clients
            .0
            .iter()
            .filter(|(_, c)| {
                c.watched
                    || clients.has_key_grab(
                        modifiers.serialized.depressed,
                        &self.active_virtual_mods,
                        sym,
                    )
            })
            .map(|(name, _)| name)
            .collect();
        if wanted.is_empty() {
            return;
        }
        let unichar = {
            let xkb = keysym.xkb().lock().unwrap_or_else(|e| e.into_inner());
            // SAFETY: only read, and not kept past this block; smithay's own
            // lock is not held while the filter runs.
            unsafe { xkb.state() }.key_get_utf32(keysym.raw_code())
        };
        for destination in wanted {
            let _ = self.signals.send(Signal {
                destination: destination.clone(),
                released: state == KeyState::Released,
                state: modifiers.serialized.depressed,
                keysym: sym.raw(),
                unichar,
                keycode: keysym.raw_code().raw() as u16,
            });
        }
    }
}

/// The filter's decision, apart from anything D-Bus or xkb, so it can be
/// tested: cosmic-comp's order, for one key.
fn decide(
    clients: &Clients,
    active_virtual_mods: &mut HashSet<Keysym>,
    suppressed: &mut HashSet<u32>,
    depressed: u32,
    sym: Keysym,
    code: u32,
    state: KeyState,
) -> bool {
    match state {
        KeyState::Released => {
            active_virtual_mods.remove(&sym);
            // A key whose press the client never saw must not be released
            // at it either.
            suppressed.remove(&code)
        }
        KeyState::Pressed => {
            // The screen reader's own modifier (Insert, by default): held for
            // it, never seen by the client.
            if clients.has_virtual_mod(sym) {
                active_virtual_mods.insert(sym);
                suppressed.insert(code);
                debug!("active virtual mods: {active_virtual_mods:?}");
                return true;
            }
            if clients.has_keyboard_grab()
                || clients.has_key_grab(depressed, active_virtual_mods, sym)
            {
                suppressed.insert(code);
                return true;
            }
            false
        }
    }
}

/// The D-Bus thread: serve the interface, forget callers that leave, and send
/// the queued signals.
fn serve(clients: Arc<Mutex<Clients>>, signals: mpsc::Receiver<Signal>) {
    let iface = KeyboardMonitor {
        clients: Arc::clone(&clients),
    };
    let conn = match zbus::blocking::connection::Builder::session()
        .and_then(|b| b.serve_at(MANAGER_PATH, iface))
        .and_then(|b| b.build())
    {
        Ok(conn) => conn,
        Err(e) => {
            warn!("no session bus for {INTERFACE}; screen readers will not hear keys: {e}");
            return;
        }
    };
    match conn.request_name_with_flags(MANAGER_NAME, zbus::fdo::RequestNameFlags::DoNotQueue.into())
    {
        Ok(zbus::fdo::RequestNameReply::PrimaryOwner) => {
            info!("serving {INTERFACE} for screen readers")
        }
        Ok(reply) => {
            warn!("{MANAGER_NAME} is taken ({reply:?}); screen readers get keys from its owner");
            return;
        }
        Err(e) => {
            warn!("could not take {MANAGER_NAME}: {e}");
            return;
        }
    }

    // Forget a client when it leaves the bus, or stops owning Orca's name.
    let watch_conn = conn.clone();
    let watch_clients = Arc::clone(&clients);
    let _ = std::thread::Builder::new()
        .name("a11y-keyboard-owners".into())
        .spawn(move || forget_departed(&watch_conn, &watch_clients));

    for s in signals {
        if let Err(e) = conn.emit_signal(
            Some(s.destination.as_str()),
            MANAGER_PATH,
            INTERFACE,
            "KeyEvent",
            &(s.released, s.state, s.keysym, s.unichar, s.keycode),
        ) {
            debug!("could not send KeyEvent to {}: {e}", s.destination);
        }
    }
}

fn forget_departed(conn: &zbus::blocking::Connection, clients: &Mutex<Clients>) {
    let Ok(dbus) = zbus::blocking::fdo::DBusProxy::new(conn) else {
        return;
    };
    let Ok(changes) = dbus.receive_name_owner_changed() else {
        return;
    };
    for change in changes {
        let Ok(args) = change.args() else {
            continue;
        };
        let gone: Option<OwnedUniqueName> = match args.name() {
            BusName::Unique(name) if args.new_owner().is_none() => Some(name.to_owned().into()),
            BusName::WellKnown(name) if name.as_str() == ALLOWED_NAME => {
                args.old_owner().as_ref().map(|o| o.to_owned().into())
            }
            _ => None,
        };
        if let Some(name) = gone
            && clients
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .0
                .remove(&name)
                .is_some()
        {
            debug!("{name} left; its key grabs are gone");
        }
    }
}

struct KeyboardMonitor {
    clients: Arc<Mutex<Clients>>,
}

/// The caller, if it owns Orca's name.
async fn allowed_sender(
    header: &Header<'_>,
    conn: &zbus::Connection,
) -> zbus::fdo::Result<OwnedUniqueName> {
    let denied = || zbus::fdo::Error::AccessDenied("Access denied".to_owned());
    let sender = header.sender().ok_or_else(denied)?;
    let dbus = zbus::fdo::DBusProxy::new(conn).await?;
    let orca = BusName::try_from(ALLOWED_NAME).map_err(|_| denied())?;
    match dbus.get_name_owner(orca).await {
        Ok(owner) if owner.as_str() == sender.as_str() => Ok(sender.to_owned().into()),
        _ => Err(denied()),
    }
}

#[zbus::interface(name = "org.freedesktop.a11y.KeyboardMonitor")]
impl KeyboardMonitor {
    async fn grab_keyboard(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
    ) -> zbus::fdo::Result<()> {
        let sender = allowed_sender(&header, conn).await?;
        self.clients.lock().unwrap().get(&sender).grabbed = true;
        debug!("grab keyboard by {sender}");
        Ok(())
    }

    async fn ungrab_keyboard(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
    ) -> zbus::fdo::Result<()> {
        let sender = allowed_sender(&header, conn).await?;
        self.clients.lock().unwrap().get(&sender).grabbed = false;
        debug!("ungrab keyboard by {sender}");
        Ok(())
    }

    async fn watch_keyboard(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
    ) -> zbus::fdo::Result<()> {
        let sender = allowed_sender(&header, conn).await?;
        self.clients.lock().unwrap().get(&sender).watched = true;
        info!("{sender} is watching the keyboard");
        Ok(())
    }

    async fn unwatch_keyboard(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
    ) -> zbus::fdo::Result<()> {
        let sender = allowed_sender(&header, conn).await?;
        self.clients.lock().unwrap().get(&sender).watched = false;
        debug!("unwatch keyboard by {sender}");
        Ok(())
    }

    async fn set_key_grabs(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
        virtual_mods: Vec<u32>,
        keystrokes: Vec<(u32, u32)>,
    ) -> zbus::fdo::Result<()> {
        let sender = allowed_sender(&header, conn).await?;
        let virtual_mods: Vec<Keysym> = virtual_mods.into_iter().map(Keysym::from).collect();
        let key_grabs: Vec<KeyGrab> = keystrokes
            .into_iter()
            .map(|(k, mods)| KeyGrab::new(&virtual_mods, Keysym::from(k), mods))
            .collect();
        debug!(
            "key grabs set by {sender}: {:?}",
            (&virtual_mods, &key_grabs)
        );
        let mut clients = self.clients.lock().unwrap();
        let client = clients.get(&sender);
        client.virtual_mods = virtual_mods.into_iter().collect();
        client.key_grabs = key_grabs;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INSERT: Keysym = Keysym::Insert;
    const H: Keysym = Keysym::h;
    const DOWN: Keysym = Keysym::Down;

    fn orca(clients: &mut Clients) -> &mut Client {
        clients.get(&UniqueName::try_from(":1.42").unwrap())
    }

    /// Orca's grab for Insert+H: the Insert virtual modifier is bit 15.
    fn orca_with_insert_h() -> Clients {
        let mut clients = Clients::default();
        let c = orca(&mut clients);
        c.watched = true;
        c.virtual_mods = [INSERT].into_iter().collect();
        c.key_grabs = vec![KeyGrab::new(&[INSERT], H, 1 << 15)];
        clients
    }

    #[test]
    fn a_grab_decodes_its_virtual_modifiers() {
        let g = KeyGrab::new(&[INSERT, Keysym::Caps_Lock], H, (1 << 16) | 0b100);
        assert_eq!(g.mods, 0b100, "real modifiers below bit 15");
        assert_eq!(g.virtual_mods, [Keysym::Caps_Lock].into_iter().collect());
        assert_eq!(g.key, H);
    }

    #[test]
    fn ordinary_keys_reach_the_client() {
        let clients = orca_with_insert_h();
        let (mut active, mut suppressed) = (HashSet::new(), HashSet::new());
        assert!(!decide(
            &clients,
            &mut active,
            &mut suppressed,
            0,
            DOWN,
            116,
            KeyState::Pressed
        ));
        assert!(!decide(
            &clients,
            &mut active,
            &mut suppressed,
            0,
            DOWN,
            116,
            KeyState::Released
        ));
    }

    /// Insert+H is Orca's: neither the press nor the releases of either key
    /// may reach the client.
    #[test]
    fn the_orca_key_and_its_commands_are_kept_from_the_client() {
        let clients = orca_with_insert_h();
        let (mut active, mut suppressed) = (HashSet::new(), HashSet::new());

        assert!(decide(
            &clients,
            &mut active,
            &mut suppressed,
            0,
            INSERT,
            118,
            KeyState::Pressed
        ));
        assert!(active.contains(&INSERT));
        assert!(decide(
            &clients,
            &mut active,
            &mut suppressed,
            0,
            H,
            43,
            KeyState::Pressed
        ));
        assert!(decide(
            &clients,
            &mut active,
            &mut suppressed,
            0,
            H,
            43,
            KeyState::Released
        ));
        assert!(decide(
            &clients,
            &mut active,
            &mut suppressed,
            0,
            INSERT,
            118,
            KeyState::Released
        ));
        assert!(active.is_empty());
        assert!(suppressed.is_empty(), "every suppressed key was released");

        // Plain H, with Insert no longer held, is typing.
        assert!(!decide(
            &clients,
            &mut active,
            &mut suppressed,
            0,
            H,
            43,
            KeyState::Pressed
        ));
    }

    #[test]
    fn a_keyboard_grab_takes_every_key() {
        let mut clients = Clients::default();
        orca(&mut clients).grabbed = true;
        let (mut active, mut suppressed) = (HashSet::new(), HashSet::new());
        assert!(decide(
            &clients,
            &mut active,
            &mut suppressed,
            0,
            DOWN,
            116,
            KeyState::Pressed
        ));
        assert!(decide(
            &clients,
            &mut active,
            &mut suppressed,
            0,
            DOWN,
            116,
            KeyState::Released
        ));
    }

    #[test]
    fn with_no_screen_reader_nothing_is_kept() {
        let clients = Clients::default();
        let (mut active, mut suppressed) = (HashSet::new(), HashSet::new());
        for sym in [INSERT, H, DOWN] {
            assert!(!decide(
                &clients,
                &mut active,
                &mut suppressed,
                0,
                sym,
                1,
                KeyState::Pressed
            ));
            assert!(!decide(
                &clients,
                &mut active,
                &mut suppressed,
                0,
                sym,
                1,
                KeyState::Released
            ));
        }
    }
}
