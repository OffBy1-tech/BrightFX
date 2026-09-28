# brightfx-js

A typed TypeScript wrapper over BrightFX's web (wasm) build. It carries no
simulation logic of its own — every method is a one-to-one call into the
core — and exists only to give JS/TS callers a typed, ergonomic surface over
that boundary.

## Install

Not on the npm registry yet. Install from a vendored tarball:

```bash
npm install ./vendor/brightfx-js-0.1.0.tgz
```

## API

```ts
import { BrightFX } from "brightfx-js";

const engine = await BrightFX.init(source);      // URL/string (browser) or bytes (Node)
const sim = engine.create(seed);                 // one Simulation per instance

sim.setConfig(effectConfig);                     // -> {ok:true,clamped:[...]} | {ok:false,error}
sim.setViewport(width, height, scale);            // device pixels, and pixels per logical unit

sim.setEmitter(x, y, vx, vy, active);
sim.triggerBurst();
sim.advance(dt);
sim.seek(time);                                  // requires an emitterTrack in the config

sim.render();
sim.frame();                                     // { width, height, data } straight-alpha RGBA8
sim.particles();                                  // Float32Array view, 8 floats per particle
sim.particleList();                               // same buffer as { x, y, size, rotation, r, g, b, a }[]

engine.fitTrack(config, from, to);                // rescale an emitterTrack between frame sizes
engine.generateCueTracks(input);                  // lyric-cue timing -> EffectJob[]

sim.dispose();                                    // free the wasm handle
```

`BrightFX.init` loads the wasm once per module instance and every later call
shares that same load; if it fails the cache clears so a corrected call can
retry, and calling it again with a source that differs from the first one
(compared by string form for a URL/string, by identity for bytes) rejects
instead of silently reusing what already loaded.

## The two hazards this package owns

The core's frame is premultiplied RGBA8 but the Canvas API takes straight
alpha, so `frame()` unpremultiplies before handing pixels back. And WASM
linear memory can move on any allocating call, detaching every existing
typed-array view, so nothing here caches a view — `particles()` and
`frame()` rebuild theirs from the current memory and pointer on every call.

## The `.wasm` file

The binary ships inside the package at
`node_modules/brightfx-js/wasm/brightfx_wasm_bg.wasm`. A browser or Remotion
host needs to serve or copy it somewhere reachable at runtime — see
`brightfx-remotion`'s README for how that package does it.
