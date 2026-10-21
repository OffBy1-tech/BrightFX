# Phase 0: Release Workflow — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A pushed `v*` tag on `OffBy1-tech/BrightFX` produces a GitHub release carrying every artifact a consumer needs (both wasm packages, both JS packages, the macOS xcframework, the Windows DLL zip, the header, checksums), published only after the Node, Swift, and C# harnesses have replayed the fixtures against those exact artifacts.

**Architecture:** Build logic lives in shell scripts under `core/scripts/`, not in YAML, so it runs the same locally and in CI. `release.yml` has four jobs: a version check, a macOS build-and-verify job, a Windows build-and-verify job, and a publish job that runs only for tags. The harnesses gain environment-variable knobs (package directory, library directory, headers directory) whose defaults keep today's behavior, so `smoke-all.sh` is unchanged in effect.

**Tech Stack:** Bash, GitHub Actions (`macos-26`, `windows-2025`, `ubuntu-latest`), `wasm-pack` 0.15.0, `cbindgen` 0.29.4, `lipo`, `xcodebuild -create-xcframework`, `ditto`, `7z`, Node 22, `gh` ≥ 2.29 (for `--notes-from-tag`).

**Spec:** the phase 0 foundation design (not published here), section "The release" and the release half of "Testing". Mouseflare steps 1 and 2 are **out of scope** for this plan.

**Deviation from the spec, decided 2026-10-21:** versions are lockstep across *everything*: the four published crates, the wasm package, `brightfx-js`, and `brightfx-remotion`. The first tag is **`v0.4.0`**, not `v0.2.0`, because `brightfx-remotion` already says 0.3.0 and nothing may go backwards. Record this in the spec's status line.

**Status:** shipped with PR #55 and the `v0.4.0` release. Three review rounds changed the design after the tasks below were written, so `.github/workflows/release.yml` and the "Releases" section of `core/README.md` are the reference, not the code in this plan:

- An `assemble` job, run on every event, merges the build jobs' artifacts, checks the exact asset set, and writes `SHA256SUMS.txt`. `publish` runs only on tags and uploads what `assemble` produced.
- The version job refuses a lightweight tag and a tag on a commit not on `main` before anything builds. `publish` refuses a tag that moved off the built commit, and refuses if any release, draft or published, already exists for the tag: a failed run's leftover draft is deleted by hand, and a published version is never replaced. Release notes come from the tag's subject and body via `--notes-file`, not `--notes-from-tag`.
- The Windows DLL links the C runtime statically (`+crt-static` in `.cargo/config.toml`), and the build rejects any VC++ redistributable import.
- Every asset carries `LICENSE`. The xcframework nests its headers under `Headers/CBrightFX/`, and the Swift check compiles against that layout.
- A `cargo metadata --locked` step guards `Cargo.lock`. Tag runs skip every cache. The tag filter is `v[0-9]*`.

Deferred follow-ups are tracked in issue #56.

## Global Constraints

- All code changes land on branch `feat/release-workflow`, via one pull request. Never commit `docs/superpowers/` (it is gitignored), and keep local paths out of committed files.
- `wasm-pack` lives in `~/.cargo/bin`, which is not on every shell's PATH: prefix commands with `export PATH="$HOME/.cargo/bin:$PATH" &&`.
- Pins, copied from `smoke.yml`: `CBINDGEN_VERSION: 0.29.4`, `WASM_PACK_VERSION: 0.15.0`, `actions/checkout@v7`, `actions/setup-node@v7` (node 22), `actions/cache@v6`, `actions/setup-dotnet@v6` (9.0.x), `Swatinem/rust-cache@v2`. New: `actions/upload-artifact@v7`, `actions/download-artifact@v8`.
- Every workflow job refuses to start if `BRIGHTFX_REGENERATE` is set, with the same step `smoke.yml` uses.
- Version: one number, `X.Y.Z`, in `core/brightfx-{core,ffi,wasm,tracks}/Cargo.toml`, `packages/brightfx-js/package.json`, `packages/brightfx-remotion/package.json`, and `brightfx-remotion`'s dependency `"brightfx-js": "^X.Y.Z"`. `brightfx-test-support` is `publish = false` and is not checked. The tag is `vX.Y.Z`.
- Asset names (exact): `brightfx-wasm-web-<ver>.tgz`, `brightfx-wasm-nodejs-<ver>.tgz`, `brightfx-js-<ver>.tgz`, `brightfx-remotion-<ver>.tgz`, `BrightFX.xcframework.zip`, `brightfx_ffi-windows-x64.zip`, `brightfx.h`, `SHA256SUMS.txt`.
- The harness knob defaults must reproduce today's behavior byte for byte: `./core/scripts/smoke-all.sh` passes unchanged after every task.
- Git: `git add` explicit paths, never `-A`. Commit messages carry no session links; end them with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Tagging `v0.4.0` is outward-facing and permanent: it happens only after the PR merges **and** the user says to cut it (Task 7).

---

### Task 0: Branch

- [ ] **Step 1: Create the branch** from an up-to-date `origin/main`, in its own worktree if other work shares the checkout:

```bash
git fetch -q && git worktree add ../BrightFX-wt-release -b feat/release-workflow origin/main
```

- [ ] **Step 2: Baseline**

```bash
./core/scripts/smoke-all.sh 2>&1 | tail -3
```

Expected: `all BrightFX checks passed`. If it fails on `main`, stop and report; this plan does not fix pre-existing failures.

