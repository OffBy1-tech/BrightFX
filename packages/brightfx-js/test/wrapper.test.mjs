// The wrapper adds nothing to the core's output. Every number here comes
// from the same fixtures the three harnesses replay.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

import { BrightFX, unpremultiply } from "../dist/index.js";
import { wasmMemory } from "../wasm/brightfx_wasm.js";

const here = dirname(fileURLToPath(import.meta.url));
const fixtures = join(here, "..", "..", "..", "core", "fixtures");
const read = (name) => readFileSync(join(fixtures, name), "utf8");
const smokeExpected = JSON.parse(read("ffi-smoke.expected.json"));
const frameExpected = JSON.parse(read("ffi-frame.expected.json"));
const frameBytes = new Uint8Array(readFileSync(join(fixtures, frameExpected.rgbaFile)));
const seekForward = JSON.parse(read("ffi-seek-forward.expected.json"));
// The fixture records the whole `{ok, jobs}` envelope (byte-exact, see
// cues_fixture.rs); the wrapper returns the jobs array.
const cueExpected = JSON.parse(read("tracks-cues.expected.json")).jobs;

const engine = await BrightFX.init(readFileSync(join(here, "..", "wasm", "brightfx_wasm_bg.wasm")));

function drive(sim, { emitterFrames, burstFrame, dt }) {
  emitterFrames.forEach((e, i) => {
    sim.setEmitter(e.x, e.y, e.vx, e.vy, true);
    if (i === burstFrame) sim.triggerBurst();
    sim.advance(dt);
  });
}

function assertClose(actual, expected, tolerance, label) {
  assert.equal(actual.length, expected.length, `${label}: length`);
  for (let i = 0; i < actual.length; i++) {
    assert.ok(Math.abs(actual[i] - expected[i]) <= tolerance, `${label}: float ${i} got ${actual[i]}, expected ${expected[i]}`);
  }
}

test("setConfig returns the envelope for objects and strings", () => {
  const sim = engine.create(1);
  // The fixture's worst case (10/step x 60 + a burst of 20) is over the
  // 500-particle pool, so it carries one advisory warning.
  for (const input of [read("ffi-smoke.config.json"), JSON.parse(read("ffi-smoke.config.json"))]) {
    const result = sim.setConfig(input);
    assert.equal(result.ok, true);
    assert.deepEqual(result.clamped, []);
    assert.equal(result.warnings.length, 1);
    assert.match(result.warnings[0], /particle pool/);
  }
  // A config that fits the pool carries an empty list, not a missing field.
  const small = JSON.parse(read("ffi-smoke.config.json"));
  small.emitter.spawnRateWhileActive = 1;
  small.emitter.spawnBurstSize = 0;
  assert.deepEqual(sim.setConfig(small), { ok: true, clamped: [], warnings: [] });
  const bad = sim.setConfig("{ not json");
  assert.equal(bad.ok, false);
  assert.match(bad.error, /invalid JSON/);
  sim.dispose();
});

test("particles() reproduces the smoke fixture buffer", () => {
  const sim = engine.create(smokeExpected.seed);
  sim.setConfig(read("ffi-smoke.config.json"));
  drive(sim, smokeExpected);
  assert.equal(sim.particleCount(), smokeExpected.particleCount);
  assertClose(sim.particles(), smokeExpected.buffer, smokeExpected.tolerance, "buffer");
  sim.dispose();
});

test("particleList() is the buffer as objects", () => {
  const sim = engine.create(smokeExpected.seed);
  sim.setConfig(read("ffi-smoke.config.json"));
  drive(sim, smokeExpected);
  const list = sim.particleList();
  const flat = sim.particles();
  assert.equal(list.length, sim.particleCount());
  const p = list[3];
  assert.deepEqual([p.x, p.y, p.size, p.rotation, p.r, p.g, p.b, p.a], Array.from(flat.subarray(24, 32)));
  sim.dispose();
});

test("seek forward matches the recorded forward fixture", () => {
  const sim = engine.create(seekForward.seed);
  sim.setConfig(read("ffi-seek.config.json"));
  for (const t of seekForward.seekTimes) sim.seek(t);
  assertClose(sim.particles(), seekForward.buffer, seekForward.tolerance, "forward seek");
  sim.dispose();
});

test("frame() is the golden frame with straight alpha", () => {
  const sim = engine.create(frameExpected.seed);
  sim.setConfig(read("ffi-frame.config.json"));
  assert.equal(sim.setViewport(frameExpected.width, frameExpected.height, frameExpected.scale).ok, true);
  drive(sim, frameExpected);
  sim.render();
  const { width, height, data } = sim.frame();
  assert.equal(width, frameExpected.width);
  assert.equal(height, frameExpected.height);
  assert.equal(data.length, frameBytes.length);
  assert.ok(data instanceof Uint8ClampedArray);

  // Re-premultiply and compare against the recorded premultiplied frame.
  // +1 on the tolerance covers the round trip's rounding.
  let differing = 0;
  for (let i = 0; i < data.length; i += 4) {
    const a = data[i + 3];
    let off = a !== frameBytes[i + 3];
    for (let c = 0; c < 3 && !off; c++) {
      off = Math.abs(Math.round((data[i + c] * a) / 255) - frameBytes[i + c]) > frameExpected.channelTolerance + 1;
    }
    if (off) differing++;
  }
  assert.ok(differing <= frameExpected.maxDifferingPixels, `${differing} pixels differ`);
  sim.dispose();
});

