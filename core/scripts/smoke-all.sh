#!/usr/bin/env bash
# Runs every BrightFX check: Rust tests, then each FFI smoke harness.
# This is the verification entry point for the FFI boundary. CI runs it on
# every push to main and every pull request: .github/workflows/smoke.yml.
set -euo pipefail
# cargo-installed tools (cbindgen, wasm-pack) live here; some shells don't
# have it on PATH.
export PATH="$HOME/.cargo/bin:$PATH"
core="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

echo "==> Rust tests"
cargo test --manifest-path "$core/Cargo.toml"

echo "==> C header is up to date"
"$core/scripts/gen-header.sh" >/dev/null
if ! git -C "$core" diff --quiet -- brightfx-ffi/include/brightfx.h; then
  echo "ERROR: brightfx.h is stale. Commit the regenerated header." >&2
  git -C "$core" --no-pager diff -- brightfx-ffi/include/brightfx.h >&2
  exit 1
fi

echo "==> fixtures and presets are not rewritten by a verify run"
if ! git -C "$core" diff --quiet -- fixtures/ ../presets/; then
  echo "ERROR: running the tests modified the fixtures or presets." >&2
  echo "BRIGHTFX_REGENERATE is probably set in this environment, which turns" >&2
  echo "the fixture tests into a no-op that re-baselines instead of verifying." >&2
  git -C "$core" --no-pager diff --stat -- fixtures/ ../presets/ >&2
  exit 1
fi

echo "==> WASM build + Node harness"
wasm-pack build "$core/brightfx-wasm" --target nodejs --out-dir pkg-node
node "$core/harnesses/node/smoke.mjs"

echo "==> Swift harness"
"$core/harnesses/swift/run.sh"

echo "==> C# harness"
"$core/harnesses/csharp/run.sh"

echo "==> WASM web build + packages"
wasm-pack build "$core/brightfx-wasm" --target web --out-dir pkg-web
# brightfx-remotion imports brightfx-js's built dist; `--workspaces` runs in
# package.json array order, which is not a dependency order, so build the
# wrapper explicitly first rather than relying on the array.
(cd "$core/../packages" && npm ci && npm run build -w brightfx-js && npm test --workspaces)

echo
echo "all BrightFX checks passed"
