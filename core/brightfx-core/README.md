# brightfx-core

The shared particle simulation and rendering engine for BrightFX. Pure Rust,
no platform code, no UI — every authoring tool (web, macOS, Windows) links
against this crate so particle behavior and drawing are each defined
exactly once.

## Public API

- `schema` module — `ParticleFxConfig` and its nested types
  (`EmitterConfig`, `EmitterTrack`, `ColorStop`, and the vocabulary enums
  `ParticleShape`/`BlendMode`/`EmissionPattern`/`ColorMode`/`SizeCurve`/
  `Category`). All `serde`-serializable as camelCase JSON (enums as
  kebab-case strings), and derive `schemars::JsonSchema`.
- `Simulation` — the runtime engine.
  - `Simulation::new(config, seed) -> Simulation`
  - `set_config(config)` — hot-swaps parameters; live particles keep
    simulating, new spawns use the new config.
  - `set_emitter(x, y, vx, vy, active)` — **live mode**: call once per host
    tick before `advance`, with `active` reflecting whether continuous
    emission should be running (e.g. mouse currently moving, or a game
    object currently firing).
  - `trigger_burst()` — fires `emitter.spawn_burst_size` particles from the
    current emitter position immediately.
  - `advance(dt)` — steps the simulation by `dt` seconds (live mode).
  - `seek(time)` — **baked mode**: if the config has an `emitter_track`,
    runs playback *through* the point on the 1/60 s grid at or after
    `time` (a `time` within tolerance of a grid point snaps to it). So
    when `time` is rendered, every trigger authored at or before it has
    fired — never one frame late, whatever rate the host renders at —
    and the simulation may sit up to one step (1/60 s) past `time`. A
    seek at or after the previous seek's step applies only the steps in
    between; any other call sequence (a backward seek, or any `advance`,
    `trigger_burst`, `set_emitter`, or `set_config` since) resets and
    replays from t=0. Both paths run the same whole steps, so the buffer
    is a pure function of `time`. No-op without a track.
  - `buffer() -> &[ParticleInstance]` — current particle instances to draw
    (position, size, rotation, normalized RGBA color).
  - `particle_count() -> usize`.

