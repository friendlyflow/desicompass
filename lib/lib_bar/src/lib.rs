//! desicompass-bar: the strip along the top or bottom of a desicompass
//! session, with the clock and the status icons.
//!
//! A resident sicompass-ui client, started once per session by desicompass
//! and placed by it (see `bar.rs` in the compositor). It is also where the
//! session's status is gathered: it follows NetworkManager, UPower, BlueZ and
//! WirePlumber, and it is the session's notification server and tray. What it
//! knows it writes for the superkey, whose Status section is how that is
//! reached by keyboard and screen reader. See `docs/bar.md` at the root of the
//! desicompass repository.
//!
//! A library as well as a binary so the pieces can be tested on their own.

#![cfg(target_os = "linux")]

pub mod gui;
pub mod icons;
pub mod ipc;
pub mod layout;
pub mod model;
pub mod speech;
pub mod status;
