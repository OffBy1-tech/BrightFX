// Node smoke test for the BrightFX WASM boundary.
// Replays the shared driving protocol from core/fixtures and compares against
// the Rust-generated expectation.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

import * as mod from "../../brightfx-wasm/pkg-node/brightfx_wasm.js";
import { readBuffer, readFrame, applyConfig, unwrap } from "./brightfx.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const fixtures = join(here, "..", "..", "fixtures");
const configJson = readFileSync(join(fixtures, "ffi-smoke.config.json"), "utf8");
const expected = JSON.parse(readFileSync(join(fixtures, "ffi-smoke.expected.json"), "utf8"));

const frameConfigJson = readFileSync(join(fixtures, "ffi-frame.config.json"), "utf8");
const frameExpected = JSON.parse(readFileSync(join(fixtures, "ffi-frame.expected.json"), "utf8"));
const frameBytes = new Uint8Array(readFileSync(join(fixtures, frameExpected.rgbaFile)));

const seekConfigJson = readFileSync(join(fixtures, "ffi-seek.config.json"), "utf8");
const seekExpected = JSON.parse(readFileSync(join(fixtures, "ffi-seek.expected.json"), "utf8"));
const seekForwardExpected = JSON.parse(
  readFileSync(join(fixtures, "ffi-seek-forward.expected.json"), "utf8"),
);
const prerollConfigJson = readFileSync(join(fixtures, "ffi-preroll.config.json"), "utf8");
const prerollExpected = JSON.parse(readFileSync(join(fixtures, "ffi-preroll.expected.json"), "utf8"));

const cueInputJson = readFileSync(join(fixtures, "tracks-cues.input.json"), "utf8");
const cueExpectedText = readFileSync(join(fixtures, "tracks-cues.expected.json"), "utf8");

function test(name, fn) {
  fn();
  console.log(`  ok  ${name}`);
}

// Replays the emitter states the fixture recorded, one advance per frame,
// with the burst on the recorded frame.
function drive(sim, { emitterFrames, burstFrame, dt }) {
  emitterFrames.forEach((e, frame) => {
    sim.setEmitter(e.x, e.y, e.vx, e.vy, true);
    if (frame === burstFrame) sim.triggerBurst();
    sim.advance(dt);
  });
}

function assertBufferMatches(sim, { particleCount, buffer, tolerance }) {
  assert.equal(sim.particleCount(), particleCount, "particle count diverged");
  const actual = readBuffer(sim, mod);
  assert.equal(actual.length, buffer.length, "buffer length diverged");
  for (let i = 0; i < actual.length; i++) {
    // Finiteness is checked explicitly rather than left to the comparison
    // happening to be written in the NaN-failing direction. The message is
    // only built on failure: this loop runs over every float.
    const ok =
      Number.isFinite(actual[i]) && Number.isFinite(buffer[i]) && Math.abs(actual[i] - buffer[i]) <= tolerance;
    if (!ok) assert.fail(`float ${i} drifted or is not finite: got ${actual[i]}, expected ${buffer[i]}`);
  }
}

test("stride matches the Rust constant", () => {
  assert.equal(mod.particleFloats(), expected.particleFloats);
});

test("fixture is not vacuous", () => {
  assert.ok(expected.particleCount > 0, "fixture is vacuous — no particles to compare");
  assert.equal(expected.emitterFrames.length, expected.frames, "emitter frames do not cover every frame");
});

test("the driving protocol reproduces the Rust buffer", () => {
  const sim = new mod.BrightFx(BigInt(expected.seed));
  const clamped = applyConfig(sim, configJson);
  assert.deepEqual(clamped, [], "fixture config should need no clamping");

  drive(sim, expected);
  assertBufferMatches(sim, expected);
  sim.free();
});

test("the seek fixture is not vacuous", () => {
  assert.ok(seekExpected.particleCount > 0, "seek fixture is vacuous — no particles to compare");
  assert.ok(seekExpected.seekTimes.length > 0, "seek fixture never seeks");
});

test("seek reproduces the Rust buffer from a baked track", () => {
  const sim = new mod.BrightFx(BigInt(seekExpected.seed));
  applyConfig(sim, seekConfigJson);
  for (const time of seekExpected.seekTimes) sim.seek(time);
  assertBufferMatches(sim, seekExpected);
  sim.free();
});

test("forward seeks reproduce the Rust buffer and match a fresh seek exactly", () => {
  assert.ok(seekForwardExpected.particleCount > 0, "forward-seek fixture is vacuous");
  const forward = new mod.BrightFx(BigInt(seekForwardExpected.seed));
  applyConfig(forward, seekConfigJson);
  for (const time of seekForwardExpected.seekTimes) forward.seek(time);
  assertBufferMatches(forward, seekForwardExpected);

  const fresh = new mod.BrightFx(BigInt(seekForwardExpected.seed));
  applyConfig(fresh, seekConfigJson);
  fresh.seek(seekForwardExpected.seekTimes.at(-1));
  // Same binary, same steps: bit-identical, no tolerance. `deepStrictEqual`
  // compares with `Object.is`, so -0 and +0 differ as they do bitwise; it
  // also treats NaN as equal to NaN, so finiteness is asserted separately.
  const freshFloats = Array.from(readBuffer(fresh, mod));
  const forwardFloats = Array.from(readBuffer(forward, mod));
  assert.ok(freshFloats.every(Number.isFinite), "fresh-seek buffer is not finite");
  assert.ok(forwardFloats.every(Number.isFinite), "forward-seek buffer is not finite");
  assert.deepStrictEqual(freshFloats, forwardFloats);
  fresh.free();
  forward.free();
});

