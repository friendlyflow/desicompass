//! desicompass-superkey: the list a bare Super tap opens.
//!
//! A resident sicompass-ui client, started once per session by desicompass
//! and shown and hidden by it. The root of the list is five sections
//! (notifications, windows, controls, the status the bar shows, settings)
//! and then every installed program. It lives in simple search, and Enter does what the row
//! is for. See `docs/superkey.md` at the root of the desicompass repository.
//!
//! A library as well as a binary so `tests/` can drive the real provider
//! through the real renderer.

#![cfg(target_os = "linux")]

pub mod actions;
pub mod apps;
pub mod gui;
pub mod i18n;
pub mod ipc;
pub mod power;
pub mod provider;
pub mod status;
