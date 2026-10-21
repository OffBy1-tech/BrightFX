#!/usr/bin/env bash
# Replays the harnesses against the assets build-release-macos.sh wrote, so
# a release ships the bytes that were verified, not a lookalike build:
#
#   the Node harness against each unpacked wasm tarball (nodejs, then web)
#   the Swift harness against the xcframework's universal slice and headers
#   the brightfx-js tarball installed into a scratch project
#   the header against the committed one, and the remotion tarball's range
#   a LICENSE in every tarball and in the xcframework zip
set -euo pipefail
core="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
[ $# -eq 1 ] || { echo "usage: $0 <out-dir>" >&2; exit 2; }
out="$(cd "$1" && pwd)"
ver="$("$core/scripts/check-version.sh")"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

for target in nodejs web; do
  echo "==> Node harness against brightfx-wasm-$target-$ver.tgz"
  mkdir -p "$work/$target"
  tar -xzf "$out/brightfx-wasm-$target-$ver.tgz" -C "$work/$target"
  BRIGHTFX_WASM_PKG="$work/$target/package" BRIGHTFX_WASM_TARGET="$target" \
    node "$core/harnesses/node/smoke.mjs"
done

echo "==> Swift harness against BrightFX.xcframework.zip"
unzip -q "$out/BrightFX.xcframework.zip" -d "$work/xcf"
slices=("$work"/xcf/BrightFX.xcframework/macos-*)
[ ${#slices[@]} -eq 1 ] || { echo "ERROR: expected one macOS slice, found: ${slices[*]}" >&2; exit 1; }
slice="${slices[0]}"
for arch in arm64 x86_64; do lipo "$slice/libbrightfx_ffi.a" -verify_arch "$arch"; done
BRIGHTFX_LIB_DIR="$slice" BRIGHTFX_HEADERS_DIR="$slice/Headers" "$core/harnesses/swift/run.sh"

echo "==> brightfx-js-$ver.tgz as a consumer installs it"
mkdir -p "$work/consumer"
cp "$core/scripts/consumer-smoke.mjs" "$work/consumer/"
(cd "$work/consumer" \
  && npm init -y >/dev/null \
  && npm install --no-audit --no-fund "$out/brightfx-js-$ver.tgz" >/dev/null \
  && node consumer-smoke.mjs "$core/fixtures")

echo "==> brightfx-remotion-$ver.tgz depends on brightfx-js ^$ver"
range="$(tar -xzOf "$out/brightfx-remotion-$ver.tgz" package/package.json | node -p 'JSON.parse(require("fs").readFileSync(0, "utf8")).dependencies["brightfx-js"]')"
[ "$range" = "^$ver" ] || { echo "ERROR: brightfx-remotion's tarball depends on brightfx-js \"$range\"" >&2; exit 1; }

echo "==> every asset carries LICENSE"
for tgz in brightfx-wasm-web brightfx-wasm-nodejs brightfx-js brightfx-remotion; do
  tar -tzf "$out/$tgz-$ver.tgz" | grep -x 'package/LICENSE' >/dev/null \
    || { echo "ERROR: $tgz-$ver.tgz has no package/LICENSE" >&2; exit 1; }
done
zipinfo -1 "$out/BrightFX.xcframework.zip" | grep -x 'LICENSE' >/dev/null \
  || { echo "ERROR: BrightFX.xcframework.zip has no top-level LICENSE" >&2; exit 1; }

echo "==> brightfx.h is the committed header"
cmp "$out/brightfx.h" "$core/brightfx-ffi/include/brightfx.h"

echo
echo "release $ver verified"
