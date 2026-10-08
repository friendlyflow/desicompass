# desicompass

*A keyboard-driven tiling Wayland compositor for Sicompass.*

desicompass is part of [Sicompass](https://github.com/friendlyflow/sicompass), a
keyboard-first, accessibility-first way to use your entire computer. It is the
session that Sicompass runs in when it is your whole desktop rather than one
window among others.

It has no mouse pointer at all. It advertises no `wl_pointer`, it places every
window itself, and every interaction is a key. It is built on
[smithay](https://github.com/Smithay/smithay).

## Keys

All bindings hold Super. Sicompass uses Alt for its own shortcuts, and on most
non-US layouts the right Alt is AltGr, which you need to type characters like
`@`, `#` and `{`.

| Keys | Action |
|---|---|
| `Super+J` / `Super+K` | Focus the next or previous window |
| `Super+H` / `Super+L` | Focus left or right |
| `Super+Shift+H/J/K/L` | Move the focused window |
| `Super+Tab` | Focus the least recently used window |
| `Super+M` | Switch between columns and monocle |
| `Super+Return` | Open a terminal (`--terminal`, `foot` by default) |
| `Super+Shift+Q` | Close the focused window |
| `Super+Shift+E` | End the session (press it twice) |
| `Super`, tapped on its own | Open the superkey (Escape closes it) |
| `Super+W` | Open the superkey on the open windows |
| `Super+C` | Open the superkey on the controls (suspend, restart, shut down, log out) |
| `Super+S` | Open the superkey on the Store. Twice quickly, on the settings (accessibility, then the bar) |
| `Super+B` | Open the superkey on the status the bar shows |
| `Super+N` | Open the superkey on the notifications |
| `Super+T` | Open the superkey on the tutorial |
| `Super+D` | Say the time and the date out loud |

The keyboard layout comes from systemd-localed, which is where your OS installer
put it. `--xkb-layout`, `--xkb-variant`, `--xkb-model` and `--xkb-options`
override it.

## The superkey

The superkey is a screen of its own, full screen over the windows. It is a list you
search by typing, the way Sicompass searches, and it opens straight into search.
From the top it holds the sections below, each with the key that opens it with
Super held after its name (`Status [b]`):

- **Notifications**, with how many there are. Enter dismisses one.
- **Windows**, most recently used first. `Super+W` then Enter goes back to the
  window you used before this one.
- **Status**: what the bar's icons show, in words. The date and time, the
  network, the volume, the battery and Bluetooth, then **Tray**, where Enter
  activates an item.
- **Tutorial**: the Sicompass tutorial. In a desicompass session it is here
  rather than in Sicompass. Inside it the superkey works like Sicompass, in
  General mode, because those are the keys it teaches. Tabs, undo and the
  timeline are the exceptions, since the superkey keeps nothing. Left at its
  top goes back to the list and to searching, and Escape closes the superkey.
- **Store**: the Sicompass Store, where programs are installed, updated and
  removed. In a desicompass session it is here rather than in Sicompass, which
  starts what you install within a second. Inside it the superkey works like
  Sicompass too.
- **Settings**: the colour scheme and the language, then two groups. Press
  `Super+S` twice quickly to open them.
  **Accessibility** holds the screen reader, the font scale and
  shoulder-surfing protection. **Bar** holds where the bar sits, whether its
  clock shows seconds and whether it shows the keys you press.
- **Controls**: suspend, restart, shut down and log out.
- every installed program, by name.

Enter does what the row is for: it focuses a window, starts a program, runs a
control or changes a setting. On a section it opens the section, and you go on
typing inside it. Escape closes the superkey and gives the keyboard back to the
window that had it.

It is its own program, `desicompass-superkey` in `lib/lib_superkey`, drawn by
the same renderer as Sicompass, and part of desicompass: there is nothing to
turn on. desicompass starts it once and shows and hides it, so it opens at
once. The Nix package brings its own, and a `cargo build --workspace` puts it
next to the compositor, where desicompass finds it. The login screen runs
without it. How the two
talk is in [docs/superkey.md](docs/superkey.md). To try it on its own, run
`cargo run -p desicompass-superkey -- --standalone`.

## The bar

The bar is a strip along the bottom of the screen, or the top if you choose so
in Settings > Bar (`Super+S` twice). It is 1.7 lines tall, with its line in the middle, and drawn in the colours of
the list's focused row. The windows are tiled in the rest of the screen.

The date and time sit at the right, with seconds if you turn them on. To their
left are the notifications (a bell and how many there are), the battery, the
volume, Bluetooth, the network and then the tray icons of programs like Dropbox.
Something that is off or wrong has an icon of its own: a slash through
Bluetooth that is off or a network that is not connected, a crossed speaker when
muted, an exclamation mark on a network that does not reach the internet. A
battery, Bluetooth adapter or network service that the machine does not have
shows no icon at all.

With **show key strokes** checked in Settings > Bar, the bar shows the keys
you press, like Showmethekey: letters as they are typed, other keys by name
(`Enter`, `Esc`, arrows) and chords as `Ctrl+C` or `Super+J`, a key pressed
again in a row counted as `a×3`. They take the left half of the bar at twice
the text size, and fade two and a half seconds after the last one. The bar is
2.7 lines tall then, and the status icons keep to its right half. Everything
you type shows, passwords included, so turn it off before typing one in front
of others. Shoulder-surfing protection blanks the bar and hides them too.

`Super+D` says the time and the date out loud through speech-dispatcher, whether or not a
screen reader is running. What the icons show is also in the superkey's Status
section (`Super+B`) and its notifications (`Super+N`), which is how you reach it by keyboard and with a screen reader.

The bar is where that status comes from. It follows NetworkManager, UPower,
BlueZ and WirePlumber (`wpctl`), and it is the session's notification server
and tray. Notifications do not pop up: they are kept until you dismiss them.
Tray items need to run inside the session, started from Sicompass or the
superkey, because the session has a D-Bus session bus of its own.

It is its own program too, `desicompass-bar` in `lib/lib_bar`, started by
desicompass like the superkey. The login screen runs without it. How it works
is in [docs/bar.md](docs/bar.md). To try it on its own, run
`cargo run -p desicompass-bar -- --standalone`.

## Install on NixOS

The flake has a NixOS module. Turn it on in two steps, in this order:

```nix
{
  inputs.desicompass.url = "github:friendlyflow/desicompass";

  # in your NixOS configuration
  imports = [ inputs.desicompass.nixosModules.default ];

  # Step 1: adds "Desicompass" to the session list of the login screen you
  # already have. If the session fails, you are back at that login screen.
  services.desicompass.enable = true;

  # Step 2, only once step 1 works: replaces the login screen itself with
  # loginsicompass, the accessible Sicompass login screen. A login screen that
  # fails to start leaves no graphical way in, so recovery is a text console
  # and `nixos-rebuild --rollback`.
  # services.desicompass.greeter.enable = true;
}
```

The session's output goes to the journal: `journalctl -t desicompass -b`.

The module takes the compositor (with its superkey and bar), Sicompass and the login
screen from the revisions this flake pins. To build them from your own checkouts
instead, set `services.desicompass.package`,
`services.desicompass.sicompassPackage` and
`services.desicompass.greeter.package`.

## Screen readers and the keyboard

desicompass hands the keyboard to Orca the way GNOME and COSMIC do, through the
`org.freedesktop.a11y.KeyboardMonitor` D-Bus interface on the session bus. Orca
hears every key, so a key press stops what it is saying, and its own commands
(the Orca key, typing echo, Ctrl to stop speech) work. Only Orca may use the
interface. The code is `src/a11y_keyboard_monitor.rs`, ported from cosmic-comp.

## Accessibility defaults

The login screen and Sicompass share one set of accessibility defaults:

```nix
services.desicompass.accessibility = {
  screenReader = true;       # Orca at the login screen and in the session
  fontScale = "2.00";        # "1.00" to "2.50" in steps of 0.25
  colorScheme = "light";     # "dark" or "light"
  language = "nl-BE";        # "en-US", "nl-BE", "fr-BE" or "de-BE"
  shoulderSurfingProtection = false;
};
```

Every option is optional. They are written to `/etc/sicompass/accessibility.json`,
and they only fill in what nobody has chosen. Nothing ever writes that file
while the machine runs.

A choice made at the login screen, in the superkey's Settings or in Sicompass's
own goes to `/var/lib/sicompass/accessibility.json`. That is one object for the
whole machine, shared both ways: what you choose at the login screen is what the
session starts with, and what you choose in a session is what the login screen
shows next time. Every program that shows these settings follows a change made
in another within a quarter of a second. The module creates the directory,
writable by a `sicompass-a11y` group that holds the login screen and every
normal user (a user added to the machine has it from their next login).
Sicompass on another desktop keeps these settings in its own `settings.json`, as
before.

A value is taken from that shared file first, then `/etc`, then the program's
own default.

Left unset, the login screen starts Orca the first time it runs, until someone
turns it off there. Sicompass is the one that starts Orca in the session.

On another distribution, write that file by hand. The login screen's
`docs/greeter.md` lists its keys.

## A session for development

A second session runs a second pair of packages next to the stable one. The
usual setup is the last release as "Desicompass" and your working trees as
"Desicompass (dev)", both built by `nixos-rebuild switch`:

```nix
let
  release = builtins.getFlake "github:friendlyflow/desicompass/v0.2.0";
  desicompass = builtins.getFlake "git+file:///home/alice/src/friendlyflow/desicompass";
  sicompass = builtins.getFlake "git+file:///home/alice/src/friendlyflow/sicompass";
  system = "x86_64-linux";
in
{
  imports = [ desicompass.nixosModules.default ];

  services.desicompass.package = release.packages.${system}.desicompass;
  services.desicompass.sicompassPackage = release.inputs.sicompass.packages.${system}.default;
  services.desicompass.greeter.package = release.inputs.loginsicompass.packages.${system}.default;

  services.desicompass.dev.enable = true;
  services.desicompass.dev.sicompassPackage = sicompass.packages.${system}.default;
}
```

`dev.package` defaults to the desicompass the module was imported from, here
the working tree, superkey included. Use `git+file://` for a working tree, never a plain path. A plain
path copies `target/` into the store. `git+file://` includes uncommitted edits
to files git tracks, but not files git does not track yet.

The login screen then offers "Desicompass (dev)" next to "Desicompass". If the
dev build misbehaves, you are back at the login screen and "Desicompass" still
works. Its output goes to the journal: `journalctl -t desicompass-dev -b`.

## Trying it without logging out

```bash
nix develop
cargo build --workspace
cargo run -- --backend auto --startup-cmd foot
```

`--backend auto` runs desicompass in a window inside your current session when
`WAYLAND_DISPLAY` or `DISPLAY` is set, and takes over the display otherwise. The
dev shell includes three test clients, cheapest first: `wayland-info` lists what
the compositor advertises, `foot` is a terminal that needs no GPU, and `vkcube`
is the smallest Vulkan client there is.

## Building from source

```bash
nix develop                     # optional, brings the whole toolchain
cargo build --release
cargo test
```

The repository is a Cargo workspace: the compositor at the root, the superkey in
`lib/lib_superkey` and the bar in `lib/lib_bar`, each with the protocol it
speaks with the compositor beside it (`lib/lib_superkey_protocol`,
`lib/lib_bar_protocol`). `cargo test` covers all five. `nix build` builds the
compositor, `nix build .#desicompass-superkey` the superkey and
`nix build .#desicompass-bar` the bar.

Every build has both backends, the nested one and the one that takes over a real
display (DRM/KMS, libinput, libseat), so it needs those libraries. The dev shell
has them. `nix build` builds the packaged version.

desicompass runs on Linux only.

## Related repositories

- [sicompass](https://github.com/friendlyflow/sicompass), the application that
  runs inside this session
- [loginsicompass](https://github.com/friendlyflow/loginsicompass), the login
  screen

## Community

Join the conversation on
[Discord](https://discord.com/channels/1464152138753249313/1464152139231137894).

## License

#### Open source license

If you are creating an open source application under a license compatible with
the GNU GPL license v3, you may use this project under the terms of the GPLv3.
See [LICENSE](LICENSE).

## Contributing

Contributions are welcome. Whether it is code, documentation, or feedback, your
input helps make computing more accessible for everyone.
