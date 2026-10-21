#!/usr/bin/env bash
# Builds and runs the Swift smoke harness against target/release, or against BRIGHTFX_LIB_DIR.
# target/release holds both libbrightfx_ffi.a and .dylib, and the linker
# prefers the dylib, so the default run links dynamically. The release
# workflow's run links the xcframework slice, which holds only the .a.
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
# BRIGHTFX_SWIFT_ARCH: build the harness for this one architecture and run
# it as that architecture. Unset, swiftc builds for the host. Only x86_64 is
# accepted: the release workflow sets it to replay the universal slice's
# x86_64 half, which on an arm64 Mac runs under Rosetta. The target keeps
# the host compiler's macOS deployment target, so the two replays differ in
# architecture alone.
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

target_args=()
run=()
case "${BRIGHTFX_SWIFT_ARCH:-}" in
  "") ;;
  x86_64)
    deployment="$(swiftc -print-target-info | sed -n 's/^ *"triple": "[^"]*-apple-macosx\([0-9.]*\)".*/\1/p' | head -n 1)"
    [ -n "$deployment" ] || { echo "ERROR: could not read the macOS deployment target from swiftc -print-target-info" >&2; exit 1; }
    target_args=(-target "x86_64-apple-macosx$deployment")
    arch -x86_64 /usr/bin/true 2>/dev/null || {
      echo "ERROR: BRIGHTFX_SWIFT_ARCH=x86_64 needs Rosetta to run the harness; install it with: softwareupdate --install-rosetta --agree-to-license" >&2
      exit 1
    }
    run=(arch -x86_64)
    ;;
  *) echo "ERROR: BRIGHTFX_SWIFT_ARCH must be unset or x86_64, not \"$BRIGHTFX_SWIFT_ARCH\"" >&2; exit 1 ;;
esac

echo "swift harness: linking $lib_dir, headers from $include"
swiftc "$here/main.swift" \
  ${target_args[@]+"${target_args[@]}"} \
  -I "$include" \
  -L "$lib_dir" -lbrightfx_ffi \
  -o "$work/harness"

# The evidence of what ran: the binary's one architecture, and the
# architecture a process launched the same way reports.
echo "swift harness: built $(lipo -archs "$work/harness"), running as $(${run[@]+"${run[@]}"} uname -m)"
${run[@]+"${run[@]}"} "$work/harness"
