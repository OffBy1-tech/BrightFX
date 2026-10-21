#!/usr/bin/env bash
# Sets every published BrightFX version to X.Y.Z: the four crates' [package]
# version, both JS packages, brightfx-remotion's range on brightfx-js, and
# both lockfiles. Then runs check-version.sh and shows what changed. It does
# not commit or tag.
#
#   bump-version.sh 0.4.1
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
[ $# -eq 1 ] || { echo "usage: $0 X.Y.Z" >&2; exit 2; }
new="$1"
[[ "$new" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]] || { echo "ERROR: \"$new\" is not X.Y.Z" >&2; exit 2; }

# Keep in step with the sources list in check-version.sh.
crates=(core/brightfx-core core/brightfx-ffi core/brightfx-wasm core/brightfx-tracks)
pkgs=(packages/brightfx-js packages/brightfx-remotion)

for c in "${crates[@]}"; do
  f="$root/$c/Cargo.toml"
  # Only the first `version = ` line (the [package] one), keeping CRLF if any.
  awk -v v="$new" '!done && /^version = "/ { sub(/"[^"]*"/, "\"" v "\""); done=1 } { print }' "$f" > "$f.tmp"
  cat "$f.tmp" > "$f" && rm -f "$f.tmp"
done

for p in "${pkgs[@]}"; do
  # Edit the text, not JSON.stringify, so the file's formatting survives;
  # then parse the result to prove it is still valid and says what we meant.
  node -e '
    const fs = require("fs"); const [f, v] = process.argv.slice(1);
    let t = fs.readFileSync(f, "utf8");
    t = t.replace(/^(  "version": ")[^"]*(")/m, `$1${v}$2`);
    t = t.replace(/^(    "brightfx-js": ")[^"]*(")/m, `$1^${v}$2`);
    const j = JSON.parse(t);
    if (j.version !== v) throw new Error("no version in " + f);
    fs.writeFileSync(f, t);
  ' "$root/$p/package.json" "$new"
done

cargo update --workspace --manifest-path "$root/core/Cargo.toml"
(cd "$root/packages" && npm install --package-lock-only >/dev/null)

"$root/core/scripts/check-version.sh" "v$new" >/dev/null
echo "bumped to $new"
git -C "$root" status --short
