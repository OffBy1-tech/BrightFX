#!/usr/bin/env bash
# Builds brightfx_ffi-windows-x64.zip into <out-dir>: the MSVC DLL, its
# import library, the header, and LICENSE. Runs the C# harness first against
# the DLL in target/release, which is the file that gets zipped. Git Bash on
# Windows. The DLL links the C runtime statically (+crt-static, set in the
# repo-root .cargo/config.toml), so consumers need no VC++ redistributable.
set -euo pipefail
core="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
[ $# -eq 1 ] || { echo "usage: $0 <out-dir>" >&2; exit 2; }
mkdir -p "$1"
out="$(cd "$1" && pwd)"
"$core/scripts/check-version.sh" >/dev/null
# cargo reads .cargo/config.toml from the current directory up, not from
# --manifest-path; run from the repo root so +crt-static always applies, to
# this build and to the one csharp/run.sh links.
cd "$core/.."

cargo build --manifest-path "$core/Cargo.toml" -p brightfx-ffi --release --locked
# Import names are ASCII strings in the import table; a dynamic CRT shows up
# as vcruntime140*.dll, and C++ code as msvcp140*.dll or concrt140.dll: all
# part of the VC++ redistributable.
if grep -qaiE 'vcruntime1[0-9]{2}|msvcp1[0-9]{2}|concrt1[0-9]{2}' "$core/target/release/brightfx_ffi.dll"; then
  echo "ERROR: brightfx_ffi.dll imports the VC++ redistributable; expected +crt-static" >&2
  exit 1
fi
"$core/harnesses/csharp/run.sh"

stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
for f in brightfx_ffi.dll brightfx_ffi.dll.lib; do
  [ -f "$core/target/release/$f" ] || { echo "ERROR: no $f in target/release" >&2; exit 1; }
  cp "$core/target/release/$f" "$stage/"
done
cp "$core/brightfx-ffi/include/brightfx.h" "$stage/"
cp "$core/../LICENSE" "$stage/"
rm -f "$out/brightfx_ffi-windows-x64.zip"
(cd "$stage" && 7z a -tzip "$out/brightfx_ffi-windows-x64.zip" brightfx_ffi.dll brightfx_ffi.dll.lib brightfx.h LICENSE >/dev/null)
echo "built $out/brightfx_ffi-windows-x64.zip"
