#!/usr/bin/env bash
# Builds and runs the C# smoke harness against the release cdylib.
#
# Runs on macOS and, under Git Bash, on Windows: the harness source is the
# same on both, and DllImport("brightfx_ffi") resolves whichever library
# file the platform produces, so only the file name copied next to the
# harness differs.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
core="$here/../.."

case "$(uname -s)" in
  Darwin) lib=libbrightfx_ffi.dylib ;;
  MINGW*|MSYS*|CYGWIN*) lib=brightfx_ffi.dll ;;
  *) lib=libbrightfx_ffi.so ;;
esac

cargo build --manifest-path "$core/Cargo.toml" -p brightfx-ffi --release

out="$here/bin/Debug/net9.0"
dotnet build "$here/Harness.csproj" -v quiet --nologo

mkdir -p "$out/fixtures"
cp "$core/target/release/$lib" "$out/"
# -R: fixtures/ now also holds the presets/ subdirectory (golden preset
# frames), which a plain `cp *` refuses to copy and, under `set -e`, would
# abort this script before the harness ever runs. The harness only reads
# the top-level fixture files; copying presets/ along with them is harmless.
cp -R "$core"/fixtures/* "$out/fixtures/"

dotnet "$out/BrightFxHarness.dll"