test("a pre-rolled track is already running at t=0 and steps on bit-identically", () => {
  assert.ok(prerollExpected.particleCount > 0, "pre-roll fixture is vacuous");
  assert.deepEqual(prerollExpected.seekTimes, [0], "the pre-roll fixture records seek(0)");
  const sim = new mod.BrightFx(BigInt(prerollExpected.seed));
  applyConfig(sim, prerollConfigJson);
  for (const time of prerollExpected.seekTimes) sim.seek(time);
  assertBufferMatches(sim, prerollExpected);

  // Leaving the pre-roll is a forward seek: bit-identical to a fresh one.
  sim.seek(0.5);
  const fresh = new mod.BrightFx(BigInt(prerollExpected.seed));
  applyConfig(fresh, prerollConfigJson);
  fresh.seek(0.5);
  const forwardFloats = Array.from(readBuffer(sim, mod));
  const freshFloats = Array.from(readBuffer(fresh, mod));
  assert.ok(forwardFloats.every(Number.isFinite), "forward buffer is not finite");
  assert.ok(freshFloats.every(Number.isFinite), "fresh buffer is not finite");
  assert.deepStrictEqual(freshFloats, forwardFloats);
  sim.free();
  fresh.free();
});

// Drives a simulation to a state with live particles, for the detachment
// tests below.
function simulateSomeParticles() {
  const sim = new mod.BrightFx(1n);
  applyConfig(sim, configJson);
  sim.setEmitter(0, 0, 1.5, 0, true);
  for (let i = 0; i < 40; i++) sim.advance(0.015625);
  return sim;
}

test("a view survives setConfig growing linear memory", () => {
  const sim = simulateSomeParticles();
  const before = readBuffer(sim, mod);
  assert.ok(before.length > 0, "test is vacuous with an empty buffer");
  const expected = Array.from(before);

  // A config whose JSON text alone is larger than linear memory currently
  // is, so applying it must grow memory: the string is copied into the
  // module before it is parsed. Repeatedly applying the fixture config does
  // NOT do this; after the first load its allocations are served from the
  // free list and memory stays put, which left this test passing while
  // checking nothing. Sizing the config against the live memory
  // keeps the growth guaranteed no matter what ran before this test.
  const memory = mod.wasmMemory();
  const keyframes = Math.ceil(memory.buffer.byteLength / 20) + 1;
  const bigConfig = JSON.stringify({
    ...JSON.parse(configJson),
    emitterTrack: {
      duration: 1,
      keyframes: Array.from({ length: keyframes }, (_, i) => ({ time: i / keyframes, x: i, y: 0 })),
      triggers: [{ time: 0, kind: "startContinuous" }],
    },
  });
  assert.ok(bigConfig.length > memory.buffer.byteLength, "config is not larger than linear memory");
  applyConfig(sim, bigConfig);

  // Precondition, not the property under test: the growth actually happened
  // and detached the earlier view. If this fires, the test has stopped
  // exercising detachment; readBuffer has not broken.
  assert.equal(before.length, 0, "setConfig did not grow linear memory, so this test no longer exercises detachment");

  const after = readBuffer(sim, mod);
  assert.equal(after.length, expected.length, "readBuffer returned a detached or stale view");
  assert.deepEqual(Array.from(after), expected, "the rebuilt view does not read the same particles");
  sim.free();
});

test("a view survives linear memory growing for any reason", () => {
  // The same property with the growth forced rather than incidental:
  // growing memory from here is exactly what any allocation inside the
  // module may do, and it detaches every existing view. This is the test
  // that catches a cached view no matter how set_config allocates.
  const sim = simulateSomeParticles();
  const before = readBuffer(sim, mod);
  assert.ok(before.length > 0, "test is vacuous with an empty buffer");
  const expected = Array.from(before);

  mod.wasmMemory().grow(1);
  assert.equal(before.length, 0, "growing memory did not detach the old view");

  const after = readBuffer(sim, mod);
  assert.equal(after.length, expected.length, "readBuffer returned a detached or stale view");
  assert.deepEqual(Array.from(after), expected, "the rebuilt view does not read the same particles");
  sim.free();
});

test("an invalid config is rejected with a message, not a crash", () => {
  const sim = new mod.BrightFx(1n);
  const envelope = JSON.parse(sim.setConfig("{ not json"));
  assert.equal(envelope.ok, false);
  assert.match(envelope.error, /invalid JSON/);
  sim.free();
});

