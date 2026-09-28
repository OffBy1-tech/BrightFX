#!/usr/bin/env bash
# Builds and runs the Swift smoke harness against the release staticlib.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
core="$here/../.."

cargo build --manifest-path "$core/Cargo.toml" -p brightfx-ffi --release

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cp "$core/brightfx-ffi/include/brightfx.h" "$here/module.modulemap" "$work/"

swiftc "$here/main.swift" \
  -I "$work" \
  -L "$core/target/release" -lbrightfx_ffi \
  -o "$work/harness"

"$work/harness"
