# Project Instructions

desicompass was split out of the
[sicompass](https://github.com/friendlyflow/sicompass) workspace, and its git
history before that point is the history of `src/desicompass` there (and,
before a rename, of `src/desicompass-rs` for the Rust port and `src/desicompass`
for the C original, which later moved to `legacy-c/`). Work on it is usually
driven from a sicompass checkout next to this one (`../sicompass`), whose
`/commit-and-push`, `/release`, `/sync` and `/update-cargo` take this repo's name
as their first argument and then follow the skills in this repo's
`.claude/skills/`.

## Environment (Nix)

The toolchain comes from the flake dev shell in [flake.nix](flake.nix). Nothing
is installed system-wide.

- **Check once per session**, then stick with the answer: `command -v cargo`.
  - Non-empty: the shell is inside `nix develop`, so run `cargo ...` directly.
  - Empty: prefix every toolchain command with `nix develop -c`.
- `nix develop -c <cmd>` prints a `warning: Git tree ... is dirty` line on
  stderr first. That warning is noise, not a failure.
- Evaluate the flake through `git+file://$PWD`, never a plain path (a plain path
  copies `target/` into the store and hangs), and always under `timeout`.
- The version lives in `[workspace.package] version` in the root `Cargo.toml`.
  `flake.nix` reads it from there, so there is only one version to bump.
- The repo is a Cargo workspace. `cargo test -p` takes the package name, which
  differs from the directory: `lib/lib_<x>` is `desicompass-<x>` (with dashes:
  `lib/lib_superkey_protocol` is `desicompass-superkey-protocol`,
  `lib/lib_bar` is `desicompass-bar`). The root package is `desicompass`.
  `cargo test` and `cargo clippy` with no `-p` cover all five
  (`default-members`).
- Linux only. The dev shell and every package are `x86_64-linux` and
  `aarch64-linux`.

## Generated files that are committed

- `THIRD-PARTY-LICENSES.html`: `cargo about generate about.hbs -o
  THIRD-PARTY-LICENSES.html` (cargo-about 0.9.2, the version the `licenses.yml`
  workflow pins). Regenerate and commit it with any dependency change. The
  workflow fails if it drifts.

## Code Style

Follow standard Rust idioms. Use `#[allow(...)]` sparingly and only when
justified. In `README.md`, do not use em dashes or semicolons. Use commas
instead, or split into separate sentences.

## Testing

- `cargo test` and `cargo clippy --all-targets`. Both backends are always
  built, so libinput, libseat, udev and gbm have to be present: run cargo in
  the dev shell.
- Behaviour is checked in a nested run: `cargo run -- --backend auto
  --startup-cmd foot`, with `wayland-info`, `foot` and `vkcube` from the dev
  shell as test clients, cheapest first.
- A change to the NixOS module is checked with `nixos-rebuild build` on a
  configuration that imports it, before anyone switches to it.
- After implementing changes, always run the tests before finishing.
- When adding new code, write or update tests.
- If tests fail, fix the code. Never leave a task with failing tests.

## Test Integrity

- Never remove or weaken test assertions to make a failing test pass. Fix the
  code instead.
- If a test itself is genuinely wrong and needs changing, **ask the user
  first** before modifying it.

## Architecture: the Mesa vendor (hard rule)

The package wrapper and the dev shell put only the GL/EGL/GBM *dispatch*
libraries (libglvnd, libgbm) on `LD_LIBRARY_PATH`, and point the *vendor* at
`/run/opengl-driver` with `--set-default`. Never add nixpkgs' `mesa` to either
path. Two Mesa builds in one process segfault on the first call across the
boundary, and only on the TTY/GBM path, so a nested run does not catch it.

## Architecture: the superkey

`desicompass-superkey` (`lib/lib_superkey`) is a sicompass-ui client, found
and started once by the compositor (no flag, see `superkey::resolve_command`)
and shown and hidden by it. See
[docs/superkey.md](docs/superkey.md). Three rules:

- **The compositor never depends on the superkey crate.** They share only
  `lib/lib_superkey_protocol`, which depends on serde alone. Cargo unifies
  features across a workspace, and the superkey links SDL3 and Vulkan. The
  compositor's package builds with `-p desicompass` for the same reason.
- **The superkey's window is known by its `ClientId`**, from the Wayland
  socketpair the compositor inserted, never by its app_id. Its toplevel stays
  out of `windows`, the tiler and the focus stack.
- **The superkey never starts or stops Orca.** In a session, sicompass owns the
  screen reader.
- **The module never passes the compositor, the greeter or the superkey a flag
  an older release lacks.** The stable session and the login screen often run
  a release while the module comes from a working tree, and an unknown flag
  stops the binary: that is how the login screen once went black. Pass new
  settings in the environment instead, which older binaries ignore.

The `[patch]` sections for working on sicompass-ui and the SDK together sit at
the bottom of the root `Cargo.toml`, the workspace root being the only place
cargo honours them. Comment them out again before committing.

## Architecture: the bar

`desicompass-bar` (`lib/lib_bar`) is the strip with the clock and the status
icons, started, placed and restarted by the compositor like the superkey
(`src/bar.rs`, sharing `src/managed_client.rs`). It is also the session's
notification server, its tray, and the source of the status the superkey's
Status section lists. See [docs/bar.md](docs/bar.md). The superkey's rules hold
for it too, and:

- **Nothing depends on the bar crate.** The compositor and the superkey share
  only `lib/lib_bar_protocol` with it (serde and libc): the channel, the
  settings file (`bar.json`) and the status file.
- **The bar never has the keyboard**, so it has no AccessKit tree. Everything
  it shows must also be in the superkey's Status section, which is how a
  screen reader reaches it.
- **It never starts or stops Orca.** Super+D speaks through `spd-say`, which
  shares speech-dispatcher with Orca.
- **The tiles get `State::usable_area()`, never `output_size()`**, so nothing
  but the superkey covers the bar.
- **Tests that touch D-Bus run on a private `dbus-daemon`** (`tests/dbus.rs`),
  never on the session bus the tests run in.

## Architecture: the NixOS module

`nixosModules.default` lives in this repo's [flake.nix](flake.nix), and it
takes the `sicompass` and `loginsicompass` packages from the flake inputs of
the same names. It is
enabled in two steps (`services.desicompass.enable`, then `.greeter.enable`),
and the comments in the module explain each non-obvious line: the session
`.desktop` file has to reach `sessionPackages`, `systemPackages` *and*
`pathsToLink`, and `systemd-cat` and `dbus-run-session` are load-bearing.
The greeter side is documented in loginsicompass's `docs/greeter.md`.

## Releasing

A release is a `vX.Y.Z` tag on `main`. See `.claude/skills/release/SKILL.md`.
