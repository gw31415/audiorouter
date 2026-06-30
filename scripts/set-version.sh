#!/usr/bin/env bash
# Set the crate version across the whole workspace.
# Bumps [package].version in every workspace Cargo.toml AND the version
# requirement on intra-workspace path deps (audiorouter-*), so the crate
# remains publishable to crates.io after a bump.
# Usage: scripts/set-version.sh <version>
#   <version> must NOT have a leading 'v' (e.g. "0.2.0", not "v0.2.0")
set -euo pipefail

VERSION="${1:?Usage: $0 <version-without-v-prefix>}"
VERSION="${VERSION#v}"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

for toml in \
  "${ROOT}/Cargo.toml" \
  "${ROOT}/crates/audiorouter-core/Cargo.toml" \
  "${ROOT}/crates/audiorouter-dashboard/Cargo.toml"; do
  sed -i.bak -E \
    -e "s/^version = \".*\"/version = \"${VERSION}\"/" \
    -e "/^audiorouter-/ s/version = \"[^\"]*\"/version = \"=${VERSION}\"/" \
    "${toml}"
  rm -f "${toml}.bak"
done

echo "✓ Set workspace version to ${VERSION}"
