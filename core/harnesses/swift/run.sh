#!/usr/bin/env bash
# Builds and runs the Swift smoke harness against the release staticlib, or against BRIGHTFX_LIB_DIR.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
core="$here/../.."

# BRIGHTFX_LIB_DIR: link this directory's libbrightfx_ffi instead of building
# one. The release workflow points it at the universal slice inside the
# xcframework, so the harness links the exact library a consumer gets.
# BRIGHTFX_HEADERS_DIR: an include directory passed straight to swiftc (-I),
# nothing copied. The release workflow points it at the xcframework's
# Headers/, the parent of CBrightFX/, so `import CBrightFX` resolves through
# clang's <include>/CBrightFX/module.modulemap lookup: the layout a consumer
# gets. Unset, the source tree's header and modulemap are used.
lib_dir="${BRIGHTFX_LIB_DIR:-}"
if [ -z "$lib_dir" ]; then
  cargo build --manifest-path "$core/Cargo.toml" -p brightfx-ffi --release
  lib_dir="$core/target/release"
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
if [ -n "${BRIGHTFX_HEADERS_DIR:-}" ]; then
  include="$BRIGHTFX_HEADERS_DIR"
else
  cp "$core/brightfx-ffi/include/brightfx.h" "$here/module.modulemap" "$work/"
  include="$work"
fi

echo "swift harness: linking $lib_dir, headers from $include"
swiftc "$here/main.swift" \
  -I "$include" \
  -L "$lib_dir" -lbrightfx_ffi \
  -o "$work/harness"

"$work/harness"