test("a newer schemaVersion is rejected clearly", () => {
  const sim = new mod.BrightFx(1n);
  const future = { ...JSON.parse(configJson), schemaVersion: 999 };
  const envelope = JSON.parse(sim.setConfig(JSON.stringify(future)));
  assert.equal(envelope.ok, false);
  assert.match(envelope.error, /unsupported schemaVersion 999/);
  sim.free();
});

test("the frame fixture is not vacuous", () => {
  assert.ok(frameExpected.nonzeroPixels > 500, "frame fixture is vacuous");
  assert.equal(frameBytes.length, frameExpected.width * frameExpected.height * 4);
});

test("the render protocol reproduces the Rust frame", () => {
  const sim = new mod.BrightFx(BigInt(frameExpected.seed));
  applyConfig(sim, frameConfigJson);
  const viewport = JSON.parse(sim.setViewport(frameExpected.width, frameExpected.height, frameExpected.scale));
  assert.equal(viewport.ok, true, `viewport rejected: ${viewport.error}`);

  drive(sim, frameExpected);
  sim.render();

  assert.equal(sim.frameWidth(), frameExpected.width);
  assert.equal(sim.frameHeight(), frameExpected.height);
  const actual = readFrame(sim, mod);
  assert.equal(actual.length, frameBytes.length, "frame length diverged");

  let differing = 0;
  for (let i = 0; i < actual.length; i += 4) {
    for (let c = 0; c < 4; c++) {
      if (Math.abs(actual[i + c] - frameBytes[i + c]) > frameExpected.channelTolerance) {
        differing++;
        break;
      }
    }
  }
  assert.ok(
    differing <= frameExpected.maxDifferingPixels,
    `${differing} pixels drifted beyond ${frameExpected.channelTolerance} per channel`,
  );
  sim.free();
});

test("a frame view survives setViewport growing linear memory", () => {
  const sim = new mod.BrightFx(1n);
  applyConfig(sim, frameConfigJson);
  sim.setEmitter(50, 50, 1.5, 0, true);
  for (let i = 0; i < 40; i++) sim.advance(0.015625);
  sim.setViewport(64, 64, 1);
  sim.render();
  const before = readFrame(sim, mod);
  assert.ok(before.some((b) => b !== 0), "test is vacuous with an empty frame");

  for (let i = 0; i < 20; i++) sim.setViewport(256 + i, 256, 1);
  sim.render();
  const after = readFrame(sim, mod);
  assert.equal(after.length, 275 * 256 * 4, "readFrame returned a stale view");
  sim.free();
});

test("the cue generator reproduces the Rust output byte for byte through wasm", () => {
  const actual = mod.generateCueTracks(cueInputJson);
  assert.equal(actual + "\n", cueExpectedText, "wasm and native serialized the jobs differently");
  const envelope = JSON.parse(actual);
  assert.equal(envelope.ok, true, envelope.error);
  assert.equal(envelope.jobs.length, 4);
});

test("the cue generator rejects bad input with an envelope", () => {
  const envelope = JSON.parse(mod.generateCueTracks("{ nope"));
  assert.equal(envelope.ok, false);
  assert.match(envelope.error, /invalid cue input/);
});

test("the cue generator rejects a bad cue table with an envelope, not ok:true", () => {
  // An inverted cue would bake a lineup window whose Stop precedes its
  // Start; the sorted replay in seek would then emit to the end of the song.
  const input = JSON.parse(cueInputJson);
  input.cues[3].t = [37.899, 35.185];
  const envelope = JSON.parse(mod.generateCueTracks(JSON.stringify(input)));
  assert.equal(envelope.ok, false);
  assert.match(envelope.error, /cue input rejected: cue 3/);
});

test("fitTrack rescales a track and round-trips", () => {
  const there = unwrap(mod.fitTrack(seekConfigJson, 1920, 1080, 1080, 1920));
  const back = unwrap(mod.fitTrack(JSON.stringify(there.config), 1080, 1920, 1920, 1080));
  const original = JSON.parse(seekConfigJson).emitterTrack.keyframes;
  back.config.emitterTrack.keyframes.forEach((k, i) => {
    assert.ok(Math.abs(k.x - original[i].x) < 1e-3 && Math.abs(k.y - original[i].y) < 1e-3, `keyframe ${i} drifted`);
  });
  assert.equal(there.config.emitterTrack.keyframes[2].x, 120 * (1080 / 1920));
});

test("fitTrack refuses a missing dimension instead of writing null keyframes", () => {
  // wasm-bindgen passes `undefined` straight through as NaN.
  const envelope = JSON.parse(mod.fitTrack(seekConfigJson, 1920, 1080, undefined, 1080));
  assert.equal(envelope.ok, false);
  assert.match(envelope.error, /to\.width/);
});

test("fitTrack applies the same schemaVersion gate as setConfig", () => {
  const config = { ...JSON.parse(seekConfigJson), schemaVersion: 999 };
  const envelope = JSON.parse(mod.fitTrack(JSON.stringify(config), 1920, 1080, 1080, 1920));
  assert.equal(envelope.ok, false);
  assert.match(envelope.error, /unsupported schemaVersion 999/);
});

console.log("node harness: all checks passed");
