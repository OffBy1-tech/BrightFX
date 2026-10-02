# Cull particles that leave the viewport (#17)

## Problem

The core ends a particle only when its lifetime runs out, never when it
leaves the frame. With the lifetime ceiling at 300 steps, edge-spawned
presets must pick a lifetime long enough for the slowest particle to cross
the frame in every aspect ratio, so most of each pool is particles that have
already left the frame (frosting-rain simulates ~400 to show ~180), and each
rain needs its life, speed and turbulence hand-tuned per aspect.

## Design

An opt-in config field, `cullMargin`, removes a particle once its center is
more than the margin outside the viewport. Lifetime then only has to be
"long enough", and pools match what is on screen.

### Config

- `ParticleFxConfig.cull_margin: Option<f32>`, serialized `cullMargin`,
  `#[serde(default)]` so older presets load as `None` (culling off).
- Logical px. `clamp_to_bounds` clamps it to `>= 0` and reports
  `"cullMargin"` when it changes it.
- Schema version bumped, following the `spinDirection` precedent.

### Simulation

- `Simulation::set_bounds(width, height)` takes logical px. A non-finite or
  non-positive dimension clears the bounds ("no bounds").
- State: `bounds: Option<(f32, f32)>`.
- Changing the bounds calls `leave_baked()`. A baked `seek` then replays from
  zero instead of stepping forward from state built for another size (same
  hazard as #14 and #16). Setting identical bounds is not a change and
  leaves the cursor alone.
- In `step`'s `retain_mut`, after the position update, a particle is dropped
  when both `cull_margin` and `bounds` are set and its center is more than
  `margin` outside `[0, w] x [0, h]`.
- The test is a pure function of the particle's position, so it is
  deterministic under `seek`. Particle size is not added to the margin; the
  margin is the author's choice.
- With no bounds set, or `cullMargin` of `None`, nothing is culled
  (today's behavior).

### ABI and hosts

- The ABI's `set_viewport` also calls `sim.set_bounds(width / scale,
  height / scale)`, so frame mode is automatic.
- Sprite mode has no viewport. The JS wrapper (`packages/brightfx-js`) gets
  `setBounds(width, height)`; the Remotion wrapper's sprite path passes its
  `width` and `height` props.

### Trade-off

Culling is permanent. A particle that leaves and would later fall back in
(confetti thrown upward past the top edge) is gone. The field is opt-in per
preset, so authors can pick a large enough margin or leave it off.

The same applies to particles entering the frame, not only leaving it. A
particle spawns at the emitter (+/-2 px) inside `step`, before the cull, so
one whose first-step position is more than `cullMargin` outside the bounds is
dropped before it is ever drawn. Edge-spawned presets put the emitter
off-screen, so the margin must cover the emitter's distance outside the
bounds plus the jitter and one step of travel.

## Out of scope

Retuning presets (frosting-rain and others) to use `cullMargin` and shrink
their pools. That is a separate PR.

## Testing

- `simulation.rs` unit tests: culled past the margin; kept inside it; off
  when either `bounds` or `cullMargin` is unset; `set_bounds` clears the
  baked cursor, and identical bounds do not.
- Seek: the same time gives the same buffer with culling on, and forward
  seek equals replay.
- `clamp_to_bounds` for `cullMargin`, and a serde round trip including an
  older config without the field.
- ABI: `set_viewport` feeds the bounds (width / scale).
- JS wrapper test for `setBounds`; Remotion sprite path passes its size.