---

### Task 1: Lockstep version check, and the bump to 0.4.0

**Files:**
- Create: `core/scripts/check-version.sh`
- Modify: `core/scripts/smoke-all.sh` (call the check first)
- Modify: `core/brightfx-core/Cargo.toml`, `core/brightfx-ffi/Cargo.toml`, `core/brightfx-wasm/Cargo.toml`, `core/brightfx-tracks/Cargo.toml` (line 3, `version`), `core/Cargo.lock`
- Modify: `packages/brightfx-js/package.json`, `packages/brightfx-remotion/package.json`, `packages/package-lock.json`

**Interfaces:**
- Produces: `core/scripts/check-version.sh [vX.Y.Z]`. With no argument it exits 0 if all versions agree and prints the version on stdout, nothing else. With a tag argument it also requires the version to equal the tag minus its leading `v`. On any mismatch it exits 1 and prints each source and its version to stderr. Task 4 and Task 5 call it.

- [ ] **Step 1: Write the check**

`core/scripts/check-version.sh`:

```bash
#!/usr/bin/env bash
# Checks that every published BrightFX version is the same number: the four
# crates, the two JS packages, and brightfx-remotion's range on brightfx-js.
# Releases are lockstep, so one tag names every asset.
#
#   check-version.sh           print the shared version, or fail
#   check-version.sh v0.4.0    also fail unless it is 0.4.0
#
# Prints only the version on stdout, so callers can capture it.
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

# tr: a Windows checkout may convert Cargo.toml to CRLF.
cargo_version() { tr -d '\r' < "$root/$1" | sed -n 's/^version = "\(.*\)"$/\1/p' | head -n 1; }
json_value() { node -p "require(process.argv[1])$2" "$root/$1"; }

sources=(
  "core/brightfx-core/Cargo.toml=$(cargo_version core/brightfx-core/Cargo.toml)"
  "core/brightfx-ffi/Cargo.toml=$(cargo_version core/brightfx-ffi/Cargo.toml)"
  "core/brightfx-wasm/Cargo.toml=$(cargo_version core/brightfx-wasm/Cargo.toml)"
  "core/brightfx-tracks/Cargo.toml=$(cargo_version core/brightfx-tracks/Cargo.toml)"
  "packages/brightfx-js/package.json=$(json_value packages/brightfx-js/package.json .version)"
  "packages/brightfx-remotion/package.json=$(json_value packages/brightfx-remotion/package.json .version)"
)
version="${sources[0]#*=}"

fail() {
  echo "ERROR: $1" >&2
  for s in "${sources[@]}"; do echo "  ${s%%=*}: ${s#*=}" >&2; done
  exit 1
}

for s in "${sources[@]}"; do
  [ "${s#*=}" = "$version" ] || fail "versions disagree; releases are lockstep"
done

range="$(json_value packages/brightfx-remotion/package.json '.dependencies["brightfx-js"]')"
[ "$range" = "^$version" ] || fail "brightfx-remotion depends on brightfx-js \"$range\", expected \"^$version\""

if [ $# -gt 0 ]; then
  [ "${1#v}" = "$version" ] && [ "${1:0:1}" = "v" ] || fail "tag \"$1\" does not match version $version"
fi

echo "$version"
```

```bash
chmod +x core/scripts/check-version.sh
```

- [ ] **Step 2: Run it and see it fail on today's tree**

Run: `./core/scripts/check-version.sh; echo "exit $?"`
Expected: `ERROR: versions disagree; releases are lockstep`, the six sources listed (crates `0.1.0`, `brightfx-js` `0.2.0`, `brightfx-remotion` `0.3.0`), `exit 1`.

- [ ] **Step 3: Bump everything to 0.4.0**

Set `version = "0.4.0"` on line 3 of each of the four crate `Cargo.toml` files (leave `brightfx-test-support` at 0.1.0). In `packages/brightfx-js/package.json` set `"version": "0.4.0"`. In `packages/brightfx-remotion/package.json` set `"version": "0.4.0"` and `"brightfx-js": "^0.4.0"`. Edit by hand; do **not** use `npm install -w` or `npm version`, which reformat the compact arrays. Then refresh both lockfiles:

```bash
cargo update --manifest-path core/Cargo.toml --workspace --offline
cd packages && npm install --package-lock-only --ignore-scripts && cd ..
git diff --stat
```

Expected: the diff touches only the six manifests and the two lockfiles. `git diff packages/package-lock.json` changes only the version fields and the `^0.4.0` range. If `npm` reformatted `package.json`, restore the formatting by hand.

- [ ] **Step 4: Run the check: agree, right tag, wrong tag**

```bash
./core/scripts/check-version.sh; echo "exit $?"
./core/scripts/check-version.sh v0.4.0; echo "exit $?"
./core/scripts/check-version.sh v0.4.1; echo "exit $?"
./core/scripts/check-version.sh 0.4.0; echo "exit $?"
```

Expected, in order: `0.4.0` / `exit 0`; `0.4.0` / `exit 0`; `ERROR: tag "v0.4.1" does not match version 0.4.0` / `exit 1`; `ERROR: tag "0.4.0" does not match version 0.4.0` / `exit 1`.

Then break the remotion range temporarily to prove that branch fails: change it to `^0.3.0`, run the check (expect `ERROR: brightfx-remotion depends on brightfx-js "^0.3.0", expected "^0.4.0"`), and restore it.

