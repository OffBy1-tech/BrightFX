# brightfx-remotion

A Remotion component that drives a BrightFX effect from Remotion's frame
clock. No behavior of its own — what the particles do is decided by the
effect config, the core, and the tracks crate — just the glue between
`useCurrentFrame()`/`useVideoConfig()` and the wrapper's `seek`/`render`.

## Install

Not on the npm registry yet, and `brightfx-remotion` depends on
`brightfx-js@^0.1.0`, which also isn't on the registry — npm can't resolve
that dependency from a lone `brightfx-remotion` tarball, so install both
tarballs **in the same `npm install` command**:

```bash
npm install ./vendor/brightfx-js-0.1.0.tgz ./vendor/brightfx-remotion-0.2.0.tgz
```

Then copy (never symlink) the wasm binary into your `public/` directory,
where `staticFile(DEFAULT_WASM_PATH)` expects it:

```bash
mkdir -p public/brightfx
cp node_modules/brightfx-js/wasm/brightfx_wasm_bg.wasm public/brightfx/brightfx_wasm_bg.wasm
```

## `remotion.config.ts`

Add this webpack override, verbatim:

```ts
import { Config } from "@remotion/cli/config";

// The wrapper's wasm glue references its binary with `new URL(..., import.meta.url)`
// for the default path we never take. Leave it as an asset rather than a
// WebAssembly module so webpack does not try to instantiate it at bundle time.
Config.overrideWebpackConfig((config) => ({
  ...config,
  experiments: { ...(config.experiments ?? {}), asyncWebAssembly: false, syncWebAssembly: false },
  module: {
    ...config.module,
    rules: [...(config.module?.rules ?? []), { test: /\.wasm$/, type: "asset/resource" }],
  },
}));
```

Without it, webpack tries to treat the `.wasm` import as a WebAssembly
module to instantiate at bundle time instead of an asset to fetch at
runtime, and the bundle fails.

## Usage

Frame mode blits the rasterized frame onto a canvas:

```tsx
<BrightFX effect={confettiEffect} seed={1} amount={1} />
```

Sprite mode calls `render` once per particle instead:

```tsx
<BrightFX
  effect={confettiEffect}
  mode="sprite"
  render={(particle, index) => (
    <div style={{ width: particle.size, height: particle.size, background: "gold", borderRadius: "50%" }} />
  )}
/>
```

## Props

| Prop | Type | Default | Notes |
|---|---|---|---|
| `effect` | `EffectConfig` | required | Must carry an `emitterTrack` — baked playback needs one. |
| `seed` | `number` | `1` | Passed to `BrightFX.create`. |
| `amount` | `number` | `1` | 0..1 opacity multiplier; at or below `0.02` the component renders nothing and skips the seek. |
| `window` | `[number, number]` | — | Seconds; outside it nothing renders and no seek happens. Often optional — see [Empty frames](#empty-frames). |
| `mode` | `"frame" \| "sprite"` | `"frame"` | `frame` blits the rasterized frame; `sprite` calls `render` per particle. |
| `render` | `(particle, index) => ReactNode` | — | Required in sprite mode. |
| `wasmSrc` | `string` | `staticFile(DEFAULT_WASM_PATH)` | Overrides where the wasm is fetched from. |
| `style` | `CSSProperties` | — | Merged onto the wrapping `AbsoluteFill`. |

## Empty frames

A frame with no particles costs a seek and nothing else: frame mode skips
rasterizing and drawing it, and neither mode renders any particle element
(the wrapper carrying `style` and `amount` stays). For an effect with
`spawnRateIdle` 0, quiet stretches between its cues are such frames, so it
can stay mounted and there is no need to gate it with `window` by the span
of its triggers. An effect with idle emission spawns between cues, so it
rarely skips a frame, and dropping `window` shows those idle particles.

Either way, don't end `window` at an effect's last trigger: its particles
outlive that trigger by up to `lifetimeMax / 60` seconds, and gating there
cuts them off. Past the emitter track's `duration` (at most 600 s) the
component renders nothing and skips the seek, since every later time would
replay the track's last state frozen.

## Sprite mode's unstable key

In sprite mode, `render` is called with an **unstable index key** — particles
are not tracked by identity, so the same index can name a different particle
on the next frame. Glyphs rendered from `render` must not carry CSS
transitions or hold state across renders; treat each call as drawing a fresh,
stateless element for the current frame only.
