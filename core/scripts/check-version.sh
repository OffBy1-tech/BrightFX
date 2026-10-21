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
