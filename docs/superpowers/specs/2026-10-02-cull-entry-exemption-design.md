# Cull entry exemption (follow-up to #17)

## Problem

`cullMargin` (PR #22) removes a particle once its center is more than the
margin outside the simulation's bounds. A particle spawns (emitter +/-2 px)
inside `step`, before the `retain_mut` that culls. So a particle that starts
more than the margin outside the bounds is removed on its first step, before
it is ever drawn.

Every edge-spawned preset puts its emitter off-screen (rain above the frame,
bubbles below it, confetti, fireflies and sparkles to the left, flight-arc
left of the frame). With a margin smaller than the emitter's distance outside
the frame, the effect shows nothing, silently. The margin has to be tuned
against emitter placement, which is the per-aspect tuning the field was meant
to remove.

## Design

A particle outside the cull rectangle is kept only while it can still enter:
it has not been inside before, and it is not moving away from the rectangle.

- `Particle` (crate-private, `particle.rs`) gets `entered: bool`, false at
  spawn.
- In `step`, after the position update, when both `cullMargin` and bounds are
  set, with `rect = [-m, w + m] x [-m, h + m]`:
  - inside `rect`: `entered = true`, kept;
  - outside `rect` and (`entered` or moving away from `rect`): culled;
  - outside `rect`, not entered, not moving away: kept (an off-screen emitter
    whose particle is on its way in, or a particle at rest).
- "Moving away" means the particle's current velocity has a positive
  component along its offset from the nearest point of `rect`
  (`overshoot_x * vx + overshoot_y * vy > 0`, where each overshoot is the
  signed distance past the nearest edge on that axis and 0 when inside it).
  A particle at rest is not moving away. The test is per axis, so a particle
  outside on one axis only is judged on that axis.
- A particle whose position is not finite (NaN or infinite) is culled
  unconditionally, entered or not. It cannot be on screen, and it is never
  "inside" or "moving away", so without this rule it would be exempt.
- With no bounds or no `cullMargin`, nothing changes: the flag is not
  touched and nothing is culled.

### Why moving away is culled

The first version exempted a particle until it had entered, however it moved.
Review measured the cost: an emitter 30 px outside the frame, margin 0,
particles moving away, 5 spawned per step, left 495 of 500 pool slots holding
invisible particles after 200 steps. Sprite hosts still render an element for
each, and the pool's FIFO eviction drops visible particles to make room for
them. The rule was also discontinuous: with the emitter inside the margin,
away-moving particles were culled at the margin edge, and just outside it
they lived forever. Culling a not-yet-entered particle that is moving away
removes both problems.

### Consequences of the moving-away test

- A particle that skips the whole rectangle in one step (live `advance(dt)`
  can move a particle 100 px or more in a hitch frame, and a small sprite
  container is easily skipped) lands on the far side moving outward, so it is
  culled. No limit applies.
- Gravity, turbulence or vortex can bring back a particle that is moving away
  now. One that has not yet entered is culled early; one that has entered is
  culled the moment it leaves, as before. Culling after entry stays
  permanent.
- Turning culling on mid-run (bounds going from none to set, or `set_config`
  adding `cullMargin`) culls live particles that are already outside and moving
  away (at once for `set_bounds` and `set_config` since #26/#29; the first
  version waited for the next step). Live particles outside and moving toward the
  rectangle are kept.

### State and determinism

- The flag lives only in `Particle`. `ParticleInstance` (the 32-byte ABI
  struct) is unchanged, and there is no config, schema or preset change, so
  no schema version bump and no fixture regeneration.
- The cull is a pure function of the particle's position, velocity and flag.
  The flag is part of the replayed state, so a baked `seek` is still a pure
  function of time. `set_bounds` still calls `leave_baked`.
- Live particles keep their flag across a bounds change. A viewport shrunk
  mid-run can therefore cull, on the next step, a particle that had entered
  the larger rectangle.

## Documentation changes

- `docs/superpowers/specs/2026-10-02-cull-particles-outside-viewport-design.md`:
  the Simulation bullet and the Trade-off paragraph.
- `core/brightfx-core/README.md`, "Culling".
- The `cull_margin` field doc and the `MAX_CULL_MARGIN` comment in
  `schema.rs`, and the `cullMargin` doc in `packages/brightfx-js/src/index.ts`.

## Out of scope

Retuning presets to use `cullMargin` (separate PR, now unblocked); the other
#17 follow-ups (re-culling the live pool inside `set_bounds`, a result for
invalid `setBounds` sizes, `bfx_set_bounds`, the preset schema version).

## Testing

All in `simulation.rs` unit tests, with a control run (culling off) in each
test that proves the particle really spawned:

- A particle at rest outside the margin is kept.
- A particle moving toward the frame from outside is kept, is `entered` once
  inside, and is culled after it leaves.
- A particle moving away from an off-screen emitter is culled.
- A particle in the margin zone is kept, is `entered`, and is culled once it
  leaves the margin.
- A particle that leaves the frame in its first step is culled, at the
  schema's speed ceiling of 15 px per step.
- A particle that skips a 4x4 frame in one step is culled.
- Turning culling on mid-run via `set_config` culls a particle already
  outside and moving away.
- `culling_removes_some_but_not_all_of_a_burst` and the off-screen-emitter
  seek test assert the rule as an invariant over the surviving pool: a
  particle outside the rectangle has not entered and is not moving away.
- Seek determinism with an off-screen emitter: fresh, rewound and
  forward-stepped seeks give identical buffers, with guards that some
  particles entered and some were culled.
- The existing NaN test still passes unchanged.
