#!/usr/bin/env bash
# Set the crate version in Cargo.toml.
# Usage: scripts/set-version.sh <version>
#   <version> must NOT have a leading 'v' (e.g. "0.2.0", not "v0.2.0")
set -euo pipefail

VERSION="${1:?Usage: $0 <version-without-v-prefix>}"
VERSION="${VERSION#v}"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

sed -i.bak -E "s/^version = \".*\"/version = \"${VERSION}\"/" "${ROOT}/Cargo.toml"
rm -f "${ROOT}/Cargo.toml.bak"

echo "✓ Set version to ${VERSION}"
