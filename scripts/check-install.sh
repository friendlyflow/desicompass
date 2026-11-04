#!/usr/bin/env bash
#
# Install the .deb on a bare Ubuntu with nothing but what it declares, and start
# the session. release.yml runs this in a container, and it has to stay one: a
# runner, or anyone's desktop, already has the libraries that are the question.
# libwayland-server is dlopened, so nothing but this notices a package that does
# not declare it, and then the compositor exits with `NoWaylandLib`.
#
# Usage, as root: scripts/check-install.sh dist/desicompass_amd64.deb

set -euo pipefail

deb="$(realpath "$1")"
export DEBIAN_FRONTEND=noninteractive

apt-get update -qq
apt-get install -y -qq --no-install-recommends curl ca-certificates >/dev/null

# The sicompass package whose file name carries its version. Its release is out
# before this one, see the order in the release skill.
latest="$(curl -fsSLI -o /dev/null -w '%{url_effective}' https://github.com/friendlyflow/sicompass/releases/latest)"
tag="${latest##*/}"
sicompass="$(mktemp -d)/sicompass_${tag#v}_amd64.deb"
curl -fsSL -o "$sicompass" \
    "https://github.com/friendlyflow/sicompass/releases/download/$tag/$(basename "$sicompass")"

# --no-install-recommends, so what is declared is all there is.
apt-get install -y -qq --no-install-recommends "$sicompass" "$deb" >/dev/null
dpkg -l sicompass desicompass | tail -2

getent group sicompass-a11y >/dev/null \
    || { echo "::error::the install did not create the sicompass-a11y group"; exit 1; }
test "$(stat -c '%a %G' /var/lib/sicompass)" = "2775 sicompass-a11y" \
    || { echo "::error::/var/lib/sicompass has the wrong owner or mode"; exit 1; }
sh -n /usr/bin/desicompass-session

# There is no display to take over here, so the session runs nested in Xvfb.
# Only the nested X11 backend loads these, and a real session never does, which
# is why the package does not declare them.
apt-get install -y -qq --no-install-recommends \
    xvfb xauth libxkbcommon-x11-0 libxcursor1 libxi6 libxrandr2 libx11-xcb1 >/dev/null

export XDG_RUNTIME_DIR=/tmp/check-install
mkdir -p "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"
log="$(mktemp)"
RUST_LOG=info timeout -k 5 25 xvfb-run -a dbus-run-session \
    desicompass --backend auto --startup-cmd "sicompass --session" >"$log" 2>&1 || true
cat "$log"

for line in "the bar's window is up" "the superkey's window is up"; do
    grep -q "$line" "$log" \
        || { echo "::error::a clean install never reported: $line"; exit 1; }
done
echo "A clean install starts the session."
