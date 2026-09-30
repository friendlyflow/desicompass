# The bar

`desicompass-bar` (`lib/lib_bar`) is the strip along the bottom (or top) of the
output: the date and time at the right, and to their left the notifications,
battery, volume, Bluetooth, network and tray icons. Like the superkey it is a
sicompass-ui client that the compositor starts and places, and it never links
the Sicompass application.

It is also where the session's status is gathered. It follows the services that
know it, it is the session's notification server and its tray, and it writes
what it knows to a file that the superkey's Status section reads. That section
is how the status reaches a keyboard and a screen reader, since the bar itself
never has the keyboard.

This document is about how the pieces work together. The user-facing side is in
the README.

## Who does what

| | Compositor (`src/bar.rs`) | Bar (`lib/lib_bar`) | Superkey (`lib/lib_superkey`) |
|---|---|---|---|
| Starting | Finds it (`bar::resolve_command`), runs it, restarts it with a back-off (`managed_client.rs`, shared with the superkey) | Says `hello`, then `place` | |
| Placing | Gives it its edge of the output and the height it asked for. Tiles, maximised and full-screen windows get what is left (`usable_area`). Keeps it out of the tiler, the window list and the focus stack | Works out 1.7 lines of its font, tells the compositor again when the font scale or the position changes | |
| Keys | Super+T sends `say-time`. Super+B and Super+N open the superkey on Status and Notifications | Runs `spd-say` | Shows Status, Notifications |
| Settings | Nothing | Follows `bar.json` | Writes `bar.json` (Settings > Bar) |
| Status | Nothing | Follows the services, writes `status-<display>.json` | Lists it (Status) |
| Notifications | Nothing | Is `org.freedesktop.Notifications` | Lists them, Enter calls `CloseNotification` |
| Tray | Nothing | Is `org.kde.StatusNotifierWatcher` and the host | Lists the items, Enter calls `Activate` |

## Finding it

The same as the superkey: `DESICOMPASS_BAR` when set (empty means no bar), else
`desicompass-bar` next to the compositor's binary, else on `PATH`. The Nix
package's wrapper sets the variable to its own build as a default, and the
NixOS module sets it empty for the login screen's compositor. A variable rather
than a flag, for the same reason: a compositor older than the bar ignores it
instead of refusing to start.

## The channel

Built exactly like the superkey's (see [superkey.md](superkey.md)): a socketpair
inserted into the Wayland display as a client, so the bar's toplevel is known by
its `ClientId`, and a second one for the protocol, its fd number in
`DESICOMPASS_BAR_FD`. One JSON object per line, from `lib/lib_bar_protocol`.

```text
compositor -> bar
  {"type":"say-time"}
bar -> compositor
  {"type":"hello","version":1}
  {"type":"place","edge":"bottom","height":56}
```

`height` is in the compositor's logical pixels: 1.7 of the bar's lines, divided
by the window's pixel density. The compositor clamps it to between one pixel and
half the output. Until the bar's window exists it takes no room, so a bar that
fails to start costs the windows nothing.

When the compositor closes the channel, the bar exits and removes its status
file. When the bar exits, the compositor gives the windows the whole output
back and starts it again.

## Drawing

The bar uses sicompass-ui's window, font renderer, rectangle and image renderers
and `render::draw_frame`, but not its main loop, which is for lists and draws
sixty times a second. Its own loop (`gui.rs`) sleeps on SDL's event queue a
quarter of a second at a time, and draws only when something shown changed: the
clock's text, a status, the settings, the colour scheme or the size.

- The background is the palette's `selected` colour and everything on it is the
  palette's `text` colour: the bar looks like the list's focused row.
- It follows the shared accessibility object for the colour scheme, the font
  scale, the language and shoulder-surfing protection, which blanks it.
- The icons are drawn by the bar itself (`icons.rs`): a few shapes each,
  rasterised with signed distances at the exact size and in the exact colour
  they are shown, and handed to the renderer as PNGs under `asset:` URIs.
  sicompass-ui has no icon font, no SVG and no tint, and this needs no files in
  the repository and nothing to regenerate. Good and bad are told apart by
  shape: a slash for off, an exclamation mark for a network without internet,
  dimmed arcs for a weak signal, a bolt for charging.
- The layout (`layout.rs`) hangs everything from the right edge: the clock one
  em in, then each item leftwards, an icon one text-height square with its
  amount (`81%`, `2`) after it. What does not fit on the left is left out.

The bar has no AccessKit tree. It never has the keyboard, so there is nothing
in it for a screen reader to follow: the superkey's Status section says it all.

