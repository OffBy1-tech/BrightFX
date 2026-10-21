#!/usr/bin/env bash
# Builds and runs the Swift smoke harness against the release staticlib, or against BRIGHTFX_LIB_DIR.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
core="$here/../.."

# BRIGHTFX_LIB_DIR: link this directory's libbrightfx_ffi instead of building
# one. The release workflow points it at the universal slice inside the
# xcframework, so the harness links the exact library a consumer gets.
# BRIGHTFX_HEADERS_DIR: take brightfx.h and module.modulemap from here (the
# xcframework's Headers) instead of the source tree.
lib_dir="${BRIGHTFX_LIB_DIR:-}"
if [ -z "$lib_dir" ]; then
  cargo build --manifest-path "$core/Cargo.toml" -p brightfx-ffi --release
  lib_dir="$core/target/release"
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
if [ -n "${BRIGHTFX_HEADERS_DIR:-}" ]; then
  cp "$BRIGHTFX_HEADERS_DIR/brightfx.h" "$BRIGHTFX_HEADERS_DIR/module.modulemap" "$work/"
else
  cp "$core/brightfx-ffi/include/brightfx.h" "$here/module.modulemap" "$work/"
fi

echo "swift harness: linking $lib_dir"
swiftc "$here/main.swift" \
  -I "$work" \
  -L "$lib_dir" -lbrightfx_ffi \
  -o "$work/harness"

"$work/harness"