- [ ] **Step 5: Run the check from smoke-all.sh, first**

In `core/scripts/smoke-all.sh`, immediately after the `core=...` line, insert:

```bash
echo "==> versions are lockstep"
"$core/scripts/check-version.sh" >/dev/null
```

- [ ] **Step 6: Full verification**

Run: `./core/scripts/smoke-all.sh 2>&1 | tail -3`
Expected: `all BrightFX checks passed`. The packages' tests read the version nowhere, so a bump changes no output; if a test fails, it is not this task's change, so stop and report.

- [ ] **Step 7: Commit**

```bash
git add core/scripts/check-version.sh core/scripts/smoke-all.sh \
  core/brightfx-core/Cargo.toml core/brightfx-ffi/Cargo.toml core/brightfx-wasm/Cargo.toml core/brightfx-tracks/Cargo.toml core/Cargo.lock \
  packages/brightfx-js/package.json packages/brightfx-remotion/package.json packages/package-lock.json
git commit -m "Lockstep versions: one number for every crate and package, checked in smoke-all

Bumps everything to 0.4.0, the first number above every existing one
(brightfx-remotion was already 0.3.0).

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Harness knobs — point the Node and Swift harnesses at a built artifact

**Files:**
- Modify: `core/harnesses/node/smoke.mjs:5-13` (imports and module loading)
- Modify: `core/harnesses/swift/run.sh`

**Interfaces:**
- Produces, for Task 3:
  - `BRIGHTFX_WASM_PKG=<dir>`: the directory holding `brightfx_wasm.js` and `brightfx_wasm_bg.wasm`. Default: `core/brightfx-wasm/pkg-node`.
  - `BRIGHTFX_WASM_TARGET=nodejs|web`: default `nodejs`. `web` calls the package's default `init` with the `.wasm` bytes read from `BRIGHTFX_WASM_PKG` before any test runs.
  - `BRIGHTFX_LIB_DIR=<dir>`: where `swiftc` finds `libbrightfx_ffi`. When set, `run.sh` does **not** run `cargo build`. Default: build, then `core/target/release`.
  - `BRIGHTFX_HEADERS_DIR=<dir>`: where `brightfx.h` and `module.modulemap` come from. Default: `core/brightfx-ffi/include/brightfx.h` and the harness's own `module.modulemap`.

The C# harness needs no knob: on Windows the DLL it copies out of `core/target/release` is the one Task 4 zips.

- [ ] **Step 1: Make the Node harness load its package from a variable**

In `core/harnesses/node/smoke.mjs`, replace lines 5–13 (from `import assert` through `const here = ...`) with:

```js
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath, pathToFileURL } from "node:url";
import { dirname, join, resolve } from "node:path";

import { readBuffer, readFrame, applyConfig, unwrap } from "./brightfx.mjs";

const here = dirname(fileURLToPath(import.meta.url));

// Which wasm package to replay against. By default the nodejs build that
// smoke-all.sh makes; the release workflow points this at each unpacked
// release tarball instead, so the bytes it verifies are the bytes it ships.
const pkgDir = process.env.BRIGHTFX_WASM_PKG
  ? resolve(process.env.BRIGHTFX_WASM_PKG)
  : join(here, "..", "..", "brightfx-wasm", "pkg-node");
const target = process.env.BRIGHTFX_WASM_TARGET ?? "nodejs";
if (target !== "nodejs" && target !== "web") {
  throw new Error(`BRIGHTFX_WASM_TARGET must be nodejs or web, got ${target}`);
}
const mod = await import(pathToFileURL(join(pkgDir, "brightfx_wasm.js")).href);
// The web target's init takes the wasm as a URL or bytes rather than
// reading it itself; under Node, bytes.
if (target === "web") {
  await mod.default({ module_or_path: readFileSync(join(pkgDir, "brightfx_wasm_bg.wasm")) });
}
console.log(`wasm package: ${pkgDir} (${target})`);
```

The rest of the file uses `mod` and `here` exactly as before.

- [ ] **Step 2: Check the default path still passes**

```bash
export PATH="$HOME/.cargo/bin:$PATH" && wasm-pack build core/brightfx-wasm --target nodejs --out-dir pkg-node && node core/harnesses/node/smoke.mjs
```

Expected: `wasm package: …/core/brightfx-wasm/pkg-node (nodejs)`, then every test `ok`, no failure.

- [ ] **Step 3: Check the web path, and that it fails when it should**

```bash
export PATH="$HOME/.cargo/bin:$PATH" && wasm-pack build core/brightfx-wasm --target web --out-dir pkg-web
BRIGHTFX_WASM_PKG=core/brightfx-wasm/pkg-web BRIGHTFX_WASM_TARGET=web node core/harnesses/node/smoke.mjs
BRIGHTFX_WASM_PKG=core/brightfx-wasm/pkg-web node core/harnesses/node/smoke.mjs; echo "exit $?"
BRIGHTFX_WASM_PKG=/nonexistent node core/harnesses/node/smoke.mjs; echo "exit $?"
```

Expected: the first run passes every test against `(web)`. The second, a web package loaded without the web init, fails with a non-zero exit (any error: the module is uninitialized or is ESM the harness can't use as nodejs). The third fails with `ERR_MODULE_NOT_FOUND`. If the second one *passes*, the harness is not proving the init path. Stop and report.

- [ ] **Step 4: Give the Swift harness library and header overrides**

Replace the body of `core/harnesses/swift/run.sh` after `core="$here/../.."` with:

```bash
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
```

Update the comment on line 2 to: `# Builds and runs the Swift smoke harness against the release staticlib, or against BRIGHTFX_LIB_DIR.`

