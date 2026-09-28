#!/usr/bin/env bash
# Regenerates the committed C header from brightfx-ffi's source.
# Run after changing any `extern "C"` signature.
set -euo pipefail
# cargo-installed tools live here; some shells don't have it on PATH.
export PATH="$HOME/.cargo/bin:$PATH"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cbindgen --config "$here/brightfx-ffi/cbindgen.toml" \
         --crate brightfx-ffi \
         --output "$here/brightfx-ffi/include/brightfx.h" \
         "$here/brightfx-ffi"
echo "wrote $here/brightfx-ffi/include/brightfx.h"
