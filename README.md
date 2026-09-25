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

The keyboard layout comes from systemd-localed, which is where your OS installer
put it. `--xkb-layout`, `--xkb-variant`, `--xkb-model` and `--xkb-options`
override it.

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

The module takes the compositor, Sicompass and the login screen from the
revisions this flake pins. To build them from your own checkouts instead, set
`services.desicompass.package`, `services.desicompass.sicompassPackage` and
`services.desicompass.greeter.package`.

## A session for development

A second session runs what `cargo build` last produced, so you can test a
change on the real display and through the real login without rebuilding the
system:

```nix
services.desicompass.dev.enable = true;
services.desicompass.dev.checkout = "/home/alice/src/friendlyflow";
```

`checkout` is the directory that holds the `desicompass` and `sicompass`
checkouts side by side. The login screen then offers "Desicompass (dev)" next to
"Desicompass". It runs `target/debug/desicompass` and `target/debug/sicompass`
from those checkouts (set `dev.profile = "release"` for release builds), and
falls back to the installed version of either one you have not built. If the
dev build fails, you are back at the login screen and "Desicompass" still works. Its output goes to the journal:
`journalctl -t desicompass-dev -b`.

## Trying it without logging out

```bash
nix develop
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
