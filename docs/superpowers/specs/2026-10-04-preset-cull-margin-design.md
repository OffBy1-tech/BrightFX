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
frame), with the margins below:

| preset        | 1920x1080 no cull | 1920x1080 culled | 1080x1920 no cull | 1080x1920 culled |
|---------------|-------------------|------------------|-------------------|------------------|
| frosting-rain | 374               | 186              | 374               | 308              |
| sprinkle-rain | 270               | 136              | 270               | 215              |
| flight-arc    | 11                | 8                | 11                | 5                |
| confetti, fireflies, sparkles, bubbles | 15-215 | within 4% | same | within 4% |

The other four presets fade out inside the frame, so culling saves almost
nothing there and they are left alone.

## Design

- `frosting-rain`: `cullMargin` 24. Its capsules reach about 11 px from the
  center, so 24 is about 2x, which also covers the rotation and
  anti-aliasing. `cullMargin` is a center-only cull, so a particle is fully
  off screen once its center is past the margin.
- `sprinkle-rain`: `cullMargin` 64 (`SCATTER_MARGIN` + 4), not 24. Its emitter
  sweeps x from -60 to 1980, and the first version's margin of 24 was inside
  that sweep. A drop born outside the margin and heading outward is culled by
  the moving-away rule even though turbulence (a swing of up to 1.5 px per
  step) would carry it back onto the frame. Review measured 80 drop-steps of
  167,000 lost at 1920x1080 in 25 s, and the first one was a drop at (1918,
  618) that was on the frame. A margin that holds the emitter's whole path
  removes that. Pool cost: about 6 particles.
- `flight-arc`: `cullMargin` 120. It is a sprite-mode preset whose glyphs are
  60-120 px, scaled up by the composition from the core's size-40 cap, so the
  margin has to cover half a glyph, not the core's `size`.
- Motion, lifetimes, colors, spawn rates and the other four presets do not
  change. The three preset JSON files change only in `cullMargin` (`null` to
  the value).
- In the authored frame every emitter path is inside its margin: the rains'
  y = -20 (and `sprinkle-rain`'s x sweep) and `flight-arc`'s x = -100 against
  120. In a fitted 9:16 frame the rains' y stretches to -36, past 24, where
  #23's entry rule keeps a drop that is moving toward the frame until it has
  entered.
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
  the largest of `startSize`, `peakSize` and `endSize`, plus the glow's blur
  (the core's `glow_blur_logical`) when `glowBloom` is on. This keeps a
  particle from popping out while partly visible.
- Culling must only remove a particle nobody would have seen. Each preset with
  a margin runs twice, with and without bounds, for 25 s in both frames, and
  every particle that is on the frame in the uncut run must still be on it in
  the culled run. Culling consumes no randomness and particles do not
  interact, so a survivor is bit-identical in both runs. The census cannot
  show this, since it only sees where a particle was last. This is the guard
  that caught `sprinkle-rain`'s first margin of 24.
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
