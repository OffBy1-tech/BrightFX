# BrightFX Core

The portable particle-simulation engine, its rasterizer, and its FFI
boundary. Hosts blit the frame this produces.

## Crates

| Crate | Purpose |
|---|---|
| `brightfx-core` | Simulation, the `render` rasterizer, schema, and the `abi` module holding all boundary logic |
| `brightfx-tracks` | Emitter-track generators (`fit_track`, `sweep_track`, the lyric-cue generator): timing data in, baked tracks out, no rendering |
| `brightfx-ffi` | C ABI wrapper (staticlib + cdylib) for the macOS and Windows apps |
| `brightfx-wasm` | wasm-bindgen wrapper for the web app; also exports the `brightfx-tracks` generators |
| `brightfx-test-support` | Dev-only: the fixture helpers (`fixtures_dir`, `regenerating`) every crate's tests share; not published |

See `brightfx-core/README.md` for the simulation and render crate itself —
its schema, its units, and its known gaps (`sound_on_spawn` is the one
config field left as passthrough data the core never reads).

`brightfx-ffi` and `brightfx-wasm` contain **no logic**. Every exported
function is a type translation around one call into `brightfx_core::abi` or,
for `fitTrack` and `generateCueTracks`, `brightfx_tracks::json`. Behavior
changes belong in those crates, never in a wrapper — that is what keeps the
platforms from drifting apart.

## Using it

```
sim = new(seed)                       // starts from the default config
set_config(json) -> envelope          // {"ok":true,"clamped":[...]} | {"ok":false,"error":"..."}

// per frame
set_emitter(x, y, vx, vy, active)
advance(dt)
read particle_count() * particle_floats() floats at buffer_ptr()

// scrubbing (requires an emitterTrack in the config)
seek(time)                           // baked mode; forward seeks step from the last seek

// drawing
set_viewport(width, height, scale) -> envelope    // device pixels, and pixels per logical unit
render()
read frame_len() bytes at frame_ptr()             // premultiplied RGBA8, stride width * 4
```

Notes that matter:

- `set_config` succeeds atomically and keeps live particles; the new config
  applies from the next frame. It also clears the baked position, so the
  next `seek` replays from t=0. Re-seed with `seek(0)`.
- **A rejected config changes nothing.** The previous config stays loaded.
- **The buffer pointer is stable** for the life of a simulation, but its
  *contents* are only valid until the next `advance`/`seek`/`trigger_burst`, or a `set_bounds`/`set_viewport`
  that changes the size (it culls live particles at once).
- **On WASM, `set_config` can detach your `Float32Array`.** Read through the
  `readBuffer` helper in `harnesses/node/brightfx.mjs`, which rebuilds the
  view when linear memory has moved.
- **A handle is single-threaded.** Drive it from one thread.
- **A panicked simulation goes inert and reports zero particles.** `guard`
  stops the unwind but cannot roll back a half-finished mutation, so rather
  than let a host keep reading torn state, the simulation poisons itself:
  every further operation is a no-op, `set_config` returns
  `{"ok":false,"error":"simulation is poisoned by a panic; create a new one"}`,
  and `particle_count()` reports 0 so a host's raw read of `buffer_ptr()` stays
  in bounds. Recover by dropping the handle and building a new one.
- **Poisoning does not work on WASM, and cannot.** `wasm32-unknown-unknown`
  is `panic = "abort"` — the stable toolchain ships no linkable unwinding
  runtime for it — so `catch_unwind` never catches and the whole containment
  mechanism above is inert on the web target. A panic there traps the module
  instance: JS sees a `RuntimeError`, and every later call on that instance
  fails. Web hosts must treat a thrown call as fatal to the instance and
  construct a new `BrightFx` rather than waiting for `isPoisoned()` to go
  true, because it never will. The C ABI targets (macOS, Windows) do get the
  full guarantee.
- **The frame is premultiplied RGBA8**, row-major, `frame_width() * 4`
  bytes per row, cleared to transparent on every `render`. Composite it over
  your background with source-over; blend modes only apply between
  particles.
- **`set_viewport` reallocates the frame.** Its address is stable until the
  next `set_viewport`; on WASM, re-read `frame_ptr` afterwards for the same
  reason as `set_config`. Read through `readFrame` in
  `harnesses/node/brightfx.mjs`.

## Verifying