Units: every physics/lifetime field in `ParticleFxConfig` is a 60fps
frame-equivalent quantity (see the crate's `simulation.rs` module docs) —
`advance` takes real seconds and converts internally.

- `abi` module — the FFI boundary logic: `AbiSimulation`, a thin layer over
  `Simulation` that adds JSON config ingest with schema-version checking and
  bounds clamping, result envelopes (`{"ok":true,...}` / `{"ok":false,...}`),
  and panic containment (`catch_unwind`, poisoning on a caught panic). This is
  the crate's primary consumer-facing surface: `brightfx-ffi` (the C ABI) and
  `brightfx-wasm` (the wasm-bindgen bindings) are thin type-translation
  wrappers that delegate every operation to `abi` rather than reimplementing
  any of it, so the two platforms cannot drift apart.
- `render` module (feature `render`, default on) — the rasterizer:
  `Renderer` owns a premultiplied RGBA8 frame; `shapes` holds the 13 display
  lists; `raster`, `path`, `paint`, `blend`, and `glow` are the pieces.
  Build with `--no-default-features` for a simulation-only crate.

## Known gaps

- The particle pool has a fixed capacity of 500 (`MAX_PARTICLES`) with FIFO
  eviction — when full, the oldest particle is dropped to make room for a
  new spawn.
- `ParticleFxConfig.sound_on_spawn` is passthrough data the core never reads;
  sound is host behavior.
- `seed` (passed to `Simulation::new`) only takes effect through `reset()`
  (called internally by `seek()`) — in live mode (`advance()`), it seeds
  the initial RNG stream once at construction and is never re-applied;
  there is currently no public way to reseed a live-mode simulation
  without constructing a new `Simulation`.

## Performance

`cargo run --release --example render_bench -p brightfx-core` measures
frame time for 500 glowing particles at 1280×800, per shape, with glow off,
on, and with sizes changing every frame (a glow-sprite cache miss per
frame). On Apple M3, release, ms per frame:

| shape | no glow | glow | glow, resizing |
|---|---|---|---|
| sparkle-star (the reference scene) | 0.6 | 3.6 | 5.2 |
| circle | 0.7 | 2.5 | 3.6 |
| glow-disc | 1.8 | 3.8 | 5.6 |
| ring | 2.1 | 3.9 | 5.7 |
| lightning-bolt | 0.9 | 4.1 | 6.0 |
| rune | 1.5 | 5.1 | 7.1 |

The spec's budget is under 4 ms for the reference scene; it started at
8.7 ms. Glow stamping is still the hot path. What keeps it at this cost:
each frame row is clipped to an octagon around the halo; a pixel whose
premultiplied alpha is under `0.49/255` is never stamped, since no blend
mode can change a byte with it (`glow::MIN_VISIBLE_ALPHA`); rotationally
symmetric halos (circle, glow-disc, ring, smoke-puff) are stamped
axis-aligned; each stamp loop is compiled per blend mode; and a stroke
join is a bevel wherever a round join could not differ from it by a
twentieth of a pixel, which is every join of a flattened curve.

Lightning-bolt and rune remain above the budget. Both are asymmetric, so
their halos take the rotated stamp path, whose remaining cost is the
bilinear sample and the byte round trip per pixel. The lever left for
them is an f32 working frame converted once per render, which trades
memory proportional to the viewport for the round trip; nearest sampling
was rejected because at small blur its error reaches tens of byte levels.
Two cheap ideas were measured and gave nothing: a lookup table for the
byte-to-unit division, and a row-major vertical blur pass.

Memory is bounded as well as time: a glow sprite's size is predicted
before it is built and the cache makes room first, so the 64 MiB budget
bounds allocation rather than retention; a sprite's half-side is capped at
2047 device pixels so one always fits; and a frame the allocator refuses
comes back as an error envelope rather than an abort.

`cargo run --release --example frame_dump -p brightfx-core -- <dir>` writes
105 reference frames (every shape, every blend mode, two scales, and the
benchmark scene) as raw RGBA. Run it before and after a rendering change
and diff the directories: a speedup that should not change pixels can be
held to bit-identical output, and one that legitimately does can be
measured rather than eyeballed.

## Color modes

`ColorMode::MultiPalette` samples `color_stops` across each particle's
lifetime: `offset` is lifetime progress in [0, 1], stops are sorted by
offset on config load, neighbors interpolate linearly, and progress before
the first stop or after the last holds that stop's color. Without stops
(`null` or empty) the mode falls back to the primary color, like `Single`.
The other modes ignore `color_stops`.

## Loading untrusted / hand-edited configs

`ParticleFxConfig` deserializes permissively (serde will happily accept an
out-of-range `gravityX: 999.0`). After parsing a config from a file a host
didn't just generate itself, call `config.clamp_to_bounds()` — it clamps
every field with a documented valid range in place and returns the field
names it changed, so the host UI can show something like "3 values were out
of range and have been clamped." Fields with no documented range (shape,
colors, `sound_on_spawn`) are left as-is.

Two details beyond per-field ranges:

- **Ordered pairs are repaired.** After range clamping, an inverted
  `lifetimeMin`/`lifetimeMax`, `initialSpeedMin`/`Max`, or
  `rotationSpeedMin`/`Max` has its max raised to min, and only the max
  field is reported.
- **`color_stops` offsets are clamped into [0, 1]** and reported once as
  `colorStops`, not per index.

## Testing

- `cargo test -p brightfx-core` runs unit tests (in each module) plus the
  golden regression suite (`tests/golden.rs`) and the FFI fixture test
  (`tests/ffi_fixture.rs`).
- If you intentionally change simulation behavior, regenerate the golden
  fixture: `BRIGHTFX_UPDATE_GOLDEN=1 cargo test -p brightfx-core --test golden`,
  then review the diff in `tests/fixtures/golden_fire.json` before
  committing — a diff here means every consuming platform's rendered output
  will change too.
- `tests/ffi_fixture.rs` drives an `AbiSimulation` through the same protocol
  the Node, Swift, and C# harnesses in `../harnesses/` replay, and records the
  result in `../fixtures/ffi-smoke.expected.json`. It guards boundary drift
  the way `golden.rs` guards simulation drift. Regenerate it the same way:
  `BRIGHTFX_REGENERATE=1 cargo test -p brightfx-core --test ffi_fixture`.
