# Emitter track pre-roll

Closes #19. A baked effect starts at composition time 0 with an empty pool,
so a rain shown in a composition's first seconds has an empty lower frame
while it fills (about 4 s for frosting-rain at 1080×1920). This design lets
an effect file say that its timeline began before 0, so `seek(0)` returns
the pool as it stands after that much playback.

## Decisions

- **Pre-roll is a property of the effect file**, not of the composition.
  A preset that needs warm-up says so, and every host (Remotion, Swift,
  C#, web) that seeks to 0 gets the steady state without knowing why.
  This keeps the file self-contained, the README's stated goal.
- **The timeline extends into negative time.** Keyframes and triggers may
  carry negative times; `emitterTrack.preroll` declares how far back the
  timeline runs. Authored times stay literal (authored t equals
  composition t), so `fit_track`, the cue generator, the Remotion
  `window`, and every future track editor keep working unchanged.
- Rejected: an explicit `preroll` with the emitter parked at its first
  keyframe (a sweeping rain would start with every drop born under one
  point, not steady state), and a `preroll` that shifts the whole
  timeline (authored times would stop meaning what they say).
- Rejected: a host-side `preroll` prop (every host and composition would
  have to know to ask), and a file default with host override (more
  surface than the need justifies).

## Core semantics

The track's timeline runs from `-preroll` to `duration`.

- **The 1/60 s grid stays anchored at authored t=0.** Every existing
  argument about grid snapping, off-grid frame rates, and never-late
  triggers holds unchanged. The pre-roll adds `S = grid_step(preroll)`
  whole steps in front of authored step 0.
- A step index is **absolute**: 0 is the start of pre-roll, `S` is
  authored t=0. The grid time of absolute step `k` is computed from the
  index as `(k - S) × PLAYBACK_STEP`, never accumulated, as today.
- `seek(time)` clamps `time` to `[0, duration]` as today and runs through
  absolute step `S + grid_step(time)`. Pre-roll is simulated, never
  rendered: `seek(0)` returns the pool after `preroll` seconds of
  playback. Scrubbing into negative time is out of scope.
- `restart_track` resets, places the emitter at `sample_track(-preroll)`,
  and fires the triggers whose fire step is 0.
- For a trigger after `-preroll`, the fire step is `S` plus the signed
  grid step of its time, floored at 0. The signed grid step is today's `grid_step` rule applied
  to a signed f64 (snap to a grid point within tolerance, otherwise the
  grid point at or after), so a trigger at -0.5 s maps to `S - 30` and a
  trigger at -0.49 s to `S - 29`. A trigger at or before `-preroll` fires
  at the start (step 0), exactly rather than through the grid arithmetic,
  so a generator's `StartContinuous` at `-preroll` fires at the reset even
  when `-preroll` is off the grid; this mirrors today's `time.max(0.0)`
  for triggers before 0.
- `sample_track` already clamps to the first keyframe before it, so
  keyframes earlier than `-preroll` are harmless and a track whose first
  keyframe is at 0 keeps the emitter parked through pre-roll.
- The forward-seek cursor (`baked`) stores the absolute index. The
  forward-equals-fresh property, `leave_baked`, `set_bounds`, and
  `set_config` behavior are unchanged.
- Idle emission during pre-roll is ordinary stepping.
- Loop bound: at most `grid_step(MAX_PREROLL) + grid_step(600)` steps.
- `preroll` 0 reproduces today's behavior bit for bit; the existing
  fixtures prove it.

## File format (schema version 5)

- `emitterTrack.preroll`: seconds, f32, default 0, **omitted from JSON
  when 0** (`serde(default, skip_serializing_if)`), so a minimum-version
  check (#33) can tell a v5 file from a v4 one.
- Clamped by `clamp_to_bounds` to `[0, MAX_PREROLL]`, `MAX_PREROLL = 60`
  s, reported in the envelope as `emitterTrack.preroll`. A non-finite
  value becomes 0 (and is reported). `build_baked_track` applies the same
  floor and ceiling itself, as it does for `duration`, so a config built
  from unvalidated JSON cannot drive an unbounded loop.
- `EmitterKeyframe.time` and `EmitterTrigger.time` may be negative.
- `SCHEMA_VERSION` becomes 5; `MIN_SCHEMA_VERSION` stays 1; the 4→5
  migration is a no-op. A v4 runtime rejects a v5 file with its existing
  "unsupported schemaVersion" message, which is the point of the bump: a
  v4 runtime would mishandle negative trigger times.
- `brightfx-js`: `preroll?: number` on the track type, documented as
  "schema version 5+".
- Remotion: no change. `window` and the track-end check are composition
  time; pre-roll is invisible to them.

## Generators and presets

- `sweep_track(a, b, period, duration, preroll)` and
  `scatter_sweep(y_min, y_max, leg, preroll)` extend their keyframes to
  `-P × leg` where `P = leg_count(preroll, leg) + 1` whole legs, so the
  path covers the whole pre-roll. Position stays a function of the
  keyframe index counted from the track start. The sweep's parity
  alternation is computed with `rem_euclid`, so its keyframes at t ≥ 0
  are unchanged; the scatter's hash shifts, which only moves a
  pseudo-random path. The single `StartContinuous` moves from 0 to
  `-preroll`, and the track carries `preroll`. `preroll` 0 reproduces
  today's tracks exactly.
- The cue generator's `static_track` is untouched: cue tracks have
  nothing running at t=0.
- `fit_track` needs no change (it scales x and y only); a test pins that
  `preroll` and negative times pass through.
- **Every preset sets `preroll = lifetimeMax / 60` seconds.** Every
  particle alive at t=0 was born within the last `lifetimeMax` frames, so
  one full lifetime of warm-up is a steady-state pool for any spawn rate,
  frame size, or cull margin. That is 2 s for confetti, fireflies, and
  bubbles, 1.5 s for sparkles, 4.67 s for frosting-rain, and 5 s for
  sprinkle-rain and flight-arc. One rule, no per-preset tuning; at most
  300 extra steps on the first seek. A `preroll_for(&config)` helper in
  `presets.rs` derives it so the rule lives in one place.
- The preset invariants in `core/brightfx-tracks/tests/presets.rs`
  change from "one `StartContinuous` at 0" to "one `StartContinuous` at
  `-preroll`, and `preroll` equals `lifetimeMax / 60`". "Last keyframe
  lands on the duration" is unchanged.

## Testing

- **Core, the defining property:** a track with `preroll` P seeked to t
  produces the same buffer as the same track with every
  keyframe and trigger time shifted by +P and `preroll` 0, seeked to
  t + P: bit for bit with a parked emitter; a moving emitter to the
  fixture tolerance, because the shifted track samples its path at times
  that round differently. Pre-roll is exactly "the timeline started earlier".
- **Core, regressions:** forward seek equals fresh seek with pre-roll on;
  a trigger at a negative time fires at its step; one before `-preroll`
  fires at the start; `preroll` clamps to `[0, 60]` and non-finite becomes
  0, both reported; a v4 file parses with `preroll` 0; the existing
  fixtures stay bit-identical (the smoke script's "fixture not rewritten"
  guard enforces it).
- **Cross-language:** one new fixture, `ffi-preroll`, schema 5, with a
  1 s pre-roll, a keyframe and a trigger at negative times. Node, Swift,
  and C# each add a block in the shape of the existing seek block: seek
  to 0, compare the buffer, then a forward seek compared against a
  second expected file. No new FFI functions.
- **Presets, the acceptance test for #19:** for every preset, the pool
  count at `seek(0)` lies inside that census's min and max widened by a tenth at each end
  (the count fluctuates with the emitter's path and 180 steps is a short
  look), taken from the existing 180-step steady-state census at 9 s. All seven golden frames at t=3
  regenerate; the diff of the two rains is the visual proof.
- **Tracks:** `sweep_track` and `scatter_sweep` with a pre-roll cover the
  negative range, keep a keyframe at t=0, and place the trigger at
  `-preroll`; with `preroll` 0 they are unchanged. `fit_track` preserves
  `preroll` and negative times.
- **Docs:** the seek contract in `core/README.md`; the schema version
  list in `core/brightfx-core/README.md`; the JS type comment; one
  sentence in the Remotion README that `window` is composition time and
  pre-roll is invisible to it.

## Deviations recorded during implementation

- The cross-language blocks run a single forward check against a fresh
  seek instead of a second recorded expected file.
- The bit-identity property is qualified: bit for bit with a parked
  emitter, fixture tolerance with a moving one.
- The preset census band is widened by a tenth at each end.
- A trigger at exactly `-preroll` is pinned to step 0.

## Out of scope

Scrubbing into negative time, a host-side override, pre-roll on
cue-generated tracks, and a minimum-schema-version writer (#33).
