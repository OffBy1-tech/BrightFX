// Replays the ffi-smoke fixture through brightfx-js as a consumer installs
// it: from the release tarball, wasm loaded from the package's own export.
// Run by verify-release-macos.sh from a scratch project where the tarball
// is the only thing installed, with core/harnesses/node/replay.mjs copied
// next to it. Usage: node consumer-smoke.mjs <fixtures-dir>
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { BrightFX } from "brightfx-js";

import { drive, assertBufferClose } from "./replay.mjs";

const fixtures = process.argv[2];
const expected = JSON.parse(readFileSync(join(fixtures, "ffi-smoke.expected.json"), "utf8"));
const config = readFileSync(join(fixtures, "ffi-smoke.config.json"), "utf8");

const wasm = readFileSync(fileURLToPath(import.meta.resolve("brightfx-js/brightfx_wasm_bg.wasm")));
const engine = await BrightFX.init(wasm);
const sim = engine.create(BigInt(expected.seed));
const result = sim.setConfig(config);
assert.ok(result.ok, `config rejected: ${result.error}`);
assert.deepEqual(result.clamped, []);

drive(sim, expected);

assert.ok(expected.particleCount > 0, "fixture is vacuous");
assert.equal(sim.particleCount(), expected.particleCount, "particle count diverged");
assertBufferClose(sim.particles(), expected);
sim.dispose();
console.log(`  ok  brightfx-js tarball reproduces ffi-smoke (${expected.particleCount} particles)`);
