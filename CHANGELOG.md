# Changelog

## 0.2.2

Runs sicompass 0.2.3 and loginsicompass 0.2.1.

- The Store in the superkey (Super+S twice quickly) turns on cloud sync when you pay for Sicompass Cloud, the same as in sicompass: a plugin's "enable cloud sync" checkbox appears in sicompass once you hold the service, and the superkey's Store ticks it for the plugins the service covers. The Store pages say which plugins that is.
- A renewed subscription reaches the Store here too, so a monthly subscription no longer looks expired after a month.

## 0.2.1

Runs sicompass 0.2.1 and loginsicompass 0.2.1.

### The superkey

- Tap Super for the superkey: windows, controls, settings and programs in one
  list. Escape closes it, and it has no tabs or undo.
- Super+T opens the tutorial, and its programs section follows what you install.
- Super+S opens the superkey's settings, and Super+S twice quickly opens the
  Store, which sicompass leaves out in a session.
- Super+D says the time and date.

### The bar

- A clock, status icons, notifications and a tray. Status and Notifications are
  also sections of the superkey.
- It can show the keys being pressed (Settings > Bar > show key strokes).

### Accessibility

- `services.desicompass.accessibility.*` sets the accessibility settings, shared
  by the session, the superkey and the login screen.
- Orca gets the keyboard (`org.freedesktop.a11y.KeyboardMonitor`).
- The greeter's D-Bus daemon has writable XDG directories.

### Building and the dev session

- A dev session, which runs its own packages built by Nix.
  `services.desicompass.dev.plugins` hands it plugins built from local checkouts.
- The TTY backend is built by default, and `cargo run` runs `desicompass`.
