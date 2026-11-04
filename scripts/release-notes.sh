#!/usr/bin/env bash
#
# Print the GitHub release body for a version: the install table, then that
# version's section of CHANGELOG.md. Fails when the section is missing, because
# what would go out instead is an empty page.
#
# Usage: scripts/release-notes.sh 0.2.3

set -euo pipefail
cd "$(dirname "$0")/.."

version="$1"
tag="v$version"
base="https://github.com/friendlyflow/desicompass/releases/download/$tag"

changes="$(awk -v heading="## $version" '
    $0 == heading { found = 1; next }
    found && /^## / { exit }
    found { print }
' CHANGELOG.md)"
test -n "$(tr -d '[:space:]' <<<"$changes")" \
    || { echo "::error::CHANGELOG.md has no section for $version" >&2; exit 1; }

cat <<NOTES
## Install

| Platform | Download |
| --- | --- |
| Debian, Ubuntu | [desicompass_amd64.deb]($base/desicompass_amd64.deb) |
| Fedora, openSUSE | [desicompass.x86_64.rpm]($base/desicompass.x86_64.rpm) |
| Any Linux, binaries only | [desicompass-x86_64-unknown-linux-gnu.tar.xz]($base/desicompass-x86_64-unknown-linux-gnu.tar.xz) |
| Nix, NixOS | \`github:friendlyflow/desicompass/$tag\`, see the [README](https://github.com/friendlyflow/desicompass#install-on-nixos) |

The packages need [sicompass](https://github.com/friendlyflow/sicompass/releases/latest), so install the two together. The [README](https://github.com/friendlyflow/desicompass#install-on-debian-ubuntu-and-fedora) has the commands.

## What changed
$changes
NOTES
