// brightfx-remotion: translates Remotion's clock and composition size into
// wrapper calls. No behavior of its own -- what particles do is decided by
// the effect config, the core, and the tracks crate. What it does decide
// is cost: a frame with no particles is not rasterized, and a frame past
// the track's end is not even seeked.
//
// Determinism: each Remotion tab holds its own wasm instance and each
// component instance its own simulation, replayed from its own seed and
// track. A frame's pixels do not depend on which tab drew it or which
// frames it drew before.

import React, { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { AbsoluteFill, cancelRender, continueRender, delayRender, staticFile, useCurrentFrame, useVideoConfig } from "remotion";
import { BrightFX as Engine, type EffectConfig, type Particle, type Simulation } from "brightfx-js";

/** Where the wasm binary is expected under the project's `public/`
 *  directory, loaded through `staticFile`. */
export const DEFAULT_WASM_PATH = "brightfx/brightfx_wasm_bg.wasm";

/** Below this `amount` the component renders nothing and skips the seek,
 *  matching the compositions' existing `amount <= 0.02` early return. */
export const VISIBLE_THRESHOLD = 0.02;

/** The core's `MAX_EMITTER_TRACK_DURATION`, in seconds. `seek` clamps its
 *  time to `min(emitterTrack.duration, this)`, so any later time replays
 *  the track's last state; brightfx-js does not export the constant. */
const MAX_EMITTER_TRACK_DURATION = 600;

export interface BrightFXProps {
  /** A complete effect config carrying an `emitterTrack`. */
  effect: EffectConfig;
  seed?: number;
  /** 0..1 opacity multiplier; the compositions' existing "amount" values. */
  amount?: number;
  /** Seconds; outside it nothing renders and no seek happens. For an
   *  effect with `spawnRateIdle` 0 it is optional: a frame with no
   *  particles skips rasterizing, so the effect can stay mounted through
   *  its quiet stretches. An effect with idle emission spawns between
   *  cues, so it rarely skips, and without `window` those idle particles
   *  show. Either way, don't end it at an effect's last trigger -- that
   *  cuts off the particles still alive after it. Past the emitter
   *  track's duration nothing renders, window or not. */
  window?: [number, number];
  /** `frame`: blit the rasterized frame. `sprite`: call `render` per particle. */
  mode?: "frame" | "sprite";
  /** Sprite mode only: renders one element per particle. The element's
   *  `key` is the particle's index in that frame's list, which is
   *  unstable across frames -- particles are not tracked by identity, so
   *  the same index can name a different particle next frame -- so the
   *  rendered element must not carry CSS transitions or hold state that
   *  depends on being the "same" element across renders. */
  render?: (particle: Particle, index: number) => React.ReactNode;
  /** Overrides `staticFile(DEFAULT_WASM_PATH)`. */
  wasmSrc?: string;
  style?: React.CSSProperties;
}

/** One simulation per mounted component. Holds Remotion's render until the
 *  wasm has initialized and the config is loaded, then releases it.
 *
 *  `viewport` allocates a frame (frame mode). `bounds` is for a host that
 *  draws its own sprites: it gives the simulation a size for `cullMargin`
 *  without allocating a frame. A viewport sets the bounds itself, so
 *  `bounds` is ignored when a `viewport` is given.
 *
 *  Returns null until a simulation built for exactly this `viewport` and
 *  `bounds` has loaded. When either changes (a sprite/frame mode switch, or a new
 *  composition size), the render that sees the change still holds the old
 *  simulation -- the effect that replaces it runs after that render -- and
 *  drawing with it would rasterize the wrong size, or 0x0 for a sprite-mode
 *  simulation. That render gets null instead; the replacement's
 *  `delayRender` holds the frame until it arrives. */
export function useBrightFX(
  effect: EffectConfig,
  seed: number,
  wasmSrc: string,
  viewport: { width: number; height: number } | null,
  bounds: { width: number; height: number } | null = null,
): Simulation | null {
  const [loaded, setLoaded] = useState<{
    sim: Simulation;
    width: number;
    height: number;
    boundsWidth: number;
    boundsHeight: number;
  } | null>(null);
  const effectJson = useMemo(() => JSON.stringify(effect), [effect]);
  const viewportWidth = viewport?.width ?? 0;
  const viewportHeight = viewport?.height ?? 0;
  const boundsWidth = bounds?.width ?? 0;
  const boundsHeight = bounds?.height ?? 0;

  useEffect(() => {
    const handle = delayRender("BrightFX: loading wasm and config");
    let created: Simulation | null = null;
    let cancelled = false;
    // The handle is released exactly once: by the load on success, by the
    // cleanup if the component unmounts first. Otherwise an unmount during
    // the wasm load would leave Remotion waiting on it until it timed out.
    let released = false;
    const release = () => {
      if (!released) {
        released = true;
        continueRender(handle);
      }
    };

    Engine.init(wasmSrc)
      .then((engine) => {
        if (cancelled) return;
        if (effect.emitterTrack == null) {
          throw new Error("BrightFX: effect has no emitterTrack; baked playback needs one");
        }
        created = engine.create(seed);
        const result = created.setConfig(effectJson);
        if (!result.ok) throw new Error(`BrightFX config rejected: ${result.error}`);
        if (result.clamped.length > 0) {
          console.warn(`BrightFX clamped ${result.clamped.length} value(s): ${result.clamped.join(", ")}`);
        }
        for (const warning of result.warnings) console.warn(`BrightFX: ${warning}`);
        if (viewportWidth > 0 && viewportHeight > 0) {
          const v = created.setViewport(viewportWidth, viewportHeight, 1);
          if (!v.ok) throw new Error(`BrightFX viewport rejected: ${v.error}`);
        } else if (boundsWidth > 0 && boundsHeight > 0) {
          // Only without a viewport: a viewport already set the bounds, and
          // `setBounds` after it would replace them with a different size.
          created.setBounds(boundsWidth, boundsHeight);
        }
        setLoaded({ sim: created, width: viewportWidth, height: viewportHeight, boundsWidth, boundsHeight });
        release();
      })
      .catch((error: unknown) => cancelRender(error));

    return () => {
      cancelled = true;
      release();
      created?.dispose();
      setLoaded(null);
    };
  }, [effectJson, seed, wasmSrc, viewportWidth, viewportHeight, boundsWidth, boundsHeight]);

  return loaded &&
    loaded.width === viewportWidth &&
    loaded.height === viewportHeight &&
    loaded.boundsWidth === boundsWidth &&
    loaded.boundsHeight === boundsHeight
    ? loaded.sim
    : null;
}

export const BrightFX: React.FC<BrightFXProps> = ({
  effect,
  seed = 1,
  amount = 1,
  window: timeWindow,
  mode = "frame",
  render,
  wasmSrc,
  style,
}) => {
  const frame = useCurrentFrame();
  const { fps, width, height } = useVideoConfig();
  const src = wasmSrc ?? staticFile(DEFAULT_WASM_PATH);
  const sim = useBrightFX(
    effect,
    seed,
    src,
    mode === "frame" ? { width, height } : null,
    mode === "sprite" ? { width, height } : null,
  );
  const canvasRef = useRef<HTMLCanvasElement>(null);

  const time = frame / fps;
  const inWindow = !timeWindow || (time >= timeWindow[0] && time < timeWindow[1]);
  // Strictly past the end: the frame at exactly `duration` is the track's
  // real final state, every later one a frozen replay of it.
  const trackEnd = Math.min(effect.emitterTrack?.duration ?? Infinity, MAX_EMITTER_TRACK_DURATION);
  const visible = amount > VISIBLE_THRESHOLD && inWindow && time <= trackEnd;

  // Seeking happens during render so the output is a function of `time`
  // alone. Forward seeks step from the previous frame; a skipped frame
  // (invisible) just makes the next step longer. Every visible frame is
  // seeked; rasterizing, copying, and unpremultiplying a full frame is
  // skipped when the seek leaves no particles -- for an effect without
  // idle emission, most of its timeline between cues.
  const pixels = useMemo(() => {
    if (!sim || !visible || mode !== "frame") return null;
    sim.seek(time);
    if (sim.particleCount() === 0) return null;
    sim.render();
    return sim.frame();
  }, [sim, visible, mode, time]);

  const particles = useMemo(() => {
    if (!sim || !visible || mode !== "sprite") return [];
    sim.seek(time);
    return sim.particleList();
  }, [sim, visible, mode, time]);

  // Draw before paint, so Remotion's screenshot sees this frame's pixels.
  useLayoutEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas || !pixels) return;
    // A rendered frame that is 0x0 or short is a broken viewport, not a
    // frame with no particles (those never reach here: `pixels` is null).
    if (pixels.width === 0 || pixels.height === 0 || pixels.data.length !== pixels.width * pixels.height * 4) {
      cancelRender(new Error("BrightFX: rendered frame is 0x0 or truncated (viewport unset?)"));
      return;
    }
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    // TS 5.7+'s lib.dom.d.ts types ImageData's constructor as taking a
    // Uint8ClampedArray<ArrayBuffer> specifically, while brightfx-js's
    // FramePixels declares the unparameterized Uint8ClampedArray. The data is
    // always a fresh copy that `unpremultiply` allocated, never a view over
    // wasm memory, so the narrower type is true at runtime.
    ctx.putImageData(new ImageData(pixels.data as Uint8ClampedArray<ArrayBuffer>, pixels.width, pixels.height), 0, 0);
  }, [pixels]);

  if (!sim || !visible) return null;

  // The wrapper stays mounted on every visible frame, since it carries the
  // host's `style` and `opacity`; only its contents come and go with the
  // particles.
  if (mode === "sprite") {
    if (!render) throw new Error("BrightFX: sprite mode needs a render prop");
    return (
      <AbsoluteFill style={{ pointerEvents: "none", opacity: amount, ...style }}>
        {particles.map((p, i) => (
          <div
            key={i}
            style={{ position: "absolute", left: p.x, top: p.y, transform: "translate(-50%, -50%)", opacity: p.a }}
          >
            {render(p, i)}
          </div>
        ))}
      </AbsoluteFill>
    );
  }

  // No pixels means no particles: unmount the canvas rather than leave the
  // previous frame's drawing on it.
  return (
    <AbsoluteFill style={{ pointerEvents: "none", opacity: amount, ...style }}>
      {pixels && <canvas ref={canvasRef} width={width} height={height} style={{ width, height }} />}
    </AbsoluteFill>
  );
};
