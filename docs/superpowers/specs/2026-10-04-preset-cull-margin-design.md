# Preset retune: cullMargin on the rains and flight-arc

Follow-up to #17 (PRs #22 and #23).

## Problem

A particle used to end only when its lifetime ran out, so a rain sized to
reach the bottom of the 1080x1920 delivery frame spent half its life below a
1080-high one. In the authored 16:9 frame, `frosting-rain` simulated about 374
particles to show about 180, and `sprinkle-rain` about 270 to show about 120.
`cullMargin` now removes a particle once it has left the frame, so the pool
can match what is on screen. No preset uses it yet.

## Measurement

Steady-state pool size (peak over 180 steps from t = 9 s, fitted to each
frame), with `cullMargin` at 2x the particle's visible extent:

| preset        | 1920x1080 no cull | 1920x1080 culled | 1080x1920 no cull | 1080x1920 culled |
|---------------|-------------------|------------------|-------------------|------------------|
| frosting-rain | 374               | 187              | 374               | 311              |
| sprinkle-rain | 270               | 130              | 270               | 210              |
| flight-arc    | 11                | 8                | 11                | 5                |
| confetti, fireflies, sparkles, bubbles | 15-215 | within 4% | same | within 4% |

The other four presets fade out inside the frame, so culling saves almost
nothing there and they are left alone.

## Design

- `frosting-rain` and `sprinkle-rain`: `cullMargin` 24. Their capsules reach
  about 11 px from the center, so 24 is about 2x, which also covers the
  rotation and anti-aliasing. `cullMargin` is a center-only cull, so a
  particle is fully off screen once its center is past the margin.
- `flight-arc`: `cullMargin` 120. It is a sprite-mode preset whose glyphs are
  60-120 px, scaled up by the composition from the core's size-40 cap, so the
  margin has to cover half a glyph, not the core's `size`.
- Motion, lifetimes, colors, spawn rates and the other four presets do not
  change. The three preset JSON files change only in `cullMargin` (`null` to
  the value).
- Emitters need no special handling after #23: the rains' emitter at y = -20
  is inside the margin, and `flight-arc`'s at x = -100 is outside it but
  moving toward the frame, so its particles are kept until they enter.
- The motion constraint stays. The slowest drop must still clear 1920 px
  inside the 300-step life ceiling; culling only stops the excess life from
  filling the pool in a shorter frame.
- A host that never sets bounds sees no culling, so the old pool sizes
  (about 376 and 270) still apply to it.

### Documentation

`presets.rs`'s header and the per-preset comments describe pools that include
particles already off screen. They are updated to say what a host with bounds
sees, and to keep the old figures for one without.

## Tests (`tests/presets.rs`)

- `census` takes a `bounded` switch, and the pool-headroom guard and the
  visible-death guard run both ways: bounded (what a host that calls
  `set_viewport` or `set_bounds` sees, so `cullMargin` applies) and unbounded
  (a host that never does). The visible-death guard stays: a culled particle
  dies at least a margin outside the frame.
- Every preset with a `cullMargin` must cover its visible extent: at least 2x
  `max(startSize, peakSize)`, plus `glowRadius` when `glowBloom` is on. This
  keeps a particle from popping out while partly visible.
- Exactly the three presets named above set a margin, and `flight-arc`'s is at
  least 120 (half a sprite glyph). The other four must leave it unset.
- The two rains must actually benefit: with culling on, the authored-frame
  pool peak is at most 75% of the no-cull peak. A relative check, so it does
  not go stale when a preset is tuned. It is not applied to `flight-arc`,
  whose pool of about 11 is too small for a ratio to be stable.
- `golden_frames_match` already runs through `set_viewport`, so culling is on
  there. Only off-screen particles are removed, so the frames should not
  change; they did not.

## Out of scope

Retuning motion or lifetimes (including the rains' 270-280-step life), the
other four presets, and hosts. The other #17 follow-ups (re-culling the live
pool in `set_bounds`, a result for invalid `setBounds` sizes, `bfx_set_bounds`,
the preset schema version).
