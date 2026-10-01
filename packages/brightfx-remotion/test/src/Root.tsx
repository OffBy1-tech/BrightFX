import React from "react";
import { Composition, staticFile, useCurrentFrame } from "remotion";
import { Simulation } from "brightfx-js";
import { BrightFX } from "../../src/index";
import seekConfig from "../../../../core/fixtures/ffi-seek.config.json";
import shared from "../fixtures/probe-effects.json";

const WASM = staticFile("brightfx_wasm_bg.wasm");
type Effect = React.ComponentProps<typeof BrightFX>["effect"];
const effect = seekConfig as unknown as Effect;

const withTriggers = (triggers: unknown[]): Effect =>
  ({ ...effect, emitterTrack: { ...effect.emitterTrack!, triggers } }) as Effect;

// Emission moved past the still's t = 1.0 s, so that frame has no particles.
const lateEffect = withTriggers(shared.lateStartTriggers);
// One burst at 0.5 s and nothing after, so its particles all die mid-range.
const burstOnceEffect = withTriggers(shared.burstOnceTriggers);

// Each probe is a marker revealed when its Simulation method runs. A DOM
// write rather than React state: the sim loads asynchronously and
// re-renders only the component, so a sibling reading a counter would miss
// it. Compositions without the markers are unaffected.
const PROBE_METHODS = { raster: ["render", "frame"], seek: ["seek"] } as const;
for (const [probe, methods] of Object.entries(PROBE_METHODS)) {
  for (const method of methods) {
    const proto = Simulation.prototype as unknown as Record<string, (...args: unknown[]) => unknown>;
    const original = proto[method];
    proto[method] = function (this: Simulation, ...args: unknown[]) {
      const marker = document.getElementById(`probe-${probe}`);
      if (marker) marker.style.display = "block";
      return original.apply(this, args);
    };
  }
}

const Probes: React.FC = () => (
  <>
    {Object.entries(shared.probes).map(([name, { right, color }]) => (
      <div
        key={name}
        id={`probe-${name}`}
        style={{
          display: "none",
          position: "absolute",
          right,
          bottom: 0,
          width: shared.probeSize,
          height: shared.probeSize,
          background: `rgb(${color.join(",")})`,
          zIndex: 1,
        }}
      />
    ))}
  </>
);

const FrameModeTest: React.FC = () => (
  <>
    <BrightFX effect={effect} seed={42} mode="frame" wasmSrc={WASM} />
    <Probes />
  </>
);

const SpriteModeTest: React.FC = () => (
  <BrightFX
    effect={effect}
    seed={42}
    mode="sprite"
    wasmSrc={WASM}
    render={() => <div style={{ width: 4, height: 4, background: "#ff0000" }} />}
  />
);

const EmptyFrameTest: React.FC = () => (
  <>
    <BrightFX effect={lateEffect} seed={42} mode="frame" wasmSrc={WASM} />
    <Probes />
  </>
);

// The seek fixture's track is 2 s long; stills past it must draw nothing.
const PastTrackEndTest: React.FC = () => (
  <>
    <BrightFX effect={effect} seed={42} mode="frame" wasmSrc={WASM} />
    <Probes />
  </>
);

const BurstRangeTest: React.FC = () => <BrightFX effect={burstOnceEffect} seed={42} mode="frame" wasmSrc={WASM} />;

// Sprite mode until frame 30, frame mode from it, with particles live
// across the switch: the first frame-mode render must not draw with the
// sprite-mode simulation, which has no viewport.
const ModeSwitchTest: React.FC = () => {
  const frame = useCurrentFrame();
  return (
    <BrightFX
      effect={effect}
      seed={42}
      mode={frame < 30 ? "sprite" : "frame"}
      wasmSrc={WASM}
      render={() => <div style={{ width: 4, height: 4, background: "#ff0000" }} />}
    />
  );
};

const size = { fps: 30, width: 200, height: 120 };

export const Root: React.FC = () => (
  <>
    <Composition id="FrameModeTest" component={FrameModeTest} durationInFrames={60} {...size} />
    <Composition id="SpriteModeTest" component={SpriteModeTest} durationInFrames={60} {...size} />
    <Composition id="EmptyFrameTest" component={EmptyFrameTest} durationInFrames={60} {...size} />
    <Composition id="PastTrackEndTest" component={PastTrackEndTest} durationInFrames={90} {...size} />
    <Composition id="BurstRangeTest" component={BurstRangeTest} durationInFrames={60} {...size} />
    <Composition id="ModeSwitchTest" component={ModeSwitchTest} durationInFrames={60} {...size} />
  </>
);
