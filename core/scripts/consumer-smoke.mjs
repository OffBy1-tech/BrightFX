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
