# Changelog

## 0.2.4

Runs sicompass 0.2.5 and loginsicompass 0.2.2.

- The Store and the beginner's manual in the superkey are the ones from sicompass 0.2.5: the photo album and the video store are in the Store, in a media category, and the update banner says what it cannot update.
- Pages from plugins arrive much faster, through sicompass-sdk 0.9.14.

## 0.2.3

Runs sicompass 0.2.4 and loginsicompass 0.2.2.

- The superkey and the bar look like the app: icons in the prefixes, a back arrow on the parent line, links and buttons that stand out, and a rounded border on the row you are on. `f` searches, and Tab opens the beginner's manual.
- Packages for Debian, Ubuntu, Mint, Fedora, RHEL and openSUSE: a `.deb`, an `.rpm` and an archive, built on Ubuntu 22.04 and installed in a bare system before they are published.
- Programs you start from a session no longer inherit `LD_LIBRARY_PATH`, which made them load the compositor's libraries and die.

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
