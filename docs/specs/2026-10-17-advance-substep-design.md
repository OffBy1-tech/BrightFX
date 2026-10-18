# Sub-stepping `advance`, and idle emission per frame-equivalent

Issues: #49 (`advance` integrates a long `dt` as one step), #48 (idle emission
is a per-step probability). Also ticks the #37 item "`rebuild_buffer` runs on
every replay step".

## Problem

`Simulation::advance(dt)` runs one `step` with `fe = min(dt, 0.1) * 60`. Every
per-step term needs its own `fe` correction, and several have none:

- A long step spawns all its continuous particles before the integrate loop,
  so each is aged by the full `fe` and moved by `vx * fe`. They land in one
  clump instead of spreading over the step's ages and radii.
- Turbulence is sampled once per step; gravity and vortex are explicit Euler
  at `fe`.
- Idle emission is one draw per step with probability `spawnRateIdle * 0.3`,
  so its rate per second scales with the host's refresh rate: about 18/s at
  60 Hz, 43/s at 144 Hz for a rate of 1.

## Design

### 1. `advance` splits long steps

`advance(dt)` clamps `dt` to `0 ..= MAX_DELTA_SECONDS` as `step` does now,
converts it to frame-equivalents `fe = seconds * REFERENCE_HZ`, and runs
`n = ceil(fe / MAX_STEP_FE)` equal steps of `fe / n` (`n` at least 1), with
`MAX_STEP_FE = 1.1`. Slicing is done in frame-equivalents so that a `dt` of
exactly 1/60 s gives `fe == 1.0`, one step, identical to today.

- `fe <= 1.1` (60 Hz and faster, including 60 Hz frames that jitter up to
  about 18.3 ms, every fixture at 1/64 s, the goldens at 1/60 s): one step.
  For `fe <= 1` that is exactly today's step.
- 20 Hz (`fe` 3): three steps of 1. At the 0.1 s ceiling (`fe` 6): six.
- `dt == 0` is one step of `fe` 0. Nothing moves, as before. One thing does change: idle emission, which used to draw `rate * 0.3` per call even at `dt` 0, now spawns nothing (see 3). A paused host calling `advance(0)` stops emitting idle particles.

*Revised after the Task 1 review:* the first version split at `ceil(fe)`.
About half of real 60 Hz frames land just past 16.667 ms, and those ran as
two half-steps, doubling the work on ordinary 60 Hz hosts. The 10% tolerance
keeps them one step. The cost is that a step can be up to 1.1 frames long.

`step` takes `fe` directly rather than `dt`. `seek` passes
`PLAYBACK_STEP * REFERENCE_HZ` (1.0), so replay is unchanged.

The emitter is held at the position the host last set for every sub-step.
Interpolating between frames was considered and declined: a jump (pointer
leaving and re-entering, a host repositioning) would draw a streak, and it
adds state the saved pre-roll copy must carry. A fast trail at 20 Hz still
shows gaps between frames, but the ages within each frame spread out.

### 2. The buffer is rebuilt once per call

`step` stops calling `rebuild_buffer`. `advance` rebuilds once after its
sub-steps; `seek` rebuilds once after its replay loop (including a forward
seek that runs no steps, whose buffer is already current, and the restored
pre-roll path in `restart_track`, whose comment changes accordingly).
`spawn_burst` keeps its own rebuild. Output is unchanged; only the work
drops.

### 3. Idle emission per frame-equivalent

The idle draw becomes `rng.f32() < min(spawnRateIdle * 0.3, 1) * fe`. After
(1) every step has `fe <= 1.1`, so the expected idle rate is
`min(0.3 * rate, 1)` per frame-equivalent at any refresh rate. The one
exception is a step between 1 and 1.1 frames, where the threshold can pass 1
and saturated idle still gives one spawn per step, up to 10% short. The
draw is still made once per step, so it can't yield two.

At `fe == 1` it is bit-for-bit today's draw: below saturation the threshold
is the same number, and at saturation (`0.3 * rate > 1`) today's
`rng < 1.5` and the new `rng < 1.0` both always pass, since `rng.f32()` is in
`[0, 1)`. The draw itself still happens once per step, so the RNG sequence
is unchanged at `fe == 1`.

`pool_warnings` already models idle as `min(0.3 * rate, 1)` per frame; its
comment changes from "per step" to "per frame-equivalent".

## What changes for hosts

- Below 60 Hz: finer physics and spawn ages spread across the frame.
  `advance` output differs from today; that is the fix.
- Above 60 Hz: idle emission falls to the 60 Hz rate per second (from up to
  about 2.4x at 144 Hz).
- `advance(0)` (a paused host): no more idle spawns.
- 60 Hz, `seek`, fixtures, goldens: unchanged. Every fixture has `spawnRateIdle` 0, so the idle change below `fe` 1 can't show there. The fixture-unchanged check
  in `core/scripts/smoke-all.sh` guards this.

Cost: at most 6 integrate passes per `advance`, only on frames longer than
1/60 s, with one buffer rebuild.

## Tests

- One `advance(3/60)` equals three `advance(1/60)` calls from the same seed:
  same particle count and same buffer.
- At `advance(0.1)` with continuous emission, the new particles' `life`
  values span six distinct ages (1 through 6), not all 6.
- Idle at rate 1, `fe` 0.4 (144 Hz-like `dt`): over a fixed seed and enough
  steps for the same total time as N steps at `fe` 1, the counts agree
  within a tolerance that the old code (2.5x) fails.
- Saturated idle (rate 5) spawns one particle every 1/60 s step, as before.
- Existing fixture, golden, and seek tests pass unchanged.

## Out of scope

- Emitter interpolation between `advance` calls (declined above).
- `spawn_particle` cloning `EmitterConfig` and re-parsing `primaryColor` per
  spawn (noted in #49).
