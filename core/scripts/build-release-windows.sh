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