- [ ] **Step 5: Check both Swift paths**

```bash
./core/harnesses/swift/run.sh 2>&1 | tail -3
lib="$(mktemp -d)" && cp core/target/release/libbrightfx_ffi.a "$lib/"
hdr="$(mktemp -d)" && cp core/brightfx-ffi/include/brightfx.h core/harnesses/swift/module.modulemap "$hdr/"
BRIGHTFX_LIB_DIR="$lib" BRIGHTFX_HEADERS_DIR="$hdr" ./core/harnesses/swift/run.sh 2>&1 | tail -3
BRIGHTFX_LIB_DIR="$(mktemp -d)" ./core/harnesses/swift/run.sh; echo "exit $?"
```

Expected: the first two runs pass with the harness's usual success output. The second prints `swift harness: linking <tmp dir>` and links only the static `.a`. The third fails with `library 'brightfx_ffi' not found` and a non-zero exit.

- [ ] **Step 6: Full verification**

Run: `./core/scripts/smoke-all.sh 2>&1 | tail -3`
Expected: `all BrightFX checks passed`.

- [ ] **Step 7: Commit**

```bash
git add core/harnesses/node/smoke.mjs core/harnesses/swift/run.sh
git commit -m "Harnesses: replay against a chosen wasm package or library directory

The Node harness takes BRIGHTFX_WASM_PKG and BRIGHTFX_WASM_TARGET (web
calls init with the wasm bytes); the Swift harness takes BRIGHTFX_LIB_DIR
and BRIGHTFX_HEADERS_DIR. The defaults are today's behavior. The release
workflow uses them to verify the artifacts it ships.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: macOS release build and artifact verification

**Files:**
- Create: `core/scripts/build-release-macos.sh`
- Create: `core/scripts/verify-release-macos.sh`
- Create: `core/scripts/consumer-smoke.mjs`

**Interfaces:**
- Consumes: `check-version.sh` (Task 1); the four harness knobs (Task 2).
- Produces: `build-release-macos.sh <out-dir>` writes exactly `brightfx-wasm-web-<ver>.tgz`, `brightfx-wasm-nodejs-<ver>.tgz`, `brightfx-js-<ver>.tgz`, `brightfx-remotion-<ver>.tgz`, `BrightFX.xcframework.zip`, `brightfx.h` into `<out-dir>`. `verify-release-macos.sh <out-dir>` exits 0 only if every harness replayed against those files. Task 5 calls both.

- [ ] **Step 1: Install the two Apple targets locally**

```bash
rustup target add aarch64-apple-darwin x86_64-apple-darwin
```

- [ ] **Step 2: Write the build script**

`core/scripts/build-release-macos.sh`:

```bash
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
(cd "$stage" && ditto -c -k --keepParent BrightFX.xcframework "$out/BrightFX.xcframework.zip")

cp "$core/brightfx-ffi/include/brightfx.h" "$out/brightfx.h"

echo "==> built:"
ls -1 "$out"
```

```bash
chmod +x core/scripts/build-release-macos.sh
```

- [ ] **Step 3: Run it**

```bash
./core/scripts/build-release-macos.sh /tmp/bfx-release 2>&1 | tail -10
```


Expected: the listing shows exactly the six files named in the header comment, with `<ver>` = `0.4.0`. Then inspect them:

```bash
cd /tmp/bfx-release
tar -tzf brightfx-wasm-web-0.4.0.tgz; tar -tzf brightfx-wasm-nodejs-0.4.0.tgz
tar -tzf brightfx-js-0.4.0.tgz | grep -E 'wasm/brightfx_wasm_bg.wasm|dist/index.js'
tar -tzf brightfx-remotion-0.4.0.tgz | grep dist/index.js
unzip -l BrightFX.xcframework.zip
```

Expected: both wasm tarballs hold `package/brightfx_wasm.js`, `package/brightfx_wasm_bg.wasm`, `package/brightfx_wasm.d.ts`, `package/package.json`. The JS tarballs hold the grepped files. The zip holds `BrightFX.xcframework/Info.plist` and one `macos-arm64_x86_64/` slice with `libbrightfx_ffi.a` and `Headers/brightfx.h`, `Headers/module.modulemap`. If wasm-pack's output carries a `.gitignore` of `*` that empties a tarball, add `"files"` handling or delete that `.gitignore` before packing, and record what you did in the script's comments.

- [ ] **Step 4: Write the packaged-wrapper smoke**

This proves the `brightfx-js` tarball works as installed, wasm included, by replaying the `ffi-smoke` fixture through its public API.

`core/scripts/consumer-smoke.mjs`:

```js
// Replays the ffi-smoke fixture through brightfx-js as a consumer installs
// it: from the release tarball, wasm loaded from the package's own export.
// Run by verify-release-macos.sh from a scratch project where the tarball
// is the only thing installed. Usage: node consumer-smoke.mjs <fixtures-dir>
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { BrightFX } from "brightfx-js";

const fixtures = process.argv[2];
const expected = JSON.parse(readFileSync(join(fixtures, "ffi-smoke.expected.json"), "utf8"));
const config = readFileSync(join(fixtures, "ffi-smoke.config.json"), "utf8");

