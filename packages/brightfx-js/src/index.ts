// brightfx-js: a typed layer over the wasm build. It carries no behavior
// of its own -- every method is a call into the core -- but it owns the
// two hazards every JavaScript host would otherwise get wrong:
//
//   1. The core's frame is premultiplied RGBA8 and the canvas API takes
//      straight alpha. `frame()` unpremultiplies.
//   2. WASM linear memory can grow on any allocating call, which detaches
//      every existing typed-array view. Nothing here caches a view; each
//      accessor rebuilds it from the current memory and pointer.

import init, { BrightFx, fitTrack, generateCueTracks, particleFloats, wasmMemory } from "../wasm/brightfx_wasm.js";

export type ConfigResult = { ok: true; clamped: string[]; warnings: string[] } | { ok: false; error: string };

export interface Particle {
  x: number;
  y: number;
  size: number;
  rotation: number;
  r: number;
  g: number;
  b: number;
  a: number;
}

/** Straight-alpha RGBA8, row-major, `width * 4` bytes per row: exactly what
 *  `new ImageData(data, width, height)` accepts. */
export interface FramePixels {
  width: number;
  height: number;
  data: Uint8ClampedArray;
}

export interface EmitterKeyframe {
  time: number;
  x: number;
  y: number;
  vx?: number | null;
  vy?: number | null;
}

export interface EmitterTrigger {
  time: number;
  kind: "burst" | "startContinuous" | "stopContinuous";
}

export interface EmitterTrack {
  duration: number;
  keyframes: EmitterKeyframe[];
  triggers: EmitterTrigger[];
}

/**
 * Which way particles turn. Each particle draws a signed speed `v` from
 * `rotationSpeedMin..rotationSpeedMax`. `"fixed"` turns at `v` as drawn, so
 * the range's sign sets the direction (a range spanning zero gives both).
 * `"random"` turns at `v` or `-v` by a coin flip at spawn, so speeds are
 * symmetric about zero.
 */
export type SpinDirection = "fixed" | "random";

/** A `.brightfx.json` config. Typed loosely: the core validates it. */
export type EffectConfig = {
  schemaVersion: number;
  emitterTrack?: EmitterTrack | null;
  /** Schema version 3+. Omitted means `"fixed"`. */
  spinDirection?: SpinDirection;
  /** Schema version 4+. Logical px. Removes a particle once its center is
   *  more than this far outside the simulation's bounds (`setViewport` or
   *  `setBounds`), after it has been inside that area once. A particle
   *  outside the area that is moving away from it is removed too, so an
   *  emitter placed off-screen works with any margin. Omitted or `null`
   *  never culls. */
  cullMargin?: number | null;
} & Record<string, unknown>;

export interface Cue {
  t: [number, number];
  text?: string;
  who: string;
  mood?: string;
}

export interface Point {
  x: number;
  y: number;
}

export interface CueLayout {
  cards?: Record<string, Point>;
  lineup: Point;
  accent?: Record<string, string>;
  lineupTokens?: string[];
}

export interface CueInput {
  cues: Cue[];
  layout: CueLayout;
  duration: number;
  heroTimes?: number[];
}

export interface EffectJob {
  role: "entrance" | "lineup" | "hero";
  who?: string;
  color?: string;
  track: EmitterTrack;
}

const PARTICLE_FIELDS = 8;

/** Converts premultiplied RGBA8 to straight alpha. Fully transparent
 *  pixels come out as zeros. */
export function unpremultiply(src: Uint8Array): Uint8ClampedArray {
  const out = new Uint8ClampedArray(src.length);
  for (let i = 0; i < src.length; i += 4) {
    const a = src[i + 3];
    if (a === 0) continue;
    if (a === 255) {
      out[i] = src[i];
      out[i + 1] = src[i + 1];
      out[i + 2] = src[i + 2];
    } else {
      const k = 255 / a;
      out[i] = Math.round(src[i] * k);
      out[i + 1] = Math.round(src[i + 1] * k);
      out[i + 2] = Math.round(src[i + 2] * k);
    }
    out[i + 3] = a;
  }
  return out;
}

let ready: Promise<BrightFX> | null = null;
let readySource: string | URL | BufferSource | null = null;

/** `true` when `a` and `b` would make `BrightFX.init` load the same wasm.
 *  A string or URL is compared by its string form (so a `staticFile(...)`
 *  call and the URL it returns compare equal); a `BufferSource` is
 *  compared by identity (`===`), not by content, since hashing the bytes
 *  on every call would be wasteful and identity is enough to catch the
 *  mistake `BrightFX.init` is guarding against. */
function sameSource(a: string | URL | BufferSource, b: string | URL | BufferSource): boolean {
  const aIsText = typeof a === "string" || a instanceof URL;
  const bIsText = typeof b === "string" || b instanceof URL;
  if (aIsText || bIsText) return aIsText && bIsText && String(a) === String(b);
  return a === b;
}

export class BrightFX {
  private constructor() {}

