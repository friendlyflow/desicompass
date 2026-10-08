# The superkey

`desicompass-superkey` (`lib/lib_superkey`) is the list a bare Super tap opens:
the notifications, the open windows, the status the bar shows, the Sicompass
tutorial, the settings (accessibility, then the bar), the controls (suspend,
restart, shut down, log out), then every installed program. It is a
sicompass-ui client, like the login screen, and it never links the Sicompass
application. It links one provider crate, `sicompass-tutorial`, for the
Tutorial section: in a session the app leaves the tutorial out
(`programs::SESSION_OWNED_PROGRAMS`), and the superkey shows it instead.
Inside that section the superkey is not a launcher: `gui::follow_tutorial_mode`
turns `launcher_mode` off and leaves search for General mode, so the app's
keymap (Insert mode on the tutorial's inputs among it) works there, and turns
both back on when the cursor leaves the section. `launcher_window` stays on
throughout: Escape in General closes the superkey there too, and the keys only
the app has (tabs, undo and redo, the timeline) do nothing.

A bare Super tap only opens the superkey. While it is open the tap does
nothing (`State::open_superkey`), so a stray Super never loses the user's
place. Escape closes it.

What the user changes in the tutorial (a box ticked, a radio chosen, an input
edited) lives in the renderer's tree only, as it does in the app, so the
superkey must not throw that tree away. Inside the tutorial its `tick` reports
no change (the bar's status and the clock wait until the user is out of it),
and every frame the host copies the tutorial's rows into `Shared::tutorial`
(`gui::remember_tutorial`), which the provider builds the Tutorial section
from. An edit then lasts through leaving the section and hiding the superkey,
until the language changes.

This document is about how the compositor and the superkey work together. The
user-facing side is in the README.

## Who does what

| | Compositor (`src/superkey.rs`) | Superkey (`lib/lib_superkey`) |
|---|---|---|
| Starting | Finds it (`superkey::resolve_command`), runs it once, restarts it with a back-off, gives up after 5 starts in 60 s | Says `hello` |
| Keys | Detects the bare Super tap (`keybindings::SuperTap`), binds Super+W/C/S/B/N/T | Everything typed while it has the keyboard |
| Placing | Keeps its toplevel out of the tiler, the window list and the focus stack. Gives it the whole output, full screen, on top | Nothing: it is told its size |
| Showing | Sends `show`, moves the keyboard to it, maps it on its next frame | Opens the section in simple search, draws again |
| Hiding | Unmaps it, sends `hidden`, gives the keyboard back | Stops drawing (`AppRenderer::suspended`) |
| Windows | Sends the list, most recently used first, and again when it changes | Lists them. Enter sends `focus` |
| Programs | Starts them (`spawn`), as its own children | Finds them in the desktop entries. Enter sends `spawn` |
| Controls | Ends the session on `quit-session` | Runs `systemctl` itself for suspend, restart and shut down |
| Settings | Nothing | Reads and writes the shared accessibility object, and the bar's settings |
| Status | Nothing | Reads the bar's status file. Dismisses notifications and activates tray items over the session bus |

## Finding it

There is no flag and no option: the superkey is part of desicompass. The
compositor takes the command from `DESICOMPASS_SUPERKEY` when that is set,
and set but empty means no superkey. Otherwise it looks for
`desicompass-superkey` next to its own binary, then on `PATH`, and runs
without one if neither has it.

- The Nix package's wrapper sets `DESICOMPASS_SUPERKEY` to its own build, as a
  default.
- The NixOS module sets it empty for the login screen's compositor: a superkey
  there would let anyone start programs, or end the session, before signing
  in. A variable rather than a flag, so a compositor older than the superkey
  ignores it instead of refusing to start (an unknown flag once left the login
  screen black).
- `cargo build --workspace` puts both binaries in `target/<profile>/`, next to
  each other.

## The channel

The compositor makes two socketpairs before it starts the superkey:

1. One end of the first is inserted into the Wayland display as a client, and
   the other is handed to the superkey as `WAYLAND_SOCKET`. Every surface the
   superkey creates arrives from that `ClientId`, which is how the compositor
   knows its window. An app_id would not do: any client can claim any app_id.
2. The second carries the superkey protocol (`lib/lib_superkey_protocol`), its
   fd number in `DESICOMPASS_SUPERKEY_FD`. One JSON object per line, lines
   capped at 64 KiB. A line that does not decode is logged and skipped.

```text
compositor -> superkey
  {"type":"show","section":"root|notifications|windows|status|tutorial|settings|controls","windows":[{"id":3,"title":"foot","app_id":"foot","focused":true}]}
  {"type":"windows","windows":[...]}     the list changed while it is shown
  {"type":"hidden"}                      sent on every hide
superkey -> compositor
  {"type":"hello","version":1}
  {"type":"focus","id":3}
  {"type":"spawn","argv":["firefox"],"cwd":null}
  {"type":"hide"}
  {"type":"quit-session"}
```

When the compositor closes the channel, the superkey exits. When the superkey
exits, the compositor starts it again. The session never ends because of the
superkey, except through `quit-session`.

## Two things that are easy to break

**`SDL_VIDEO_DRIVER=wayland` is load-bearing.** SDL 3.4 first tries a
"preferred" Wayland start that wants `wp_fifo_v1`. This compositor does not
offer it, so SDL disconnects and connects again. libwayland unsets
`WAYLAND_SOCKET` on the first connect, so the second one would reach the public
socket as an ordinary client, and the superkey would be tiled like any window.
Naming the driver skips the preferred attempt.

**Hidden means unmapped, not destroyed.** `SDL_HideWindow` would destroy the
toplevel, and every show would then wait for a new one to be configured. So
the window stays, the compositor unmaps it, and the superkey stops drawing.
While it is unmapped the compositor still sends it frame callbacks
(`send_superkey_frame_if_unmapped`): a Vulkan client presenting in FIFO mode
waits for the callback of its previous frame before the next, and without them
the first frame after a show would never come. The show itself maps the window
on that first frame, or after 250 ms, whichever is first, so the previous
showing's list does not flash up.

## The settings it shows

The Settings section (Super+S) is a radio group each for the colour scheme and
the language, then two groups, Accessibility and Bar. The colour scheme and the
language are not accessibility, but they live in the same shared object below,
and the login screen shows them too.

**Bar** is the bar's own settings, a radio group for its position (bottom, top)
and checkboxes for seconds on its clock and for showing the keys being
pressed. They are the user's, in
`$XDG_CONFIG_HOME/desicompass/bar.json`, which the bar follows (see
[bar.md](bar.md)).

**Accessibility**, and the colour scheme and language above it, are the object
the login screen and every session share, `/var/lib/sicompass/accessibility.json`, which sicompass and
loginsicompass read and write too
(`sicompass_ui::accessibility::SharedAccessibility`). A change made in any of
them is written there atomically, under a lock, and the others follow it on
their next poll. Below it sit the machine's defaults,
`/etc/sicompass/accessibility.json`, which nothing writes at runtime.

Accessibility's rows are a checkbox for the screen reader and for
shoulder-surfing protection, and a radio group for the font scale. Enter ticks a
checkbox or chooses an option.

The superkey applies the display settings to itself (colour scheme, font scale,
shoulder-surfing protection, language), but never starts or stops a screen
reader. In the session, sicompass owns Orca, and a second owner would start a
second Orca.

## The status it shows

The Status section says in words what the bar's icons show: the date and time
(moved on the minute, so a focused clock row is not read out every second), the
network, the volume, the battery and Bluetooth when the machine has them, then
**Tray**. Super+B opens it. It reads them from the status file the bar writes,
`$XDG_RUNTIME_DIR/desicompass/status-<WAYLAND_DISPLAY>.json`, and so does the
**Notifications (n)** section, first in the root, which Super+N opens.

Each section's label in the root ends in its key (`Status [b]`), and the
provider recognises a label with or without it and whatever the count.

Enter on a notification dismisses it (`CloseNotification` to the bar, which is
the notification server), and "Dismiss all" all of them. Enter on a tray item
calls its `Activate` and closes the superkey. Both calls run on a thread of
their own (`status.rs`), so an item that hangs cannot freeze the list, and an
error is shown when it comes back.

## Trying it

In the dev shell:

```bash
cargo build --workspace
cargo run -- --backend auto --startup-cmd foot
cargo run -p desicompass-superkey -- --standalone     # on its own, Escape quits
```

The host desktop may take the Super tap for itself. Super+W, Super+C and
Super+S still reach a nested desicompass.

A nested run can be too slow for a Vulkan client. Seen on COSMIC with the
nested window off screen: desicompass answered its clients every two to three
seconds, and sicompass and the superkey both failed their first surface query
with "No suitable Vulkan GPU found" (the surface reported lost), while vkcube
still ran. The likely cause is the winit backend's loop waiting on the host's
frame callbacks, which a host throttles for a window nobody sees. Keep the
nested window on screen.
