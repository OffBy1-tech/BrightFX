// Reusable helper for reading BrightFX's particle buffer from JavaScript.
//
// The trap this exists to solve: `setConfig` allocates, which can grow WASM
// linear memory and DETACH any existing Float32Array view over it. A detached
// view reads as empty, so the symptom is "the particles vanished" rather than
// an error. Always read through `readBuffer`, which rebuilds the view whenever
// memory has moved.

/**
 * Returns a Float32Array view over the live particle buffer.
 * The view is valid until the next advance/seek/triggerBurst/setConfig.
 *
 * The view is rebuilt on every call rather than cached. That is deliberate:
 * the particle count changes almost every frame and linear memory can move on
 * any allocation, so a cache would be invalidated nearly every time it was
 * consulted. Constructing a typed-array view is a few nanoseconds; a stale or
 * detached one is a silent rendering bug.
 *
 * @param {object} sim  a BrightFx instance
 * @param {object} mod  the wasm module namespace (for wasmMemory() and particleFloats())
 * @returns {Float32Array}
 */
export function readBuffer(sim, mod) {
  // Read the scalars before touching `.buffer`. None of these three calls
  // allocates today, so the order doesn't matter yet -- but if one ever
  // became allocating it could grow linear memory and detach a `.buffer`
  // captured before it ran. Taking `.buffer` last means that can never
  // happen silently.
  const ptr = sim.bufferPtr();
  const count = sim.particleCount();
  const floats = mod.particleFloats();
  const memory = mod.wasmMemory().buffer;
  return new Float32Array(memory, ptr, count * floats);
}

/**
 * Parses any `{ok, ...}` envelope the module returns (setConfig,
 * setViewport, fitTrack, generateCueTracks), throwing with the message on
 * failure so a caller never dereferences a field that is not there.
 */
export function unwrap(json, what = "call") {
  const envelope = JSON.parse(json);
  if (!envelope.ok) {
    throw new Error(`${what} rejected: ${envelope.error}`);
  }
  return envelope;
}

/** Applies a config, returning the clamped field list. */
export function applyConfig(sim, json) {
  return unwrap(sim.setConfig(json), "config").clamped;
}

/**
 * Returns a Uint8Array view over the live frame: premultiplied RGBA8,
 * row-major, `frameWidth() * 4` bytes per row. Valid until the next
 * render/setViewport/setConfig. Rebuilt on every call for the same reason
 * `readBuffer` is: any allocation can move linear memory and detach a
 * cached view.
 *
 * To draw it: `new ImageData(new Uint8ClampedArray(frame), width, height)`
 * after unpremultiplying, or upload it as a premultiplied texture.
 *
 * @param {object} sim  a BrightFx instance
 * @param {object} mod  the wasm module namespace (for wasmMemory())
 * @returns {Uint8Array}
 */
export function readFrame(sim, mod) {
  const ptr = sim.framePtr();
  const len = sim.frameLen();
  const memory = mod.wasmMemory().buffer;
  return new Uint8Array(memory, ptr, len);
}
