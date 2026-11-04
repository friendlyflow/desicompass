#!/usr/bin/env bash
#
# Build the release's packages into dist/:
#
#   desicompass_amd64.deb                          Debian, Ubuntu
#   desicompass.x86_64.rpm                         Fedora, openSUSE
#   desicompass-x86_64-unknown-linux-gnu.tar.xz    the binaries, unpack by hand
#   SHA256SUMS.txt
#
# release.yml runs this on ubuntu-22.04, after installing what its "Install
# build dependencies" step lists and running scripts/setup-freetype.sh. See
# docs/releasing.md.
#
# File names carry no version, so the README's releases/latest/download/ links
# never go stale. The version is inside each package.

set -euo pipefail
cd "$(dirname "$0")/.."

version="$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)"
test -n "$version" || { echo "::error::no version in Cargo.toml"; exit 1; }

# A release built from a tag has to be the version it says it is.
if [ "${GITHUB_REF_TYPE:-}" = tag ] && [ "${GITHUB_REF_NAME:-}" != "v$version" ]; then
    echo "::error::tag ${GITHUB_REF_NAME} does not match version $version in Cargo.toml"
    exit 1
fi

# Three separate programs, and the compositor on its own: cargo unifies features
# across what one command builds, and the compositor must never link SDL3 or
# Vulkan (see CLAUDE.md). bundled-sdl3 links SDL3 statically, so nothing here
# depends on a system SDL3, which most distributions do not package yet.
cargo build --release --locked -p desicompass
cargo build --release --locked -p desicompass-superkey -p desicompass-bar --features bundled-sdl3

binaries=(desicompass desicompass-superkey desicompass-bar)
for bin in "${binaries[@]}"; do
    path="target/release/$bin"
    deps="$(ldd "$path")"
    echo "--- $bin"
    echo "$deps"
    if grep -q 'not found' <<<"$deps"; then
        echo "::error::$bin needs a library the runner does not have"
        exit 1
    fi
    if grep -q 'libSDL3\|libfreetype' <<<"$deps"; then
        echo "::error::$bin links SDL3 or FreeType dynamically, which a user's machine may not have"
        exit 1
    fi
    "$path" --help >/dev/null
done

# The compositor must not have picked up the renderer's libraries.
if ldd target/release/desicompass | grep -q 'libvulkan\|libpng'; then
    echo "::error::the compositor links the renderer's libraries"
    exit 1
fi
# A vendored FreeType has no PNG support and needs no libpng, so this is what
# catches colour emoji silently disappearing.
for bin in desicompass-superkey desicompass-bar; do
    if ! ldd "target/release/$bin" | grep -q libpng; then
        echo "::error::$bin was built against a FreeType without PNG support"
        exit 1
    fi
done

rm -rf dist target/debian target/generate-rpm
mkdir -p dist

# The two package lists are in Cargo.toml, and tests/packaging.rs keeps them
# the same.
cargo deb --no-build -p desicompass --output dist/desicompass_amd64.deb
cargo generate-rpm --auto-req packaging/rpm-find-requires -o dist/desicompass.x86_64.rpm

listing="$(dpkg-deb -c dist/desicompass_amd64.deb)"
for file in usr/bin/desicompass usr/bin/desicompass-superkey usr/bin/desicompass-bar \
    usr/bin/desicompass-session usr/share/wayland-sessions/desicompass.desktop; do
    grep -q "\./$file\$" <<<"$listing" \
        || { echo "::error::the .deb does not ship $file"; exit 1; }
done
dpkg-deb -f dist/desicompass_amd64.deb Package Version Depends Recommends

archive=desicompass-x86_64-unknown-linux-gnu
stage="$(mktemp -d)/$archive"
mkdir -p "$stage"
for bin in "${binaries[@]}"; do
    strip -o "$stage/$bin" "target/release/$bin"
done
cp packaging/desicompass-session packaging/desicompass.desktop \
    LICENSE README.md CHANGELOG.md THIRD-PARTY-LICENSES.html "$stage/"
cp lib/lib_superkey/fonts/LICENSE-*.txt "$stage/"
tar -C "$(dirname "$stage")" -cJf "dist/$archive.tar.xz" "$archive"

(cd dist && sha256sum -- * > SHA256SUMS.txt && ls -la && cat SHA256SUMS.txt)