  /** Loads the wasm once per module instance; later calls share the same
   *  promise. `source` is the `.wasm` URL (a browser, or Remotion's
   *  `staticFile(...)`) or its bytes (Node).
   *
   *  A failed load clears the cache, so a later call with a corrected
   *  source retries instead of replaying the same rejection forever.
   *  Once a load has started, every later call must name the *same*
   *  source -- see `sameSource` for how "same" is decided -- or the
   *  returned promise rejects with an `Error` explaining the mismatch,
   *  rather than silently reusing the first call's wasm. */
  static async init(source: string | URL | BufferSource): Promise<BrightFX> {
    if (ready) {
      if (!sameSource(readySource!, source)) {
        throw new Error(
          "BrightFX.init: called with a source that differs from the one already loading/loaded; " +
            "BrightFX loads once per module instance and cannot be re-initialized with a different source",
        );
      }
      return ready;
    }
    readySource = source;
    ready = init({ module_or_path: source as never }).then(() => new BrightFX());
    try {
      return await ready;
    } catch (error) {
      ready = null;
      readySource = null;
      throw error;
    }
  }

  create(seed: number | bigint): Simulation {
    return new Simulation(new BrightFx(BigInt(seed)));
  }

  fitTrack(config: EffectConfig, from: [number, number], to: [number, number]): EffectConfig {
    const envelope = JSON.parse(fitTrack(JSON.stringify(config), from[0], from[1], to[0], to[1]));
    if (!envelope.ok) throw new Error(envelope.error);
    return envelope.config as EffectConfig;
  }

  generateCueTracks(input: CueInput): EffectJob[] {
    const envelope = JSON.parse(generateCueTracks(JSON.stringify(input)));
    if (!envelope.ok) throw new Error(envelope.error);
    return envelope.jobs as EffectJob[];
  }
}

export class Simulation {
  private raw: BrightFx | null;

  /** @internal Consumers get a `Simulation` from `BrightFX.create`, never
   *  by constructing one directly. */
  constructor(raw: BrightFx) {
    this.raw = raw;
  }

  private get sim(): BrightFx {
    if (!this.raw) throw new Error("BrightFX simulation is disposed");
    return this.raw;
  }

  setConfig(config: EffectConfig | string): ConfigResult {
    const json = typeof config === "string" ? config : JSON.stringify(config);
    return JSON.parse(this.sim.setConfig(json)) as ConfigResult;
  }

  setViewport(width: number, height: number, scale = 1): ConfigResult {
    return JSON.parse(this.sim.setViewport(width, height, scale)) as ConfigResult;
  }

  /** The logical-pixel size `cullMargin` is measured from, for a host that
   *  draws its own sprites and never calls `setViewport` (which sets it
   *  itself, as `width / scale` by `height / scale`). Changing it makes the
   *  next `seek` replay from zero. A non-finite or non-positive size
   *  (including `undefined`, which wasm-bindgen turns into NaN) clears the
   *  bounds and so turns culling off. When a host calls both `setViewport`
   *  and `setBounds`, the last call wins. */
  setBounds(width: number, height: number): void {
    this.sim.setBounds(width, height);
  }

  setEmitter(x: number, y: number, vx: number, vy: number, active: boolean): void {
    this.sim.setEmitter(x, y, vx, vy, active);
  }

  triggerBurst(): void {
    this.sim.triggerBurst();
  }

  advance(dt: number): void {
    this.sim.advance(dt);
  }

  seek(time: number): void {
    this.sim.seek(time);
  }

  render(): void {
    this.sim.render();
  }

  particleCount(): number {
    return this.sim.particleCount();
  }

  /** A view over the live particle buffer, `particleFloats()` floats per
   *  particle. Valid until the next `seek`/`advance`/`triggerBurst`/
   *  `setConfig`/`setBounds`/`setViewport`; never cache it. */
  particles(): Float32Array {
    const sim = this.sim;
    // Count, floats, ptr, then memory: same rule as `frame()` -- any
    // allocating call can move linear memory, so the pointer is read as
    // late as possible and the buffer view is built last of all.
    const count = sim.particleCount();
    const floats = particleFloats();
    const ptr = sim.bufferPtr();
    return new Float32Array(wasmMemory().buffer, ptr, count * floats);
  }

  particleList(): Particle[] {
    const stride = particleFloats();
    if (stride !== PARTICLE_FIELDS) {
      throw new Error(`unsupported particle stride ${stride}; this wrapper knows ${PARTICLE_FIELDS}`);
    }
    const v = this.particles();
    const out: Particle[] = new Array(v.length / stride);
    for (let i = 0, o = 0; o < v.length; i++, o += stride) {
      out[i] = { x: v[o], y: v[o + 1], size: v[o + 2], rotation: v[o + 3], r: v[o + 4], g: v[o + 5], b: v[o + 6], a: v[o + 7] };
    }
    return out;
  }

  /** The last rendered frame, straight alpha, as a fresh copy. */
  frame(): FramePixels {
    const sim = this.sim;
    const width = sim.frameWidth();
    const height = sim.frameHeight();
    // Scalars first, memory last: any allocating call can move linear
    // memory, and a buffer captured before it would be the old one.
    const ptr = sim.framePtr();
    const len = sim.frameLen();
    const src = new Uint8Array(wasmMemory().buffer, ptr, len);
    return { width, height, data: unpremultiply(src) };
  }

  isPoisoned(): boolean {
    return this.sim.isPoisoned();
  }

  dispose(): void {
    this.raw?.free();
    this.raw = null;
  }
}