## The settings

`$XDG_CONFIG_HOME/desicompass/bar.json`, per user:

```json
{ "barPosition": "bottom", "barSeconds": false }
```

The superkey writes it (Settings > Bar) through `BarSettingsFile`, atomically,
keeping keys it does not know. The bar looks at it every quarter second and
follows. These are the user's own, so they are not in the machine-wide
accessibility object the login screen shares, whose `set` refuses unknown keys
anyway.

## The status file

`$XDG_RUNTIME_DIR/desicompass/status-<WAYLAND_DISPLAY>.json`, written atomically
by the bar whenever what it knows changes (`StatusSnapshot`): network, audio,
battery, Bluetooth, the notifications and the tray items. One file per session,
so a nested desicompass never overwrites the status of the one it runs in. The
superkey polls it every quarter second and rebuilds its Status section when it
changed.

## Where the status comes from

| Source | Where | How |
|---|---|---|
| network | NetworkManager, system bus | `PrimaryConnectionType`, `State`, `Connectivity`, and the access point's `Strength` |
| battery | UPower's display device, system bus | `IsPresent`, `Percentage`, `State` |
| Bluetooth | BlueZ's object manager, system bus | adapters `Powered`, devices `Connected` |
| audio | `wpctl get-volume @DEFAULT_AUDIO_SINK@` | every two seconds |
| notifications | the bar, `org.freedesktop.Notifications` | every call |
| tray | the bar, `org.kde.StatusNotifierWatcher` | every registration, item signal and departure |

The three system services are read on each of their signals and every thirty
seconds. A service that is not there reports nothing and has no icon, and is
looked for again, since it can start later. PipeWire has no D-Bus interface for
the volume, and `wpctl` is on every system that runs WirePlumber, so the bar
asks it rather than linking libpipewire.

## Notifications

The bar is the notification server, and nothing pops up: it is a notification
centre. A notification is kept until it is dismissed, so `expire_timeout` (how
long a popup stays up) has nothing to act on. One with the `transient` hint gets
its id and `NotificationClosed` straight away, and is not kept. At most a
hundred are kept, and past that the oldest is dropped as expired.

The superkey's Notifications section (first in its root, Super+N) lists them,
oldest first, with "Dismiss
all" at the top when there is more than one. Enter calls `CloseNotification` on
the bar, which answers the sender with `NotificationClosed` (reason 3). The
superkey takes the row off at once rather than waiting for the next status
file.

## The tray

The bar is the `StatusNotifierWatcher` and the host. An item registers by bus
name or, like libappindicator (Dropbox), by an object path on its own
connection. The bar shows the item's pixmap, or a PNG found by its icon name in
its own theme path, then the hicolor theme of each data directory, then
`pixmaps`. An SVG icon is not drawn: the item shows as the first letter of its
title. A `Passive` item is not shown. An item goes when its owner leaves the
bus.

Status > Tray lists the items by title, and Enter calls the item's `Activate`
and closes the superkey, so what it opens is seen. Menus
(`com.canonical.dbusmenu`) are not shown, so an item that is only a menu (many
libappindicator ones are) may answer `Activate` with an error, which the
superkey shows.

Only programs on the session's bus can register, and in a desicompass session
that is the private bus `dbus-run-session` started. A program started inside
the session (from Sicompass or the superkey) is on it. A systemd user service is
on the user bus instead, and its icon never appears.

## Super+T

The compositor sends `say-time`, and the bar runs `spd-say` with the time in the
session's language (`It is 14:05`, `Het is 14:05`), at the `message` priority so
it is not queued behind a long reading. speech-dispatcher is what Orca speaks
through too, so the two share a voice and a queue, and the bar never starts or
stops Orca. `spd-say` comes from the session's `PATH`: the module enables Orca,
which brings speech-dispatcher.

## Trying it

In the dev shell:

```bash
cargo build --workspace
cargo run -- --backend auto --startup-cmd foot
cargo run -p desicompass-bar -- --standalone     # in a window of its own
```

Nested in another desktop, that desktop keeps the notification and tray names
(the bar logs that they are taken). To try them, give the nested session a bus
of its own:

```bash
dbus-run-session -- ./target/debug/desicompass --backend auto --startup-cmd foot
```

SDL asks the desktop portal for settings as the bar and the superkey start, so
on a fresh private bus both take about half a minute to appear while the portal
is activated.

`tests/dbus.rs` runs the notification server and the tray against a
`dbus-daemon` of their own. It is skipped when there is none.
