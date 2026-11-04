#!/usr/bin/env bash
#
# Build a PNG-capable FreeType and print the PKG_CONFIG_PATH that finds it.
#
# The Linux half of sicompass's scripts/setup-freetype.sh, which has the whole
# story. Keep the version and the cmake flags in step with it.
#
# The renderer in the superkey and the bar draws colour emoji from
# NotoColorEmoji, whose strikes are PNG bitmaps. freetype-sys links the system
# FreeType only when pkg-config reports `freetype2 >= 24.3.18` (a libtool
# version), and otherwise falls back to a vendored build with PNG support off.
# That fallback does not error: every colour emoji silently disappears.
# Ubuntu 22.04's FreeType reports 24.1.18, and the release builds there for its
# glibc 2.35 floor, so a new enough one is built here.
#
# It is static, so the binaries do not depend on a copy that will not exist on
# a user's machine. libpng and zlib stay dynamic.
#
# In a workflow:  scripts/setup-freetype.sh >> "$GITHUB_ENV"
# Everything informational goes to stderr, so stdout is only `KEY=value` lines.

set -euo pipefail

FREETYPE_VERSION="${FREETYPE_VERSION:-2.13.3}"
PREFIX="${FREETYPE_PREFIX:-$HOME/.local/freetype}"
FLOOR="24.3.18"

log() { echo "$@" >&2; }

if [ -f "$PREFIX/lib/pkgconfig/freetype2.pc" ]; then
    log "Reusing FreeType already built at $PREFIX"
else
    log "Building FreeType $FREETYPE_VERSION from source into $PREFIX"
    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT

    # Savannah is the upstream home but returns 502 often enough to break a
    # build, so SourceForge is tried first. Both serve the same tarball.
    downloaded=""
    for url in \
        "https://downloads.sourceforge.net/project/freetype/freetype2/${FREETYPE_VERSION}/freetype-${FREETYPE_VERSION}.tar.xz" \
        "https://download.savannah.gnu.org/releases/freetype/freetype-${FREETYPE_VERSION}.tar.xz"
    do
        log "Fetching $url"
        if curl -fsSL --proto '=https' --tlsv1.2 --retry 3 --retry-delay 2 \
            "$url" -o "$tmp/freetype.tar.xz"; then
            downloaded="$url"
            break
        fi
        log "  failed, trying the next mirror"
    done
    [ -n "$downloaded" ] || { log "error: could not download FreeType ${FREETYPE_VERSION}"; exit 1; }

    tar -xf "$tmp/freetype.tar.xz" -C "$tmp"

    # REQUIRE on purpose: a silent "libpng not found, carrying on" would defeat
    # the point of this script.
    cmake -S "$tmp/freetype-${FREETYPE_VERSION}" -B "$tmp/build" \
        -DCMAKE_BUILD_TYPE=Release \
        -DCMAKE_INSTALL_PREFIX="$PREFIX" \
        -DCMAKE_INSTALL_LIBDIR=lib \
        -DCMAKE_POSITION_INDEPENDENT_CODE=ON \
        -DBUILD_SHARED_LIBS=OFF \
        -DFT_REQUIRE_PNG=ON \
        -DFT_REQUIRE_ZLIB=ON \
        -DFT_DISABLE_HARFBUZZ=ON \
        -DFT_DISABLE_BROTLI=ON \
        -DFT_DISABLE_BZIP2=ON >&2
    cmake --build "$tmp/build" -j"$(nproc)" >&2
    cmake --install "$tmp/build" >&2
fi

# Fail here rather than let the build quietly take the vendored path.
path="$PREFIX/lib/pkgconfig"
if ! PKG_CONFIG_PATH="$path" pkg-config --atleast-version="$FLOOR" freetype2; then
    log "error: freetype2.pc under $path is missing or below the $FLOOR floor."
    exit 1
fi
log "freetype2.pc at $path: $(PKG_CONFIG_PATH="$path" pkg-config --modversion freetype2) (floor $FLOOR)"
echo "PKG_CONFIG_PATH=$path:${PKG_CONFIG_PATH:-}"