const wasm = readFileSync(fileURLToPath(import.meta.resolve("brightfx-js/brightfx_wasm_bg.wasm")));
const engine = await BrightFX.init(wasm);
const sim = engine.create(BigInt(expected.seed));
const result = sim.setConfig(config);
assert.ok(result.ok, `config rejected: ${result.error}`);
assert.deepEqual(result.clamped, []);

expected.emitterFrames.forEach((e, frame) => {
  sim.setEmitter(e.x, e.y, e.vx, e.vy, true);
  if (frame === expected.burstFrame) sim.triggerBurst();
  sim.advance(expected.dt);
});

assert.ok(expected.particleCount > 0, "fixture is vacuous");
assert.equal(sim.particleCount(), expected.particleCount, "particle count diverged");
const actual = sim.particles();
assert.equal(actual.length, expected.buffer.length, "buffer length diverged");
for (let i = 0; i < actual.length; i++) {
  if (!(Math.abs(actual[i] - expected.buffer[i]) <= expected.tolerance)) {
    assert.fail(`float ${i}: got ${actual[i]}, expected ${expected.buffer[i]}`);
  }
}
sim.dispose();
console.log(`  ok  brightfx-js tarball reproduces ffi-smoke (${expected.particleCount} particles)`);
```

- [ ] **Step 5: Write the verify script**

`core/scripts/verify-release-macos.sh`:

```bash
#!/usr/bin/env bash
# Replays the harnesses against the assets build-release-macos.sh wrote, so
# a release ships the bytes that were verified, not a lookalike build:
#
#   the Node harness against each unpacked wasm tarball (nodejs, then web)
#   the Swift harness against the xcframework's universal slice and headers
#   the brightfx-js tarball installed into a scratch project
#   the header against the committed one, and the remotion tarball's range
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
lipo "$slice/libbrightfx_ffi.a" -verify_arch arm64 x86_64
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

echo "==> brightfx.h is the committed header"
cmp "$out/brightfx.h" "$core/brightfx-ffi/include/brightfx.h"

echo
echo "release $ver verified"
```

```bash
chmod +x core/scripts/verify-release-macos.sh
```

- [ ] **Step 6: Verify the build from Step 3**

Run: `./core/scripts/verify-release-macos.sh /tmp/bfx-release 2>&1 | tail -15`
Expected: Node harness output twice (`(nodejs)` then `(web)`, both from the scratch dir, not `pkg-node`), the Swift harness linking `…/macos-arm64_x86_64`, the `ok  brightfx-js tarball reproduces ffi-smoke` line, and `release 0.4.0 verified`.

- [ ] **Step 7: Prove verification can fail**

Corrupt one artifact and confirm the script catches it, then rebuild:

```bash
cd /tmp/bfx-release && mkdir x && tar -xzf brightfx-wasm-web-0.4.0.tgz -C x \
  && printf 'not wasm' > x/package/brightfx_wasm_bg.wasm \
  && tar -czf brightfx-wasm-web-0.4.0.tgz -C x package && rm -rf x && cd -
./core/scripts/verify-release-macos.sh /tmp/bfx-release; echo "exit $?"
```

Expected: the web replay fails with a WebAssembly compile error and `exit 1`. Then rerun Step 3's build so `/tmp/bfx-release` is clean again.

- [ ] **Step 8: Commit**

```bash
git add core/scripts/build-release-macos.sh core/scripts/verify-release-macos.sh core/scripts/consumer-smoke.mjs
git commit -m "Release scripts: build the macOS-side assets, then verify those exact files

build-release-macos.sh packs both wasm targets, both JS packages, a
universal xcframework, and the header. verify-release-macos.sh replays the
Node harness against each wasm tarball, the Swift harness against the
xcframework's slice, and the ffi-smoke fixture through the installed
brightfx-js tarball.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Windows release build

**Files:**
- Create: `core/scripts/build-release-windows.sh`

**Interfaces:**
- Produces: `build-release-windows.sh <out-dir>` writes `brightfx_ffi-windows-x64.zip` holding `brightfx_ffi.dll`, `brightfx_ffi.dll.lib`, `brightfx.h`, after the C# harness has run against that DLL. Task 5 calls it under Git Bash on `windows-2025`.

This cannot run on macOS. Its test is the Windows job in Task 5's dry run.

- [ ] **Step 1: Write the script**

`core/scripts/build-release-windows.sh`:

```bash
#!/usr/bin/env bash
# Builds brightfx_ffi-windows-x64.zip into <out-dir>: the MSVC DLL, its
# import library, and the header. Runs the C# harness first against the DLL
# in target/release, which is the file that gets zipped. Git Bash on Windows.
set -euo pipefail
core="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
[ $# -eq 1 ] || { echo "usage: $0 <out-dir>" >&2; exit 2; }
mkdir -p "$1"
out="$(cd "$1" && pwd)"
"$core/scripts/check-version.sh" >/dev/null

cargo build --manifest-path "$core/Cargo.toml" -p brightfx-ffi --release
"$core/harnesses/csharp/run.sh"

stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
for f in brightfx_ffi.dll brightfx_ffi.dll.lib; do
  [ -f "$core/target/release/$f" ] || { echo "ERROR: no $f in target/release" >&2; exit 1; }
  cp "$core/target/release/$f" "$stage/"
done
cp "$core/brightfx-ffi/include/brightfx.h" "$stage/"
rm -f "$out/brightfx_ffi-windows-x64.zip"
(cd "$stage" && 7z a -tzip "$out/brightfx_ffi-windows-x64.zip" brightfx_ffi.dll brightfx_ffi.dll.lib brightfx.h >/dev/null)
echo "built $out/brightfx_ffi-windows-x64.zip"
```

