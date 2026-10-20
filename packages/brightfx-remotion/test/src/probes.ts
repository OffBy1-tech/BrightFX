// The test instrumentation: patches on Simulation.prototype and the console
// lines the test reads back through renderFrames' onBrowserLog. Imported
// first from index.ts, as a module of side effects of its own, so it runs
// once per page: Fast Refresh in Remotion Studio re-runs Root.tsx when it
// changes, not this. A change to this module is declined below, so Studio
// reloads the page rather than wrap the methods a second time.
import { Simulation } from "brightfx-js";
import shared from "../fixtures/probe-effects.json";

(import.meta as { webpackHot?: { decline(): void } }).webpackHot?.decline();

// One console line, `tag` then JSON. JSON.stringify writes NaN, ±Infinity
// and undefined all as null, so those are written as `{ "$js": "<their JS
// spelling>" }` instead. That keeps them distinct from each other, from a
// null that means "none", and from a string that happens to read "NaN".
export const log = (tag: string, data: unknown) =>
  console.log(
    `${tag} ${JSON.stringify(data, (_, v) => (v === undefined || (typeof v === "number" && !Number.isFinite(v)) ? { $js: String(v) } : v))}`,
  );

const proto = Simulation.prototype as unknown as Record<string, (...args: unknown[]) => unknown>;
// Runs `before` with each call's simulation and arguments, then the method.
function wrap(method: string, before: (sim: Simulation, args: unknown[]) => void) {
  const original = proto[method];
  proto[method] = function (this: Simulation, ...args: unknown[]) {
    before(this, args);
    return original.apply(this, args);
  };
}

// Marks a fresh page, so the test can tell a tab Remotion replaced mid-render.
log(shared.pageLogTag, { href: window.location.href });

// Each probe is a marker revealed when its Simulation method runs. A DOM
// write rather than React state: the sim loads asynchronously and
// re-renders only the component, so a sibling reading a counter would miss
// it. Compositions without the markers are unaffected.
const PROBE_METHODS = { raster: ["render", "frame"], seek: ["seek"] } as const;
for (const [probe, methods] of Object.entries(PROBE_METHODS)) {
  for (const method of methods) {
    wrap(method, () => {
      const marker = document.getElementById(`probe-${probe}`);
      if (marker) marker.style.display = "block";
    });
  }
}

/** The last setBounds/setViewport arguments per simulation, for HookTest's use log. */
export const sizedWith = new WeakMap<Simulation, unknown[]>();

// Every setBounds and setViewport call is logged, with its arguments and
// the page's devicePixelRatio, as one `shared.callLogTag` line. Each call
// is kept, not just the last, since each one replays the track from
// preroll.
for (const method of ["setBounds", "setViewport"]) {
  wrap(method, (sim, args) => {
    log(shared.callLogTag, { method, args, dpr: window.devicePixelRatio });
    sizedWith.set(sim, args);
  });
}
