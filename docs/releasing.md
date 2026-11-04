# Releasing

How a `vX.Y.Z` tag turns into installable packages. The steps to follow are the
`/release desicompass` skill. This is the background.

## What a release produces

One GitHub Release carrying:

| Asset | Built by | For |
| --- | --- | --- |
| `desicompass_amd64.deb` | cargo-deb | Debian 12, Ubuntu 22.04 and newer |
| `desicompass.x86_64.rpm` | cargo-generate-rpm | Fedora, openSUSE |
| `desicompass-x86_64-unknown-linux-gnu.tar.xz` | `tar` | anyone installing by hand |
| `SHA256SUMS.txt` | `sha256sum` | verification |

Each holds `desicompass`, `desicompass-superkey` and `desicompass-bar`, the
script a session entry starts, the session entry, and the license texts. Nix is
the other format and is not an asset: the flake builds from the tag.

The packages install the session and change nothing else. A display manager that
reads `/usr/share/wayland-sessions` offers "Desicompass" next to what it already
offers, and a session that fails costs a login attempt. It is step one of the
NixOS module's two steps. Step two, the login screen, is loginsicompass's.

## The pipeline

```
git push origin vX.Y.Z
   -> .github/workflows/release.yml
      |- packages        on ubuntu-22.04: scripts/setup-freetype.sh,
      |                  scripts/release-package.sh, scripts/release-notes.sh
      |- install-check   in a bare ubuntu:24.04: scripts/check-install.sh
      `- publish         gh release create, gh release upload
```

**Started by hand** (Actions -> Release -> Run workflow, or
`gh workflow run release.yml`) it runs everything but `publish`, and the
files stay on the run as an artifact. CI builds in the flake's toolchain, so this
is the only check that the Ubuntu build still works. The release skill runs it
before tagging.

`scripts/release-package.sh` refuses a tag that is not the version in
`Cargo.toml`, and `ldd`s every binary before packaging it. It stops on a library
the runner lacks, on SDL3 or FreeType linked dynamically, on the compositor
linking the renderer's libraries, and on a superkey or bar without libpng, which
means a FreeType with no PNG support and no colour emoji.

`scripts/check-install.sh` then installs the `.deb` with `--no-install-recommends`
in a bare container, next to the latest sicompass, and starts the session nested in
Xvfb on software Vulkan. It needs the bar and the superkey to report their windows,
and nothing is published otherwise. A container and not the runner, because the
runner, like every desktop, already has the libraries that are the question: the
first package lacked libwayland-server, which is dlopened, and exited with
`NoWaylandLib` on a clean system. It cannot test `--backend tty`, which needs a real
seat, so that part of the smoke test stays manual.

## How it differs from sicompass, and why

The same runner, the same glibc 2.35 floor, the same FreeType and the same static
SDL3 as sicompass's Linux packages. Three differences:

- **No cargo-dist.** It exists to cover four targets and an MSI, and it generates
  a workflow that cannot be edited by hand. This is one target and a short
  workflow.
- **cargo-deb instead of cargo-packager.** cargo-packager cannot emit maintainer
  scripts (sicompass repacks its `.deb` to add them), and the AppImage is what it
  would otherwise be here for. cargo-deb takes the scripts directly, and its asset
  list reads like cargo-generate-rpm's, which `tests/packaging.rs` compares.
- **No AppImage.** An AppImage is for running a program without installing it. A
  session has to be installed: a display manager finds it by a file in
  `/usr/share/wayland-sessions`.

**File names carry no version.** sicompass's `.deb`, `.rpm` and AppImage do, so
its README's `releases/latest/download/` links have to be rewritten at every
release, and 404 when someone forgets. Here they never go stale.

## What the packages assume

`[package.metadata.deb]` and `[package.metadata.generate-rpm]` in `Cargo.toml`
list the files, and the comments above them explain the dependencies. Things no
compiler checks:

- **The session script mirrors the NixOS module.** `packaging/desicompass-session`
  is `mkSessionPackage` in `flake.nix` without the store paths. A change to one
  belongs in the other. `tests/packaging.rs` holds the load-bearing words.
- **`sicompass (>= X)` follows the superkey's pin.** The superkey's Store checks
  plugins against the sicompass it was built with. `tests/packaging.rs` compares
  the floor, the pin and the flake's input once the pin is a tag.
- **Dlopened libraries have to be declared by hand.** libEGL, libGLESv2, the
  Vulkan loader and four libwayland libraries show up in no scan. A new one is found
  by `scripts/check-install.sh`.
- **The `.rpm`'s requirements come from `packaging/rpm-find-requires`,** because
  cargo-generate-rpm's own scan uses `ldd`, which also lists what the libraries
  link. On Ubuntu that brought in `libpcre.so.3` through libinput and glib, which
  Fedora does not have, and the package could not be installed.
- **`packaging/scripts/postinst` is both package managers' script.** It reads
  dpkg's and rpm's arguments to tell a first install from an upgrade, and only a
  first install adds people to `sicompass-a11y`.
- **`scripts/setup-freetype.sh` is a copy** of the Linux half of sicompass's.
- **`release.yml` names the build's Ubuntu packages**, and nothing else does. The
  flake's list is a different list, for a different toolchain.

## Smoke test

A green run proves that a clean install starts nested. Check the real thing, on a
machine you can get back into, or a virtual machine with a GPU.

What `check-install.sh` cannot see is a real display and seat. So:

- **Debian or Ubuntu**: `sudo apt install ./sicompass_*_amd64.deb
  ./desicompass_amd64.deb`, log out, and pick Desicompass. The window list, the
  bar and `sicompass` should appear, and Super should open the superkey.
- **Fedora**: the same with `dnf` and the `.rpm`s.
- **After either**: `journalctl -t desicompass -b` holds the session's output,
  `id` shows `sicompass-a11y` after logging in again, and a setting changed in the
  superkey is in `/var/lib/sicompass/accessibility.json`.
- **With Orca on**: the session speaks. This is what `at-spi2-core` and
  `dbus-run-session` are in the package for.