```bash
chmod +x core/scripts/build-release-windows.sh
bash -n core/scripts/build-release-windows.sh && echo syntax ok
```

- [ ] **Step 2: Commit**

```bash
git add core/scripts/build-release-windows.sh
git commit -m "Release script: the Windows DLL zip, after the C# harness runs against it

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: The release workflow, and a dry run on the PR

**Files:**
- Create: `.github/workflows/release.yml`

**Interfaces:**
- Consumes: `check-version.sh`, `build-release-macos.sh`, `verify-release-macos.sh`, `build-release-windows.sh`.

`workflow_dispatch` only works once a workflow file is on the default branch, so the dry run uses a `pull_request` trigger limited to the release machinery: any PR that touches it builds and verifies every artifact, and only a tag publishes.

- [ ] **Step 1: Write the workflow**

`.github/workflows/release.yml`:

```yaml
# Builds, verifies, and publishes a BrightFX release.
#
# A pushed tag vX.Y.Z publishes release X.Y.Z. Every version in the repo
# must already say X.Y.Z (core/scripts/check-version.sh); releases are
# lockstep. The release notes are the annotated tag's message.
#
# A pull request that touches the release machinery, or a manual run,
# builds and verifies every artifact and stops before publishing.
#
# Build logic lives in core/scripts/, not here. The verify step replays the
# Node, Swift, and C# harnesses against the exact files that get uploaded,
# so a release exists only if its artifacts reproduced the fixtures.
#
# Keeps the two rules smoke.yml documents: never set BRIGHTFX_REGENERATE,
# and keep cbindgen pinned. The pins below match smoke.yml; change both.

name: release

