// Stages the wasm binary where the test compositions' staticFile() finds it.
import { copyFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const src = join(here, "..", "..", "brightfx-js", "wasm", "brightfx_wasm_bg.wasm");
const dst = join(here, "..", "public");
mkdirSync(dst, { recursive: true });
copyFileSync(src, join(dst, "brightfx_wasm_bg.wasm"));
console.log(`staged wasm into ${dst}`);
