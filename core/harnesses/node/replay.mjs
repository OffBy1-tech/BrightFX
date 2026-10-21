// The fixture replay the three JS harnesses share: the raw-wasm Node harness
// (smoke.mjs), the release consumer smoke (core/scripts/consumer-smoke.mjs),
// and the brightfx-js wrapper test. The raw module and the wrapper name the
// driving methods the same, so both pass their simulation in as it is. How
// to read the buffer differs (readBuffer over linear memory vs particles()),
// so each caller reads it and hands the floats over.
//
// No imports beyond node: builtins. verify-release-macos.sh copies this file
// next to consumer-smoke.mjs in a scratch project.
import assert from "node:assert/strict";

// Replays the emitter states the fixture recorded, one advance per frame,
// with the burst on the recorded frame.
export function drive(sim, { emitterFrames, burstFrame, dt }) {
  emitterFrames.forEach((e, frame) => {
    sim.setEmitter(e.x, e.y, e.vx, e.vy, true);
    if (frame === burstFrame) sim.triggerBurst();
    sim.advance(dt);
  });
}

// Asserts `actual` matches the fixture's `buffer` float for float within its
// `tolerance`, naming the first float that diverges.
export function assertBufferClose(actual, { buffer, tolerance }, label = "buffer") {
  assert.equal(actual.length, buffer.length, `${label}: length diverged`);
  for (let i = 0; i < actual.length; i++) {
    // Finiteness is checked explicitly rather than left to the comparison
    // happening to be written in the NaN-failing direction. The message is
    // only built on failure: this loop runs over every float.
    const ok =
      Number.isFinite(actual[i]) && Number.isFinite(buffer[i]) && Math.abs(actual[i] - buffer[i]) <= tolerance;
    if (!ok) assert.fail(`${label}: float ${i} drifted or is not finite: got ${actual[i]}, expected ${buffer[i]}`);
  }
}