on:
  push:
    tags: ["v*"]
  pull_request:
    paths:
      - .github/workflows/release.yml
      - core/scripts/check-version.sh
      - core/scripts/build-release-*.sh
      - core/scripts/verify-release-*.sh
      - core/scripts/consumer-smoke.mjs
      - core/harnesses/**
  workflow_dispatch:

permissions:
  contents: read

concurrency:
  group: release-${{ github.ref }}
  cancel-in-progress: false

env:
  CBINDGEN_VERSION: 0.29.4
  WASM_PACK_VERSION: 0.15.0
  CARGO_TERM_COLOR: always
  DOTNET_NOLOGO: "1"
  DOTNET_CLI_TELEMETRY_OPTOUT: "1"

jobs:
  version:
    name: Versions are lockstep and match the tag
    runs-on: ubuntu-latest
    outputs:
      version: ${{ steps.check.outputs.version }}
    steps:
      - uses: actions/checkout@v7
      - uses: actions/setup-node@v7
        with:
          node-version: 22
      - id: check
        run: |
          if [ "$GITHUB_REF_TYPE" = tag ]; then
            version="$(./core/scripts/check-version.sh "$GITHUB_REF_NAME")"
          else
            version="$(./core/scripts/check-version.sh)"
          fi
          echo "version=$version" >> "$GITHUB_OUTPUT"
          echo "release $version"

  macos:
    name: macOS assets, built and verified
    needs: version
    runs-on: macos-26
    timeout-minutes: 45
    steps:
      - uses: actions/checkout@v7

      - name: Refuse to run with BRIGHTFX_REGENERATE set
        run: |
          if [ -n "${BRIGHTFX_REGENERATE+set}" ]; then
            echo "::error::BRIGHTFX_REGENERATE is set (to \"$BRIGHTFX_REGENERATE\"). Only =1 regenerates, but CI refuses any value so a verify run is never ambiguous." >&2
            exit 1
          fi

      - name: Rust toolchain
        run: |
          rustup update stable --no-self-update
          rustup default stable
          rustup target add wasm32-unknown-unknown aarch64-apple-darwin x86_64-apple-darwin

      - uses: Swatinem/rust-cache@v2
        with:
          workspaces: core

      - name: Restore pinned cargo tools
        id: cargo-tools
        uses: actions/cache@v6
        with:
          path: |
            ~/.cargo/bin/cbindgen
            ~/.cargo/bin/wasm-pack
          key: cargo-tools-${{ runner.os }}-${{ runner.arch }}-cbindgen-${{ env.CBINDGEN_VERSION }}-wasm-pack-${{ env.WASM_PACK_VERSION }}

      - name: Install pinned cargo tools
        if: steps.cargo-tools.outputs.cache-hit != 'true'
        run: |
          cargo install cbindgen --version "$CBINDGEN_VERSION" --locked
          cargo install wasm-pack --version "$WASM_PACK_VERSION" --locked

      - uses: actions/setup-node@v7
        with:
          node-version: 22

      - name: Cache Remotion's Chrome Headless Shell
        uses: actions/cache@v6
        with:
          path: packages/brightfx-remotion/node_modules/.remotion
          key: remotion-chrome-${{ runner.os }}-${{ hashFiles('packages/package-lock.json') }}

      - uses: actions/setup-dotnet@v6
        with:
          dotnet-version: 9.0.x

      # The full verification first: Rust tests, header and fixture guards,
      # every harness, the package tests. Then build the assets and replay
      # the harnesses against them.
      - name: smoke-all.sh
        run: ./core/scripts/smoke-all.sh

      - name: Build release assets
        run: ./core/scripts/build-release-macos.sh dist

      - name: Verify the built assets
        run: ./core/scripts/verify-release-macos.sh dist

      - uses: actions/upload-artifact@v7
        with:
          name: release-macos
          path: dist/
          if-no-files-found: error

  windows:
    name: Windows DLL, built and verified
    needs: version
    runs-on: windows-2025
    timeout-minutes: 30
    defaults:
      run:
        shell: bash
    steps:
      - uses: actions/checkout@v7

      - name: Refuse to run with BRIGHTFX_REGENERATE set
        run: |
          if [ -n "${BRIGHTFX_REGENERATE+set}" ]; then
            echo "::error::BRIGHTFX_REGENERATE is set (to \"$BRIGHTFX_REGENERATE\"). Only =1 regenerates, but CI refuses any value so a verify run is never ambiguous." >&2
            exit 1
          fi

      - name: Rust toolchain
        run: |
          rustup update stable --no-self-update
          rustup default stable

      - uses: Swatinem/rust-cache@v2
        with:
          workspaces: core

      - uses: actions/setup-node@v7
        with:
          node-version: 22

      - uses: actions/setup-dotnet@v6
        with:
          dotnet-version: 9.0.x

      - name: Rust tests
        run: cargo test --manifest-path core/Cargo.toml

      - name: Build the DLL zip (runs the C# harness against the DLL first)
        run: ./core/scripts/build-release-windows.sh dist

      - uses: actions/upload-artifact@v7
        with:
          name: release-windows
          path: dist/
          if-no-files-found: error

  publish:
    name: Publish the release
    needs: [version, macos, windows]
    if: github.ref_type == 'tag'
    runs-on: ubuntu-latest
    permissions:
      contents: write
    steps:
      - uses: actions/download-artifact@v8
        with:
          path: dist
          merge-multiple: true

      - name: Checksums
        working-directory: dist
        run: |
          sha256sum * > SHA256SUMS.txt
          cat SHA256SUMS.txt

      # Not a draft: consumers pin an exact tag, so publishing promotes
      # nothing. The notes are the annotated tag's message.
      - name: Create the release
        env:
          GH_TOKEN: ${{ github.token }}
        run: |
          gh release create "$GITHUB_REF_NAME" dist/* \
            --repo "$GITHUB_REPOSITORY" \
            --title "BrightFX $GITHUB_REF_NAME" \
            --verify-tag --notes-from-tag
```

- [ ] **Step 2: Lint it**

```bash
command -v actionlint >/dev/null || brew install actionlint
actionlint .github/workflows/release.yml && echo lint ok
```

Expected: `lint ok`. Fix anything it reports.

- [ ] **Step 3: Commit, push, open the PR**

Do Task 6 first if you want one review round; otherwise push now and add Task 6 as a second commit.

```bash
git add .github/workflows/release.yml
git commit -m "Release workflow: build, verify, and publish on a v* tag

PRs that touch the release machinery run everything but the publish job.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git push -u origin feat/release-workflow
gh pr create --repo OffBy1-tech/BrightFX --title "Release workflow: tagged releases with verified artifacts" --body "$(cat <<'EOF'
A pushed `vX.Y.Z` tag now publishes a GitHub release with every artifact a consumer needs, after the Node, Swift, and C# harnesses have replayed the fixtures against those exact files.

- **Lockstep versions.** One number for the four crates, the wasm package, `brightfx-js`, and `brightfx-remotion`, checked by `core/scripts/check-version.sh` (also run first in `smoke-all.sh`). Everything is bumped to 0.4.0, the first release.
- **Assets.** `brightfx-wasm-{web,nodejs}-<ver>.tgz`, `brightfx-js-<ver>.tgz`, `brightfx-remotion-<ver>.tgz`, `BrightFX.xcframework.zip` (universal static library, `import CBrightFX`), `brightfx_ffi-windows-x64.zip`, `brightfx.h`, `SHA256SUMS.txt`.
- **Verified, not rebuilt.** The Node harness replays against each unpacked wasm tarball (the web one through its `init` with the wasm bytes), the Swift harness links the xcframework's slice, the C# harness runs against the DLL that gets zipped, and the `ffi-smoke` fixture replays through the `brightfx-js` tarball installed into a scratch project.
- **Harness knobs.** `BRIGHTFX_WASM_PKG`, `BRIGHTFX_WASM_TARGET`, `BRIGHTFX_LIB_DIR`, `BRIGHTFX_HEADERS_DIR`; defaults are today's behavior.
- **Dry run.** This PR touches the release machinery, so the `release` workflow runs every job except `publish` on it.

After merge: an annotated `v0.4.0` tag cuts the first release.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 4: Watch the dry run**

```bash
gh pr checks --repo OffBy1-tech/BrightFX --watch
```

Expected: `smoke` (both jobs) and `release` (`version`, `macos`, `windows`) pass; `publish` shows as skipped. Download the artifacts and check their contents:

```bash
run="$(gh run list --repo OffBy1-tech/BrightFX --workflow release.yml --branch feat/release-workflow --limit 1 --json databaseId --jq '.[0].databaseId')"
gh run download "$run" --repo OffBy1-tech/BrightFX --dir /tmp/bfx-dry && ls -R /tmp/bfx-dry
unzip -l /tmp/bfx-dry/release-windows/brightfx_ffi-windows-x64.zip
```

Expected: `release-macos/` holds the six macOS files; `release-windows/` holds the zip with `brightfx_ffi.dll`, `brightfx_ffi.dll.lib`, `brightfx.h`. A failure on Windows is most likely the `7z` path or the `.dll.lib` name: read the log, fix the script, push, re-watch.

---

### Task 6: Documentation

**Files:**
- Modify: `core/README.md` (a "Releases" section after the build/verify instructions near line 179)
- Modify: `README.md` (the "Where this is" list, and a line under "Verifying")

- [ ] **Step 1: Add the Releases section to `core/README.md`**

```markdown
## Releases

Releases are tagged `vX.Y.Z` and lockstep: the four crates, the wasm
package, `brightfx-js`, and `brightfx-remotion` all carry the same version,
and `core/scripts/check-version.sh` (run first by `smoke-all.sh`) fails if
they don't.

To cut one:

1. In a pull request, bump every version to `X.Y.Z` (the six manifests,
   brightfx-remotion's `brightfx-js` range, both lockfiles) and merge it.
   While the version is `0.x`, a change to the C ABI, the wasm exports, the
   schema, or the fixtures' recorded output bumps the minor; anything else
   bumps the patch.
2. Tag `main` with an annotated tag; its message becomes the release notes:
   `git tag -a vX.Y.Z -m "..." && git push origin vX.Y.Z`.

`.github/workflows/release.yml` builds every asset, replays the Node, Swift,
and C# harnesses against those exact files, and only then publishes:

| Asset | For |
| --- | --- |
| `brightfx-wasm-web-<ver>.tgz` | the raw wasm package, `--target web` |
| `brightfx-wasm-nodejs-<ver>.tgz` | the raw wasm package, `--target nodejs` |
| `brightfx-js-<ver>.tgz` | the typed wrapper, web wasm bundled |
| `brightfx-remotion-<ver>.tgz` | the Remotion component |
| `BrightFX.xcframework.zip` | Swift: a universal static library; `import CBrightFX` |
| `brightfx_ffi-windows-x64.zip` | C#/C: `brightfx_ffi.dll`, its import library, the header |
| `brightfx.h` | the C header, for hosts linking the raw library |
| `SHA256SUMS.txt` | one checksum per asset |

A consumer pins an exact tag and fetches with
`gh release download vX.Y.Z --repo OffBy1-tech/BrightFX`. The JS packages
install from the tarballs as `file:` dependencies until they are published
to npm.

To build and verify the macOS-side assets locally:
`./core/scripts/build-release-macos.sh dist && ./core/scripts/verify-release-macos.sh dist`
(needs `rustup target add aarch64-apple-darwin x86_64-apple-darwin`).
```

- [ ] **Step 2: Update the root `README.md`**

In "Built and verified", after the "Continuous integration" bullet, add:

```markdown
- Releases: a `vX.Y.Z` tag publishes the wasm packages, the JS packages,
  a universal macOS xcframework, and the Windows DLL, each replayed
  against the fixtures before it ships. See `core/README.md`.
```

- [ ] **Step 3: Commit and push**

```bash
git add core/README.md README.md
git commit -m "docs: how releases are versioned, cut, and consumed

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git push
```

---

### Task 7: Merge, tag, consumer check (after the user's go-ahead)

- [ ] **Step 1: Report CI and ask for the merge.** The user merges on GitHub and says "merged".

- [ ] **Step 2: Ask before tagging.** A published tag and release are permanent and public. With the user's yes:

```bash
git checkout main && git pull --ff-only
./core/scripts/check-version.sh v0.4.0
git tag -a v0.4.0 -m "BrightFX 0.4.0: the first release

Every crate and package at one version. Assets: the wasm packages for
web and Node, brightfx-js, brightfx-remotion, a universal macOS
xcframework, the Windows DLL, the C header."
git push origin v0.4.0
# Wait for the run on this tag, not whatever run was newest before the push.
until run="$(gh run list --repo OffBy1-tech/BrightFX --workflow release.yml --branch v0.4.0 --limit 1 --json databaseId --jq '.[0].databaseId')" && [ -n "$run" ]; do sleep 5; done
gh run watch --repo OffBy1-tech/BrightFX "$run" --exit-status
gh release view v0.4.0 --repo OffBy1-tech/BrightFX
```

Expected: the release lists the eight assets.

- [ ] **Step 3: Consumer smoke, no Rust toolchain**

```bash
d="$(mktemp -d)" && cd "$d" \
  && gh release download v0.4.0 --repo OffBy1-tech/BrightFX --pattern 'brightfx-js-*.tgz' --pattern 'SHA256SUMS.txt' \
  && grep brightfx-js SHA256SUMS.txt | shasum -a 256 -c \
  && npm init -y >/dev/null && npm install --no-audit --no-fund ./brightfx-js-0.4.0.tgz \
  && node -e 'import("brightfx-js").then(m => console.log(typeof m.BrightFX.init))'
```

Expected: `brightfx-js-0.4.0.tgz: OK`, then `function`.

- [ ] **Step 4: Clean up and garden**

Remove the worktree and branch. Move this plan to `docs/plans/2026-10-21-phase-0-release-workflow.md`, add its row to `docs/README.md` ("shipped with PR #N"), and open that as a small docs PR.
