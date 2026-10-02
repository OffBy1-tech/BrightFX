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

A particle is exempt from culling until it has been inside the margin
rectangle once.

- `Particle` (crate-private, `particle.rs`) gets `entered: bool`, false at
  spawn.
- In `step`, when both `cullMargin` and bounds are set, a particle that is
  inside `[-m, w + m] x [-m, h + m]` sets `entered = true`. This is checked
  twice per step: before the position update (so the spawn position counts,
  and a particle that spawns inside and leaves in its first step is still
  culled) and after it. After the update, a particle outside the rectangle is
  culled if `entered` is already true, and kept if not.
- A particle whose position is not finite (NaN or infinite) is culled
  unconditionally, entered or not. It cannot be on screen, and without this
  rule the entry exemption would keep a NaN particle for its whole lifetime,
  undoing the NaN cull. The inside test itself keeps the NaN-safe form
  (`inside = x >= -m && x <= w + m && y >= -m && y <= h + m`), so the
  non-finite check is explicit rather than an accident of comparisons.
- With no bounds or no `cullMargin`, nothing changes: the flag is not
  touched and nothing is culled.

### State and determinism

- The flag lives only in `Particle`. `ParticleInstance` (the 32-byte ABI
  struct) is unchanged, and there is no config, schema or preset change, so
  no schema version bump and no fixture regeneration.
- The cull stays a pure function of the particle's position and its flag.
  The flag is part of the replayed state, so a baked `seek` is still a pure
  function of time. `set_bounds` still calls `leave_baked`.
- Live particles keep their flag across a bounds change. A viewport shrunk
  mid-run can therefore cull, on the next step, a particle that had entered
  the larger rectangle.

### Limits to document

- A particle that crosses the whole rectangle between two steps (outside on
  one side, outside on the other) is never seen inside, so it is never
  culled. Speeds top out near 21 px per step, so this only matters for
  frames a few pixels wide.
- A particle that never enters (moving away from an off-screen emitter)
  lives until its lifetime ends, as it did before culling existed.
- Culling after entry stays permanent: a particle that leaves the rectangle
  after entering and would fall back in is gone.

## Documentation changes

- `docs/superpowers/specs/2026-10-02-cull-particles-outside-viewport-design.md`,
  "Trade-off": replace the entering-the-frame paragraph. The margin no longer
  has to cover the emitter's distance outside the bounds.
- `core/brightfx-core/README.md`, "Culling": the same change, plus the limits
  above.

## Out of scope

Retuning presets to use `cullMargin` (separate PR, now unblocked); the other
#17 follow-ups (re-culling the live pool in `set_bounds`, a result for
invalid `setBounds` sizes, `bfx_set_bounds`, the preset schema version).

## Testing

All in `simulation.rs` unit tests, using the existing runner helpers:

- The test pinning the old behavior,
  `a_particle_that_spawns_beyond_the_margin_is_culled_on_its_first_step`,
  becomes its inverse: an emitter outside the bounds with margin 0 keeps its
  particle.
- After entering, a particle is culled when it leaves the rectangle
  (an off-screen emitter whose particle crosses the frame and exits).
- A particle moving away from an off-screen emitter is kept until its
  lifetime ends.
- A particle that spawns inside the margin zone but outside the frame, and
  drifts out past the margin, is culled (it counts as entered at spawn).
- A particle that spawns inside the bounds and leaves in its first step is
  culled (the spawn position counts as inside). This is the test that fails
  if the pre-update check is removed.
- The other culling tests pass unchanged, except
  `culling_removes_some_but_not_all_of_a_burst`: its range check on every
  surviving particle encoded the old behavior (particles spawned just above
  the top edge are now kept until they enter). It now asserts the rule as an
  invariant: no particle that has entered survives outside the rectangle.
- Seek determinism with an off-screen emitter: fresh, rewound and
  forward-stepped seeks give identical buffers, with non-vacuity guards that
  some particles entered and some never did.
- The existing NaN test
  (`a_particle_with_a_non_finite_position_is_culled`) still passes unchanged:
  its NaN particle never enters, and is culled because non-finite positions
  are culled unconditionally.
