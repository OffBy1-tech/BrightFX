#!/usr/bin/env bash
# Builds every release asset that comes from macOS into <out-dir>:
#
#   brightfx-wasm-web-<ver>.tgz     wasm-pack --target web, npm pack
#   brightfx-wasm-nodejs-<ver>.tgz  wasm-pack --target nodejs, npm pack
#   brightfx-js-<ver>.tgz           the typed wrapper, web wasm bundled
#   brightfx-remotion-<ver>.tgz     the Remotion component
#   BrightFX.xcframework.zip        universal (arm64 + x86_64) static library,
#                                   brightfx.h and the CBrightFX modulemap in
#                                   Headers/CBrightFX/, LICENSE beside it
#   brightfx.h                      the committed cbindgen header
#
# Builds only; verify-release-macos.sh replays the harnesses against the
# result. Needs the aarch64-apple-darwin and x86_64-apple-darwin targets.
# This script's cargo builds pass --locked, and it checks Cargo.lock first. In
# CI, the macOS job's "Cargo.lock is current" step guards every cargo run after
# it (smoke-all.sh and the harnesses build without --locked).
# Every asset carries the repo's MIT LICENSE.
set -euo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
core="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
root="$(cd "$core/.." && pwd)"
[ $# -eq 1 ] || { echo "usage: $0 <out-dir>" >&2; exit 2; }
mkdir -p "$1"
out="$(cd "$1" && pwd)"
ver="$("$core/scripts/check-version.sh")"
echo "==> building release $ver into $out"

# wasm-pack runs a plain `cargo metadata` before its build, which rewrites a
# stale Cargo.lock, so the --locked it forwards to cargo build comes too
# late. Check the lockfile first.
cargo metadata --manifest-path "$core/Cargo.toml" --locked --format-version 1 >/dev/null

# wasm-pack names the tarball after the crate (brightfx-wasm-<ver>.tgz) for
# both targets; rename each before the next one overwrites it.
for target in web nodejs; do
  dir="pkg-${target/nodejs/node}"
  echo "==> wasm package: $target"
  wasm-pack build "$core/brightfx-wasm" --release --target "$target" --out-dir "$dir" -- --locked
  # npm packs a root-level LICENSE whatever package.json's "files" says.
  cp "$root/LICENSE" "$core/brightfx-wasm/$dir/LICENSE"
  (cd "$core/brightfx-wasm/$dir" && npm pack --pack-destination "$out" >/dev/null)
  mv "$out/brightfx-wasm-$ver.tgz" "$out/brightfx-wasm-$target-$ver.tgz"
done

# brightfx-js's build copies pkg-web (just built above) into the package.
# The LICENSE copies are gitignored (packages/*/LICENSE).
echo "==> JS packages"
cp "$root/LICENSE" "$root/packages/brightfx-js/LICENSE"
cp "$root/LICENSE" "$root/packages/brightfx-remotion/LICENSE"
(cd "$root/packages" \
  && npm ci \
  && npm run build -w brightfx-js \
  && npm run build -w brightfx-remotion \
  && npm pack -w brightfx-js -w brightfx-remotion --pack-destination "$out" >/dev/null)

echo "==> xcframework"
for triple in aarch64-apple-darwin x86_64-apple-darwin; do
  cargo build --manifest-path "$core/Cargo.toml" -p brightfx-ffi --release --target "$triple" --locked
done
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
# Headers/CBrightFX/, not Headers/: Xcode copies a static xcframework's
# Headers into one shared include/ directory, where a root module.modulemap
# would collide with another static xcframework's.
mkdir -p "$stage/lib" "$stage/headers/CBrightFX" "$stage/zip"
lipo -create \
  "$core/target/aarch64-apple-darwin/release/libbrightfx_ffi.a" \
  "$core/target/x86_64-apple-darwin/release/libbrightfx_ffi.a" \
  -output "$stage/lib/libbrightfx_ffi.a"
# The modulemap the Swift harness uses: a consumer writes `import CBrightFX`.
cp "$core/brightfx-ffi/include/brightfx.h" "$core/harnesses/swift/module.modulemap" "$stage/headers/CBrightFX/"
xcodebuild -create-xcframework \
  -library "$stage/lib/libbrightfx_ffi.a" -headers "$stage/headers" \
  -output "$stage/zip/BrightFX.xcframework" >/dev/null
cp "$root/LICENSE" "$stage/zip/LICENSE"
rm -f "$out/BrightFX.xcframework.zip"
# The zip holds zip/'s contents: BrightFX.xcframework/ and LICENSE at the top.
# --norsrc/--noextattr keep AppleDouble (._*) files out of the zip.
ditto -c -k --norsrc --noextattr "$stage/zip" "$out/BrightFX.xcframework.zip"

cp "$core/brightfx-ffi/include/brightfx.h" "$out/brightfx.h"

echo "==> built:"
ls -1 "$out"
