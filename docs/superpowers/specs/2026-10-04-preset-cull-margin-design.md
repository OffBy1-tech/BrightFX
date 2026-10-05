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
| frosting-rain | 374               | 187              | 374               | 317              |
| sprinkle-rain | 270               | 138              | 270               | 217              |
| flight-arc    | 11                | 8                | 11                | 5                |
| confetti, fireflies, sparkles, bubbles | 15-215 | within 4% | same | within 4% |

The other four presets fade out inside the frame, so culling saves almost
nothing there and they are left alone.

## Design

### The margin has to hold the emitter, not only the particle

`cullMargin` is a center-only cull, so a margin of twice a particle's reach
keeps a particle from popping out while partly visible. That is not enough
for a preset whose emitter sits outside the frame. A drop born near the edge
of the margin and swung out past it by turbulence is culled by #23's
moving-away rule although the sway would carry it back onto the frame.

The margin therefore also holds the emitter's excursion outside the frame,
the spawn jitter, and the sway:

    emitter_cull_margin(excursion, turbulence) = excursion + 2 + 24 * turbulence

- 2 px is the core's spawn jitter.
- 24 px per unit of `turbulence` is the swing's amplitude. The core adds
  `0.15 * turbulence` to a velocity per step along a sine of 0.1 rad per step
  in x and 0.08 in y, so the amplitude is 15 px per unit in x and 23.4 in y;
  24 is the larger, rounded up.
- It was found in three rounds, each caught by a check that compared a culled
  run with an uncut one. A margin of 24 on `sprinkle-rain` lost 80 drop-steps
  of 167,000 at 1920x1080 (the first at (1918, 618)). A margin of the emitter's
  60 plus 4 fixed that frame and lost drops at 2560x1440 and 3840x2160, because
  `fit_track` stretches the emitter and not the margin. Measured at 12 seeds x
  3000 steps, 4 px past the emitter still lost drops at 1920x1080, 1920x1920
  and 2160x3840 and 10 px did not.

### Presets

- `frosting-rain`: `emitter_cull_margin(20, 0.4)` rounded up = 32. The emitter
  is 20 px above the frame. A capsule's ~11 px reach alone would need only 22.
- `sprinkle-rain`: `emitter_cull_margin(60, 0.8)` rounded up = 82. Its emitter
  sweeps x from -60 to 1980 (`SCATTER_MARGIN`).
- `flight-arc`: 120. It is a sprite-mode preset whose glyphs are 60-120 px,
  scaled up by the composition from the core's size-40 cap, so the margin has
  to cover half a glyph, not the core's `size`. Its emitter is 100 px left of
  the frame, which `emitter_cull_margin(100, 0)` = 102 is under.
- Motion, lifetimes, colors, spawn rates and the other four presets do not
  change. The three preset JSON files change only in `cullMargin` (`null` to
  the value).
- The motion constraint stays. The slowest drop must still clear 1920 px
  inside the 300-step life ceiling; culling only stops the excess life from
  filling the pool in a shorter frame.
- A host that never sets bounds sees no culling, so the old pool sizes
  (about 376 and 270) still apply to it.

### `fit_track` keeps the margin covering the emitter

`fit_track` stretches an emitter's path with the frame and leaves every other
pixel value alone, so a margin that held the path at 1920 px does not at 3840.
When the fitted track reaches farther outside the target frame than
`cullMargin` does, `fit_track` raises the margin to `emitter_cull_margin` for
that distance, using the config's own `turbulence`. It is never lowered, never
set on a config that has none, and untouched when the emitter stays inside the
frame (including a margin of 0). The distance is measured at the keyframes,
which are the extremes: the emitter moves in straight lines between them.

`emitter_cull_margin` is public in `fit` and the presets compute their margins
with it, so the two cannot drift apart.

### Documentation

`presets.rs`'s header and the per-preset comments describe pools that include
particles already off screen. They are updated to say what a host with bounds
sees, to keep the old figures for one without, and to explain the margins.

## Tests

`tests/presets.rs`:

- `census` takes a `bounded` switch, and the pool-headroom guard and the
  visible-death guard run both ways: bounded (what a host that calls
  `set_viewport` or `set_bounds` sees, so `cullMargin` applies) and unbounded
  (a host that never does). The visible-death guard stays: a culled particle
  dies at least a margin outside the frame.
- Every preset with a `cullMargin` must cover its visible extent: at least 2x
  the largest of `startSize`, `peakSize` and `endSize`, plus the glow's blur
  (the core's `glow_blur_logical`) when `glowBloom` is on.
- Every margin already holds its emitter's path: `fit_track` to the authored
  frame, which leaves the track as it is, must not raise it.
- Culling must only remove a particle nobody would have seen. Each preset with
  a margin runs with and without bounds, 3 seeds x 20 s, in five frames
  (1920x1080, 1080x1920, 2560x1440, 3840x2160, 1920x1920), each fitted with
  `fit_track`. Every particle that is on the frame by its reach in the uncut
  run must still be there in the culled run. Culling consumes no randomness and
  particles do not interact, so a survivor is bit-identical in both runs and
  the culled buffer is an ordered subsequence of the uncut one, which the test
  walks with two pointers. The census cannot show this, since it only sees
  where a particle was last.
- Exactly the three presets named above set a margin, and `flight-arc`'s is at
  least 120 (half a sprite glyph). The other four must leave it unset.
- The two rains must actually benefit: with culling on, the authored-frame
  pool peak is at most 75% of the no-cull peak. A relative check, so it does
  not go stale when a preset is tuned. It is not applied to `flight-arc`,
  whose pool of about 11 is too small for a ratio to be stable.
- `golden_frames_match` already runs through `set_viewport`, so culling is on
  there. Only off-screen particles are removed, so the frames should not
  change; they did not.

`fit.rs` unit tests: the margin grows to cover the excursion (x and y), grows
with turbulence, is never lowered, is never invented, and a margin of 0 stays
0 for an emitter inside the frame.

Verified once beyond the committed tests: 12 seeds x 3000 steps for each of the
three presets in nine frames (the five above plus 2160x3840, 1080x1080,
1280x720 and 480x270), with zero on-frame drops lost.

## Out of scope

Retuning motion or lifetimes (including the rains' 270-280-step life), the
other four presets, and hosts. The other #17 follow-ups (re-culling the live
pool in `set_bounds`, a result for invalid `setBounds` sizes, `bfx_set_bounds`,
the preset schema version).
