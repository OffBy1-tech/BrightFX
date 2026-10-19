import React from "react";
import { Composition, staticFile, useCurrentFrame } from "remotion";
import { Simulation } from "brightfx-js";
import { BrightFX, useBrightFX } from "../../src/index";
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
const proto = Simulation.prototype as unknown as Record<string, (...args: unknown[]) => unknown>;
// Runs `before` with each call's simulation and arguments, then the method.
function wrap(method: string, before: (sim: Simulation, args: unknown[]) => void) {
  const original = proto[method];
  proto[method] = function (this: Simulation, ...args: unknown[]) {
    before(this, args);
    return original.apply(this, args);
  };
}

const PROBE_METHODS = { raster: ["render", "frame"], seek: ["seek"] } as const;
for (const [probe, methods] of Object.entries(PROBE_METHODS)) {
  for (const method of methods) {
    wrap(method, () => {
      const marker = document.getElementById(`probe-${probe}`);
      if (marker) marker.style.display = "block";
    });
  }
}

// Every setBounds and setViewport call is also logged, with its arguments
// and the page's devicePixelRatio, as one `shared.callLogTag` line that
// `remotion render --log=verbose` forwards to the test (`remotion still`
// does not forward the render tab's console). Each call is kept, not just
// the last, since each one replays the track from preroll. JSON.stringify
// writes NaN as null, so a NaN size cannot pass for a number. The last
// call's arguments are also kept per simulation, for HookTest's use log.
const sizedWith = new WeakMap<Simulation, unknown[]>();
for (const method of ["setBounds", "setViewport"]) {
  wrap(method, (sim, args) => {
    console.log(`${shared.callLogTag} ${JSON.stringify({ method, args, dpr: window.devicePixelRatio })}`);
    sizedWith.set(sim, args);
  });
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

// Sprite mode until the switch frame, frame mode from it, with particles
// live across the switch: the first frame-mode render must not draw with
// the sprite-mode simulation, which has no viewport.
const ModeSwitchTest: React.FC = () => {
  const frame = useCurrentFrame();
  return (
    <BrightFX
      effect={effect}
      seed={42}
      mode={frame < shared.switchFrame ? "sprite" : "frame"}
      wasmSrc={WASM}
      render={() => <div style={{ width: 4, height: 4, background: "#ff0000" }} />}
    />
  );
};

// Calls the hook directly with the given viewport and bounds. Each render
// that gets a simulation logs, as a `shared.useLogTag` line, the frame and
// the size that simulation was built with, so a stale one shows.
const HookTest: React.FC<{
  viewport: { width: number; height: number } | null;
  bounds: { width: number; height: number } | null;
}> = ({ viewport, bounds }) => {
  const frame = useCurrentFrame();
  const sim = useBrightFX(effect, 42, WASM, viewport, bounds);
  if (sim) console.log(`${shared.useLogTag} ${JSON.stringify({ frame, args: sizedWith.get(sim) ?? null })}`);
  return null;
};

// A container measured before layout: rejected, warned about, not fatal.
const ZeroBoundsTest: React.FC = () => <HookTest viewport={null} bounds={{ width: 0, height: 0 }} />;
// Bounds are ignored beside a viewport, so a NaN there must not matter.
const ViewportNaNBoundsTest: React.FC = () => (
  <HookTest viewport={{ width: 200, height: 120 }} bounds={{ width: NaN, height: NaN }} />
);

// A host whose measured size arrives late: 0 before the switch frame, then
// larger than the composition. The simulation built for 0 must be
// replaced, not kept.
const GrowingBoundsTest: React.FC = () => {
  const frame = useCurrentFrame();
  return (
    <HookTest
      viewport={null}
      bounds={frame < shared.switchFrame ? { width: 0, height: 0 } : { width: 640, height: 360 }}
    />
  );
};
// The same for a viewport, which is rebuilt by its own dependencies.
const GrowingViewportTest: React.FC = () => {
  const frame = useCurrentFrame();
  return (
    <HookTest
      viewport={frame < shared.switchFrame ? { width: 100, height: 60 } : { width: 640, height: 360 }}
      bounds={null}
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
    <Composition id="ZeroBoundsTest" component={ZeroBoundsTest} durationInFrames={60} {...size} />
    <Composition id="ViewportNaNBoundsTest" component={ViewportNaNBoundsTest} durationInFrames={60} {...size} />
    <Composition id="GrowingBoundsTest" component={GrowingBoundsTest} durationInFrames={60} {...size} />
    <Composition id="GrowingViewportTest" component={GrowingViewportTest} durationInFrames={60} {...size} />
  </>
);
