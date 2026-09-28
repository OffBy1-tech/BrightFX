// Copies the web-target wasm-pack output into this package. The build is
// a gitignored artifact; the release workflow (phase 0) runs wasm-pack
// first, and so must anyone building locally.
import { copyFileSync, existsSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const src = join(here, "..", "..", "..", "core", "brightfx-wasm", "pkg-web");
const dst = join(here, "..", "wasm");
const files = ["brightfx_wasm.js", "brightfx_wasm_bg.wasm", "brightfx_wasm.d.ts", "brightfx_wasm_bg.wasm.d.ts"];

if (!existsSync(join(src, "brightfx_wasm_bg.wasm"))) {
  console.error(`no web build at ${src}\nrun: wasm-pack build core/brightfx-wasm --target web --out-dir pkg-web`);
  process.exit(1);
}
mkdirSync(dst, { recursive: true });
for (const f of files) copyFileSync(join(src, f), join(dst, f));
console.log(`synced ${files.length} files into ${dst}`);
