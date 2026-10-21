#!/usr/bin/env bash
# Replays the harnesses against the assets build-release-macos.sh wrote, so
# a release ships the bytes that were verified, not a lookalike build:
#
#   the Node harness against each unpacked wasm tarball (nodejs, then web)
#   the Swift harness against the xcframework's universal slice and headers,
#     once as arm64 and once as x86_64 (under Rosetta on an arm64 Mac)
#   the brightfx-js and brightfx-remotion tarballs installed together into a
#     scratch project, with remotion's peer dependencies at the ranges its
#     package.json declares: the brightfx-js consumer smoke, then an import
#     of brightfx-remotion checked for every name its source exports
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
  BRIGHTFX_WASM_PKG="$work/$target/package" node "$core/harnesses/node/smoke.mjs"
done

echo "==> Swift harness against BrightFX.xcframework.zip"
unzip -q "$out/BrightFX.xcframework.zip" -d "$work/xcf"
slices=("$work"/xcf/BrightFX.xcframework/macos-*)
# An unmatched glob stays literal, so one "slice" that is not a directory
# means there is none.
[ -d "${slices[0]}" ] || { echo "ERROR: BrightFX.xcframework has no macos-* slice directory" >&2; exit 1; }
[ ${#slices[@]} -eq 1 ] || { echo "ERROR: expected one macOS slice, found: ${slices[*]}" >&2; exit 1; }
slice="${slices[0]}"
for arch in arm64 x86_64; do lipo "$slice/libbrightfx_ffi.a" -verify_arch "$arch"; done
# arm64 first, by naming it: an unset BRIGHTFX_SWIFT_ARCH builds for the
# host, and this script runs on arm64 runners. The x86_64 replay needs
# Rosetta; run.sh fails with how to install it rather than skipping.
[ "$(uname -m)" = arm64 ] || { echo "ERROR: run this on an arm64 Mac; found $(uname -m)" >&2; exit 1; }
echo "--> arm64"
BRIGHTFX_LIB_DIR="$slice" BRIGHTFX_HEADERS_DIR="$slice/Headers" "$core/harnesses/swift/run.sh"
echo "--> x86_64"
BRIGHTFX_SWIFT_ARCH=x86_64 BRIGHTFX_LIB_DIR="$slice" BRIGHTFX_HEADERS_DIR="$slice/Headers" "$core/harnesses/swift/run.sh"

echo "==> brightfx-js-$ver.tgz and brightfx-remotion-$ver.tgz as a consumer installs them"
mkdir -p "$work/consumer"
cp "$core/scripts/consumer-smoke.mjs" "$core/harnesses/node/replay.mjs" "$work/consumer/"
remotion_pkg="$(cd "$core/../packages/brightfx-remotion" && pwd)"
# Peer dependencies as name@range, from the package.json the tarball was
# packed from, so this follows the declared ranges rather than a copy.
peers=()
while IFS= read -r peer; do peers+=("$peer"); done < <(
  node -e 'const p = require(process.argv[1]); for (const [n, r] of Object.entries(p.peerDependencies ?? {})) console.log(`${n}@${r}`)' \
    "$remotion_pkg/package.json")
[ ${#peers[@]} -gt 0 ] || { echo "ERROR: brightfx-remotion's package.json declares no peerDependencies" >&2; exit 1; }
echo "peer dependencies: ${peers[*]}"
# The names src/index.tsx exports as values; type-only exports (interface,
# type) leave nothing in the built module.
exports="$(sed -nE 's/^export (const|function|class|let) ([A-Za-z_$][A-Za-z0-9_$]*).*/\2/p' "$remotion_pkg/src/index.tsx" | sort | paste -sd ' ' -)"
[ -n "$exports" ] || { echo "ERROR: found no value exports in brightfx-remotion's src/index.tsx" >&2; exit 1; }
(cd "$work/consumer" \
  && npm init -y >/dev/null \
  && npm install --no-audit --no-fund "$out/brightfx-js-$ver.tgz" "$out/brightfx-remotion-$ver.tgz" "${peers[@]}" >/dev/null \
  && node consumer-smoke.mjs "$core/fixtures")
# brightfx-remotion must resolve brightfx-js to the tarball beside it, not
# a registry copy nested under it.
[ ! -e "$work/consumer/node_modules/brightfx-remotion/node_modules/brightfx-js" ] \
  || { echo "ERROR: brightfx-remotion installed its own brightfx-js instead of using brightfx-js-$ver.tgz" >&2; exit 1; }
echo "--> import(\"brightfx-remotion\") exports: $exports"
(cd "$work/consumer" && node -e '
  const want = process.argv[1].trim().split(" ");
  import("brightfx-remotion").then((m) => {
    const missing = want.filter((name) => !(name in m));
    if (missing.length > 0) {
      console.error(`ERROR: brightfx-remotion does not export: ${missing.join(", ")}`);
      process.exit(1);
    }
    console.log(`brightfx-remotion loaded, exports: ${Object.keys(m).sort().join(" ")}`);
  }, (error) => {
    console.error(`ERROR: import("brightfx-remotion") failed: ${error.stack ?? error}`);
    process.exit(1);
  });
' "$exports")

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