test("unpremultiply inverts premultiplication", () => {
  const out = unpremultiply(new Uint8Array([255, 0, 0, 255, 64, 32, 0, 128, 10, 10, 10, 0]));
  assert.deepEqual(Array.from(out), [255, 0, 0, 255, 128, 64, 0, 128, 0, 0, 0, 0]);
});

test("particles() survives linear memory growing", () => {
  const sim = engine.create(1);
  sim.setConfig(read("ffi-smoke.config.json"));
  sim.setEmitter(0, 0, 1.5, 0, true);
  for (let i = 0; i < 40; i++) sim.advance(0.015625);
  const before = Array.from(sim.particles());
  assert.ok(before.length > 0, "vacuous");
  wasmMemory().grow(1);
  assert.deepEqual(Array.from(sim.particles()), before);
  sim.dispose();
});

test("a disposed simulation throws instead of touching freed memory", () => {
  const sim = engine.create(1);
  sim.dispose();
  assert.throws(() => sim.particleCount(), /disposed/);
  sim.dispose(); // idempotent
});

test("generateCueTracks and fitTrack run through the wrapper", () => {
  const input = JSON.parse(read("tracks-cues.input.json"));
  assert.deepEqual(engine.generateCueTracks(input), cueExpected);
  const config = JSON.parse(read("ffi-seek.config.json"));
  const fitted = engine.fitTrack(config, [1920, 1080], [1080, 1920]);
  assert.equal(fitted.emitterTrack.keyframes[2].x, 120 * (1080 / 1920));
  assert.throws(() => engine.generateCueTracks({}), /invalid cue input/);
});

test("init clears the cached promise on rejection so a retry with real bytes succeeds", () => {
  // This module's `engine` above already resolved `BrightFX.init` with the
  // real wasm bytes, which also leaves the wasm glue's own module-level
  // instance set -- a second `init()` call in *this* process would resolve
  // immediately no matter what bytes it was given, masking the bug this
  // test exists to catch. A fresh process gives the glue module a clean
  // slate so invalid bytes actually fail to compile.
  const wasmPath = join(here, "..", "wasm", "brightfx_wasm_bg.wasm");
  const indexPath = join(here, "..", "dist", "index.js");
  const script = `
    import { BrightFX } from ${JSON.stringify(indexPath)};
    import { readFileSync } from "node:fs";
    let rejected = false;
    try {
      await BrightFX.init(Buffer.from("not wasm"));
    } catch {
      rejected = true;
    }
    if (!rejected) throw new Error("bad source did not reject");
    const retried = await BrightFX.init(readFileSync(${JSON.stringify(wasmPath)}));
    if (!retried) throw new Error("retry with real bytes did not succeed");
    process.stdout.write("OK");
  `;
  const result = spawnSync(process.execPath, ["--input-type=module", "-e", script], { encoding: "utf8" });
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout, "OK");
});

test("init with a second, different source after success throws a source-mismatch error", async () => {
  // `engine` above already resolved `BrightFX.init` with the real wasm
  // bytes as the first source. A fresh Buffer over the identical bytes is
  // still a *different* BufferSource by identity -- the rule the doc
  // comment on `sameSource` documents -- so it must be rejected, not
  // silently reused.
  const bytes = readFileSync(join(here, "..", "wasm", "brightfx_wasm_bg.wasm"));
  await assert.rejects(() => BrightFX.init(bytes), /source that differs/);
});

const cullConfig = JSON.parse(read("ffi-seek.config.json"));

function seekCount(setup, cullMargin = 0) {
  const sim = engine.create(42);
  assert.equal(sim.setConfig({ ...cullConfig, cullMargin }).ok, true);
  setup(sim);
  sim.seek(1.5);
  const count = sim.particleCount();
  const particles = sim.particleList();
  sim.dispose();
  return { count, particles };
}

test("setBounds lets cullMargin remove particles that leave the bounds", () => {
  const margin = 50;
  const width = 200;
  const height = 120;
  const open = seekCount(() => {}, margin);
  const bounded = seekCount((sim) => sim.setBounds(width, height), margin);
  assert.ok(open.count > 0, "test is vacuous: nothing to cull");
  assert.ok(bounded.count > 0, "test is vacuous: the bounds cull everything");
  assert.ok(bounded.count < open.count, `nothing culled: ${bounded.count} of ${open.count}`);
  for (const p of bounded.particles) {
    assert.ok(
      p.x >= -margin && p.x <= width + margin && p.y >= -margin && p.y <= height + margin,
      `(${p.x}, ${p.y}) is outside the bounds plus margin`,
    );
  }
});

test("setViewport sets the same bounds, in logical units", () => {
  const viaBounds = seekCount((sim) => sim.setBounds(100, 60), 50);
  // Margin 50, so some particles survive at 100x60 and the sizes can be told apart.
  // 200x120 device pixels at scale 2 is a 100x60 logical frame.
  const viaViewport = seekCount((sim) => sim.setViewport(200, 120, 2), 50);
  assert.ok(viaBounds.count > 0, "test is vacuous: the 100x60 bounds cull everything");
  const wider = seekCount((sim) => sim.setBounds(200, 120), 50);
  assert.ok(viaBounds.count < wider.count, "test is vacuous: 100x60 culls no more than 200x120, so device px would pass too");
  assert.equal(viaViewport.count, viaBounds.count);
  assert.deepEqual(viaViewport.particles, viaBounds.particles);
});
