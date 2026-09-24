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
- The version lives in `[package] version` in `Cargo.toml`. `flake.nix` reads it
  from there, so there is only one version to bump.
- Linux only. The dev shell and every package are `x86_64-linux` and
  `aarch64-linux`.

## Code Style

Follow standard Rust idioms. Use `#[allow(...)]` sparingly and only when
justified. In `README.md`, do not use em dashes or semicolons. Use commas
instead, or split into separate sentences.

## Testing

- `cargo test`, and `cargo clippy --all-targets --features tty` so the TTY
  backend is linted as well. The `tty` feature is off by default so that the
  nested (winit) backend builds without libinput, libseat, udev and gbm.
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

## Architecture: the NixOS module

`nixosModules.default` lives in this repo's [flake.nix](flake.nix), and it
takes the `sicompass` and `loginsicompass` packages from flake inputs. It is
enabled in two steps (`services.desicompass.enable`, then `.greeter.enable`),
and the comments in the module explain each non-obvious line: the session
`.desktop` file has to reach `sessionPackages`, `systemPackages` *and*
`pathsToLink`, and `systemd-cat` and `dbus-run-session` are load-bearing.
The greeter side is documented in loginsicompass's `docs/greeter.md`.

## Releasing

A release is a `vX.Y.Z` tag on `main`. See `.claude/skills/release/SKILL.md`.