```bash
./scripts/smoke-all.sh
```

Runs the Rust tests, checks the generated C header is current, then replays
identical protocols through the Node, Swift, and C# harnesses, each compared
against a fixture in `fixtures/` that a Rust test generated. `tests/golden.rs`
guards simulation drift; the fixtures guard boundary drift.

- `fixtures/ffi-smoke.*`: 120 frames of `set_emitter` + `advance` with one
  burst, compared as a particle buffer. The fixture records the emitter's
  state on every frame (`emitterFrames`) so the harnesses replay numbers
  rather than each re-typing a formula.
- `fixtures/ffi-seek.*`: the same config with an `emitterTrack`, driven
  through `seek` alone. This is the only place `seek` crosses the boundary,
  and the only cross-target coverage of baked timeline playback. It seeks
  twice, backwards the second time, so the recorded state cannot depend on
  call history. Regenerated in the forward-seek change: the old replay
  loop accumulated its step time, so its running time undershot each grid
  point and a trigger authored at exactly `j/60` fell one step late.
  Triggers now fire in the step that ends at their authored time, so
  every buffer after this fixture's burst at t = 1.0 changed.
- `fixtures/ffi-seek-forward.*`: the same config seeked at strictly
  increasing times on one handle. Each harness compares to the recorded
  buffer and then requires a fresh `seek` to the last time to be
  bit-identical — the property that lets a forward seek step instead of
  replay.
- `fixtures/tracks-cues.*`: the lyric-cue generator's input and recorded
  jobs. Rust records; the Node harness runs the same input through the
  wasm build and requires the identical bytes, which is what keeps one
  implementation of every track generator. It embeds the core's
  `EmitterTrack` serialization, so an intentional change there means
  `BRIGHTFX_REGENERATE=1 cargo test -p brightfx-tracks --test cues_fixture`
  (the recording test lives in that crate, not in `brightfx-core`).
- `fixtures/ffi-frame.*`: the golden frame, compared with a per-channel
  tolerance and a cap on differing pixels (both recorded in the JSON).
  Regenerate with
  `BRIGHTFX_REGENERATE=1 cargo test -p brightfx-core --test frame_fixture`
  and look at the diff.

Every buffer comparison uses one tolerance, `LIBM_DRIFT_TOLERANCE` in
`brightfx-core/tests/common/mod.rs`, recorded into each fixture's JSON. It
covers libm differences between the builds being compared (this OS against
the recording OS for `golden.rs`, native against wasm for the fixtures); it
is not a budget for simulation changes.

CI runs the same script on every push to `main` and every pull request
(`.github/workflows/smoke.yml`, a macOS runner, since the Swift harness needs
`swiftc` and the header guard needs the pinned `cbindgen`). A second job on
a Windows runner builds `brightfx-ffi` for MSVC and runs the Rust tests and
the C# harness against the resulting `brightfx_ffi.dll`; the harness source
is the same on both, and its run script picks the library file by OS. That
job is x64 only: the `Cdecl` calling convention the harness declares is a
no-op on x64 and arm64 and would only be exercised by an x86 build, which
nothing targets. Neither job sets `BRIGHTFX_REGENERATE`, and both refuse to
start if that variable has leaked into the environment; regeneration stays
a manual, reviewed step. The `cbindgen` pin below is repeated in the
workflow and the two must move together.

After an intentional simulation change, regenerate the particle fixtures
(and the cue fixture too if the change touched track serialization):

```bash
BRIGHTFX_REGENERATE=1 cargo test -p brightfx-core --test ffi_fixture --test seek_fixture --test seek_forward_fixture
BRIGHTFX_REGENERATE=1 cargo test -p brightfx-tracks --test cues_fixture
```

Only the exact value `1` regenerates; any other value, or none, verifies.
Every suite asks `brightfx-test-support`'s `regenerating()`, so that rule
holds workspace-wide; a new fixture test should use it rather than read
the variable itself.

After changing any `extern "C"` signature, regenerate the header:

```bash
./scripts/gen-header.sh
```

## Prerequisites

```bash
rustup target add wasm32-unknown-unknown
cargo install wasm-pack && cargo install cbindgen --version 0.29.4
```

`cbindgen` is pinned: an unpinned install can pick up a newer version whose
cosmetic formatting differs from what's committed, which trips the header
staleness guard in `smoke-all.sh` for a change nobody actually made.
