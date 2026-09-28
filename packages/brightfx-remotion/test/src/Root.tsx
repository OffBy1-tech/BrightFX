import React from "react";
import { Composition, staticFile } from "remotion";
import { Simulation } from "brightfx-js";
import { BrightFX } from "../../src/index";
import seekConfig from "../../../../core/fixtures/ffi-seek.config.json";

const WASM = staticFile("brightfx_wasm_bg.wasm");
const effect = seekConfig as unknown as React.ComponentProps<typeof BrightFX>["effect"];

// The same effect with its emission moved past the still's t = 1.0 s, so
// that frame has zero particles.
const lateEffect = {
  ...effect,
  emitterTrack: {
    ...effect.emitterTrack!,
    triggers: [
      { time: 1.5, kind: "startContinuous" },
      { time: 1.8, kind: "burst" },
    ],
  },
} as typeof effect;

// Reveals `#raster-probe` when the component rasterizes or reads a frame.
// A DOM write rather than React state: the sim loads asynchronously and
// re-renders only the component, so a sibling reading a counter would miss
// it. Other compositions have no probe element, so this is a no-op there.
const PROBE_ID = "raster-probe";
for (const method of ["render", "frame"] as const) {
  const original = Simulation.prototype[method] as (this: Simulation) => unknown;
  (Simulation.prototype as unknown as Record<string, unknown>)[method] = function (this: Simulation) {
    const probe = document.getElementById(PROBE_ID);
    if (probe) probe.style.display = "block";
    return original.call(this);
  };
}

const Probe: React.FC = () => (
  <div id={PROBE_ID} style={{ display: "none", position: "absolute", right: 0, bottom: 0, width: 6, height: 6, background: "#0000ff", zIndex: 1 }} />
);

const EmptyFrameTest: React.FC = () => (
  <>
    <BrightFX effect={lateEffect} seed={42} mode="frame" wasmSrc={WASM} />
    <Probe />
  </>
);

const ActiveFrameProbeTest: React.FC = () => (
  <>
    <BrightFX effect={effect} seed={42} mode="frame" wasmSrc={WASM} />
    <Probe />
  </>
);

const FrameModeTest: React.FC = () => <BrightFX effect={effect} seed={42} mode="frame" wasmSrc={WASM} />;

const SpriteModeTest: React.FC = () => (
  <BrightFX
    effect={effect}
    seed={42}
    mode="sprite"
    wasmSrc={WASM}
    render={() => <div style={{ width: 4, height: 4, background: "#ff0000" }} />}
  />
);

export const Root: React.FC = () => (
  <>
    <Composition id="FrameModeTest" component={FrameModeTest} durationInFrames={60} fps={30} width={200} height={120} />
    <Composition id="SpriteModeTest" component={SpriteModeTest} durationInFrames={60} fps={30} width={200} height={120} />
    <Composition id="EmptyFrameTest" component={EmptyFrameTest} durationInFrames={60} fps={30} width={200} height={120} />
    <Composition id="ActiveFrameProbeTest" component={ActiveFrameProbeTest} durationInFrames={60} fps={30} width={200} height={120} />
  </>
);
