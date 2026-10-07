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
    simulating, new spawns use the new config. Also re-applies the cull
    rule to the live pool at once (see Culling), which is permanent.
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
    `trigger_burst`, `set_emitter`, `set_config`, or a `set_bounds` that changes
    the size since) resets and
    replays from t=0. Both paths run the same whole steps, so the buffer
    is a pure function of `time`. No-op without a track.

    The snap tolerance grows with `time` (it tracks the f32 rounding of
    `time` itself), so a `time` just *after* a grid point can snap down
    to it and leave the particle state slightly *before* `time` -- by at
    most the tolerance, 80 µs at the 600 s track cap, about one f32 ULP
    of `time` there. Triggers are unaffected: one authored in that sliver
    snaps to the same grid point, so it has fired. In practice only NTSC
    rates, whose frames sit a few thousandths of a step off the grid, hit
    this: exact frame times (`n * 1001 / 60000` and so on) from 166.9 s at
    59.94, 258.6 s at 23.976, and 283.6 s at 29.97 fps, never more than
    70 µs early; hosts dividing by the decimals 59.94, 29.97, or 23.976
    from 83 s, 150 s, and 41.7 s. Integer rates (24, 25, 30, 50, 60 fps)
    land on the grid up to f32 rounding and never sit early of the frame
    they mean.
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
  new spawn. `set_config` reports a config that will overflow it (larger
  spawn rate x `lifetimeMax`, plus a burst) in the envelope's `warnings`
  array, without changing the config.
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

## Spin direction

Each particle draws a signed rotation speed `v` uniformly from
`rotationSpeedMin..rotationSpeedMax`. `spinDirection` (schema version 3;
omitted means `"fixed"`) decides its sign. `"fixed"` turns at `v` as
drawn, so the range's sign sets the direction: a positive range turns
every particle clockwise, and a range spanning zero gives both. `"random"`
turns at `v` or `-v` by a coin flip at spawn, so speeds are symmetric
about zero: a positive range tumbles pieces both ways at magnitudes from
it, and a range spanning zero gives speeds up to the larger of `|min|`
and `|max|` either way. The flip is a motion draw: toggling the
field re-lays out the whole effect, as changing any motion field does,
while `"fixed"` configs keep exactly the random sequence they had before
the field existed.

## Culling

`cullMargin` (schema version 4; omitted or `null` means off) removes a
particle once its center is more than that many logical px outside the
simulation's bounds, so an edge-spawned effect need not keep a lifetime long
enough to cross every aspect ratio. The bounds come from `set_viewport`
(`width / scale` by `height / scale`) or, for a host with no viewport such as
sprite mode, from `set_bounds` (`setBounds` in JS and WASM; Remotion's sprite
mode passes its composition size automatically). Without bounds nothing is
culled. Culling is permanent: a particle that leaves and would fall back in
is gone, so pick a margin that covers its excursion. A particle outside the
cull rectangle (the bounds plus the margin) is kept while it has not yet been
inside and is moving toward the rectangle, so an emitter outside the frame
works with any margin, including 0. One that is outside and moving away is
culled, entered or not, so particles that will never be seen do not fill the
pool. That also means a particle that gravity or turbulence would bring back
is culled early, if it had not entered yet. A particle that crosses the whole
rectangle in a single step lands on the far side moving outward, so it is
culled too. A particle whose position is not finite is always culled.

`set_bounds` takes logical px; a non-finite or non-positive size clears the
bounds. A C host without a viewport calls `bfx_set_bounds`. Changing the
bounds, or `set_config` with any config, also culls the live pool at once, by
the same rule, and permanently: growing the bounds or restoring a larger
`cullMargin` does not bring particles back. If a host calls both
`set_viewport` and `set_bounds`, the last call wins.

## Color modes

`ColorMode::MultiPalette` samples `color_stops` across each particle's
lifetime: `offset` is lifetime progress in [0, 1], stops are sorted by
offset on config load, neighbors interpolate linearly, and progress before
the first stop or after the last holds that stop's color. Without stops
(`null` or empty) the mode falls back to the primary color, like `Single`.

`ColorMode::RandomPalette` (`"random-palette"`) deals each particle one
`color_stops` entry, and the particle keeps it for life. Every entry is
equally likely -- repeat one to weight it. Offsets do not weight the pick;
they set the order the stops are dealt from (stops are sorted by offset,
as for `MultiPalette`), and `clamp_to_bounds` still reports an offset
outside [0, 1]. Because the pick is per particle rather than per spawn
order, the colours are mixed wherever the particles land -- unlike
`RainbowCycle`, whose hue follows spawn order and so streaks along a
moving emitter's path. Without stops it falls back to the primary color.

Each particle's pick is a unit float from a colour RNG of its own, drawn
at every spawn in every mode and resolved against the current stops each
frame. So a colour-only config change never moves a particle -- motion
draws from a separate RNG -- and a live stop edit, or a live switch into
this mode, recolours the particles already on screen, keeping each one's
place in the stop order. Both RNGs follow the seed, so scrubbing is
deterministic: a seek replays the same picks.

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
