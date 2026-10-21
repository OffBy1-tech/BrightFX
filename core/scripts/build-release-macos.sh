#!/usr/bin/env bash
# Builds every release asset that comes from macOS into <out-dir>:
#
#   brightfx-wasm-web-<ver>.tgz     wasm-pack --target web, npm pack
#   brightfx-wasm-nodejs-<ver>.tgz  wasm-pack --target nodejs, npm pack
#   brightfx-js-<ver>.tgz           the typed wrapper, web wasm bundled
#   brightfx-remotion-<ver>.tgz     the Remotion component
#   BrightFX.xcframework.zip        universal (arm64 + x86_64) static library,
#                                   brightfx.h and the CBrightFX modulemap
#   brightfx.h                      the committed cbindgen header
#
# Builds only; verify-release-macos.sh replays the harnesses against the
# result. Needs the aarch64-apple-darwin and x86_64-apple-darwin targets.
set -euo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
core="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
root="$(cd "$core/.." && pwd)"
[ $# -eq 1 ] || { echo "usage: $0 <out-dir>" >&2; exit 2; }
mkdir -p "$1"
out="$(cd "$1" && pwd)"
ver="$("$core/scripts/check-version.sh")"
echo "==> building release $ver into $out"

# wasm-pack names the tarball after the crate (brightfx-wasm-<ver>.tgz) for
# both targets; rename each before the next one overwrites it.
for target in web nodejs; do
  dir="pkg-${target/nodejs/node}"
  echo "==> wasm package: $target"
  wasm-pack build "$core/brightfx-wasm" --release --target "$target" --out-dir "$dir"
  (cd "$core/brightfx-wasm/$dir" && npm pack --pack-destination "$out" >/dev/null)
  mv "$out/brightfx-wasm-$ver.tgz" "$out/brightfx-wasm-$target-$ver.tgz"
done

# brightfx-js's build copies pkg-web (just built above) into the package.
echo "==> JS packages"
(cd "$root/packages" \
  && npm ci \
  && npm run build -w brightfx-js \
  && npm run build -w brightfx-remotion \
  && npm pack -w brightfx-js -w brightfx-remotion --pack-destination "$out" >/dev/null)

echo "==> xcframework"
for triple in aarch64-apple-darwin x86_64-apple-darwin; do
  cargo build --manifest-path "$core/Cargo.toml" -p brightfx-ffi --release --target "$triple"
done
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
mkdir -p "$stage/lib" "$stage/headers"
lipo -create \
  "$core/target/aarch64-apple-darwin/release/libbrightfx_ffi.a" \
  "$core/target/x86_64-apple-darwin/release/libbrightfx_ffi.a" \
  -output "$stage/lib/libbrightfx_ffi.a"
# The modulemap the Swift harness uses: a consumer writes `import CBrightFX`.
cp "$core/brightfx-ffi/include/brightfx.h" "$core/harnesses/swift/module.modulemap" "$stage/headers/"
xcodebuild -create-xcframework \
  -library "$stage/lib/libbrightfx_ffi.a" -headers "$stage/headers" \
  -output "$stage/BrightFX.xcframework" >/dev/null
rm -f "$out/BrightFX.xcframework.zip"
# --norsrc/--noextattr keep AppleDouble (._*) files out of the zip.
(cd "$stage" && ditto -c -k --norsrc --noextattr --keepParent BrightFX.xcframework "$out/BrightFX.xcframework.zip")

cp "$core/brightfx-ffi/include/brightfx.h" "$out/brightfx.h"

echo "==> built:"
ls -1 "$out"
