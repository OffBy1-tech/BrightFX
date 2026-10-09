# Emitter Track Pre-roll Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let an effect file declare that its baked timeline began before t=0, so `seek(0)` returns a steady-state pool (closes #19).

**Architecture:** `emitterTrack.preroll` (seconds) extends the timeline to `-preroll`; keyframes and triggers may carry negative times. The 1/60 s grid stays anchored at authored t=0 and the pre-roll adds `S = grid_step(preroll)` whole steps in front of it, so step indices become absolute (0 is the start of pre-roll, `S` is t=0). Generators extend their paths backwards and every preset sets `preroll = lifetimeMax / 60`. Spec: `docs/superpowers/specs/2026-10-08-emitter-track-preroll-design.md`.

**Tech Stack:** Rust (brightfx-core, brightfx-tracks), serde/schemars, cross-language fixtures replayed by Node (wasm-pack), Swift, and C# harnesses, TypeScript types in brightfx-js.

## Global Constraints

- Work in the worktree `/Users/jason/Projects/ob1/open source/BrightFX-preroll` on branch `feat/emitter-track-preroll`. Every command below runs from that directory unless it says otherwise; `core/` is the Cargo workspace, so Rust commands are `cargo test --manifest-path core/Cargo.toml ...`.
- Schema version becomes **5**. `MIN_SCHEMA_VERSION` stays **1**. The 4→5 migration is a no-op.
- `MAX_PREROLL = 60.0` seconds. `preroll` clamps to `[0, MAX_PREROLL]`; non-finite becomes 0; both are reported as `"emitterTrack.preroll"`.
- `preroll` is **omitted from JSON when 0**.
- `preroll` 0 must reproduce today's behavior bit for bit: `core/fixtures/ffi-smoke.*`, `ffi-seek.*`, `ffi-seek-forward.*`, `ffi-frame.*`, `ffi-cull.*`, and `tracks-cues.*` must not change. Never run tests with `BRIGHTFX_REGENERATE=1` except in the steps that say so, and only for the files those steps name.
- `seek(time)` keeps clamping `time` to `[0, duration]`; pre-roll is simulated, never rendered.
- Commit messages carry no session links. End each with `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>`.

---

### Task 1: Schema version 5 with `emitterTrack.preroll`

**Files:**
- Modify: `core/brightfx-core/src/schema.rs` (consts at lines 6-17 and 166-178, `EmitterTrack` at 182, `clamp_to_bounds` at 299-345, tests after 760)
- Modify: `core/brightfx-core/src/lib.rs` (the `pub use schema::{...}` list, lines 6-8)
- Modify: `core/brightfx-core/src/abi.rs` (`migrate` at 396-404, tests at 425, 569, 582-619)
- Modify: `core/brightfx-tracks/src/json.rs:95-99`
- Modify: every `EmitterTrack {` struct literal (22 across `simulation.rs`, `schema.rs`, `abi.rs`, `fit.rs`, `sweep.rs`, `presets.rs`, `cues.rs`): add `preroll: 0.0,`
- Regenerate: `presets/*.brightfx.json` (only `"schemaVersion":5` changes)

**Interfaces:**
- Produces: `pub const MAX_PREROLL: f32 = 60.0` in `schema.rs`, re-exported from `brightfx_core`; `EmitterTrack.preroll: f32`; `SCHEMA_VERSION == 5`.

- [ ] **Step 1: Write the failing schema tests**

Append inside `mod tests` in `core/brightfx-core/src/schema.rs`, after `a_config_with_no_emitter_track_is_unaffected_by_the_duration_clamp`:

```rust
    fn tracked(preroll: f32) -> ParticleFxConfig {
        let mut config = example_config();
        config.emitter_track = Some(EmitterTrack {
            duration: 10.0,
            preroll,
            keyframes: vec![],
            triggers: vec![],
        });
        config
    }

    #[test]
    fn an_oversized_preroll_is_clamped_and_reported() {
        let mut config = tracked(1_000.0);
        let changed = config.clamp_to_bounds();
        assert_eq!(config.emitter_track.unwrap().preroll, MAX_PREROLL);
        assert!(changed.contains(&"emitterTrack.preroll"));
    }

    #[test]
    fn a_negative_or_non_finite_preroll_becomes_zero_and_is_reported() {
        for bad in [-1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut config = tracked(bad);
            let changed = config.clamp_to_bounds();
            assert_eq!(config.emitter_track.unwrap().preroll, 0.0, "preroll {bad}");
            assert!(changed.contains(&"emitterTrack.preroll"), "preroll {bad} not reported");
        }
    }

    #[test]
    fn an_in_range_preroll_is_left_alone_and_not_reported() {
        let mut config = tracked(4.5);
        let changed = config.clamp_to_bounds();
        assert_eq!(config.emitter_track.unwrap().preroll, 4.5);
        assert!(changed.is_empty());
    }

    #[test]
    fn a_zero_preroll_is_omitted_from_json_and_read_back_as_zero() {
        let json = serde_json::to_value(tracked(0.0)).unwrap();
        assert!(json["emitterTrack"].get("preroll").is_none(), "{json}");
        let back: ParticleFxConfig = serde_json::from_value(json).unwrap();
        assert_eq!(back.emitter_track.unwrap().preroll, 0.0);

        let json = serde_json::to_value(tracked(2.0)).unwrap();
        assert_eq!(json["emitterTrack"]["preroll"], 2.0);
    }

    #[test]
    fn a_track_written_before_version_5_parses_with_no_preroll() {
        let json = r#"{"duration": 1.0, "keyframes": [], "triggers": []}"#;
        let track: EmitterTrack = serde_json::from_str(json).unwrap();
        assert_eq!(track.preroll, 0.0);
    }

    #[test]
    fn keyframes_and_triggers_may_carry_negative_times() {
        let json = r#"{"duration": 1.0, "preroll": 1.0,
            "keyframes": [{"time": -1.0, "x": 0.0, "y": 0.0, "vx": null, "vy": null}],
            "triggers": [{"time": -1.0, "kind": "startContinuous"}]}"#;
        let track: EmitterTrack = serde_json::from_str(json).unwrap();
        assert_eq!(track.keyframes[0].time, -1.0);
        assert_eq!(track.triggers[0].time, -1.0);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --manifest-path core/Cargo.toml -p brightfx-core preroll`
Expected: compile error, `no field 'preroll'` on `EmitterTrack` and `MAX_PREROLL` not found.

- [ ] **Step 3: Add the field, the ceiling, and the version**

In `core/brightfx-core/src/schema.rs` replace the `SCHEMA_VERSION` doc comment and value (lines 6-11):

```rust
/// The config schema version this build writes. Version 2 added the
/// `random-palette` colour mode; version 3 the `capsule` shape and
/// `spinDirection`; version 4 `cullMargin`; version 5 `emitterTrack.preroll`
/// and negative keyframe and trigger times. Nothing was renamed or removed,
/// so an older config is a valid current body once relabelled
/// (`spinDirection`, `cullMargin`, and `preroll` default).
pub const SCHEMA_VERSION: u32 = 5;
```

After `MAX_EMITTER_TRACK_DURATION` (line 174) add:

```rust
/// Upper bound for `EmitterTrack.preroll`, in seconds. A pre-roll only needs
/// to cover one particle lifetime (5 s at the `lifetimeMax` ceiling); 60 s
/// is a sanity limit that caps the extra replay at 3600 steps. Public so
/// generators (`brightfx-tracks`) clamp the same way the core does.
pub const MAX_PREROLL: f32 = 60.0;
```

Replace the `EmitterTrack` struct (lines 180-187):

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EmitterTrack {
    pub duration: f32,
    /// Schema version 5. Seconds the timeline runs before t=0, so that
    /// `seek(0)` returns the pool after that much playback instead of an
    /// empty one. Keyframes and triggers may be authored at negative times
    /// down to `-preroll`; a trigger earlier than that fires at the start.
    /// Omitted (and written as nothing) when 0.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub preroll: f32,
    pub keyframes: Vec<EmitterKeyframe>,
    pub triggers: Vec<EmitterTrigger>,
}

fn is_zero(value: &f32) -> bool {
    *value == 0.0
}
```

In `clamp_to_bounds`, after the `cull_margin` clamp (the `if let Some(margin) = self.cull_margin.as_mut() { ... }` block, around line 338) and before the `color_stops` loop, add:

```rust
        // Not through `clamp`: a NaN survives `f32::clamp` and the equality
        // test there, and a pre-roll must be a real number of steps.
        if let Some(track) = self.emitter_track.as_mut() {
            let preroll = if track.preroll.is_finite() { track.preroll.clamp(0.0, MAX_PREROLL) } else { 0.0 };
            if preroll.to_bits() != track.preroll.to_bits() {
                track.preroll = preroll;
                changed.push("emitterTrack.preroll");
            }
        }
```

(`to_bits` so a NaN counts as changed. The `clamp` closure holds a mutable borrow of `changed` until its last call; the `cull_margin` clamp is that last call, so `changed` is free here. If the compiler disagrees, move this block to just before the function's final `changed` return, after the `color_stops` push.)

In `core/brightfx-core/src/lib.rs`, add `MAX_PREROLL` to the `pub use schema::{...}` list next to `MAX_EMITTER_TRACK_DURATION`.

- [ ] **Step 4: Fix the remaining struct literals**

Run: `cargo build --manifest-path core/Cargo.toml --all-targets 2>&1 | grep -c 'missing field .preroll.'`
Expected: a count around 22. Add `preroll: 0.0,` after `duration:` in every `EmitterTrack {` literal the compiler lists (in `simulation.rs`, `schema.rs`, `abi.rs`, `fit.rs`, `sweep.rs`, `presets.rs`, `cues.rs`), including the one inside `sweep_track` and the one in `scatter_sweep` and `static_track`. Re-run until the build is clean.

- [ ] **Step 5: Update the version tests**

In `core/brightfx-core/src/abi.rs`:

Replace the `migrate` comment (line 399-400) with:
```rust
    // 1 -> 2 -> 3 -> 4 -> 5 only added vocabulary (and defaulted fields):
    // the body is already valid.
```

In `parse_config_applies_the_same_gate_as_set_config` (line 425-426) change `"schemaVersion": 5` to `6` and the expected message to `"unsupported schemaVersion 6"`.

In `the_version_is_checked_before_the_body_is_deserialized` (line 572) change `"schemaVersion": 5` to `6`.

In `older_versions_still_load_and_read_back_as_the_current_version` change the loop to `for version in [1u64, 2, 3, 4]` and the comment's first line to `// v2 to v5 only added vocabulary and defaulted fields, so an`.

Replace `the_current_version_is_4_and_loads`:
```rust
    #[test]
    fn the_current_version_is_5_and_loads() {
        assert_eq!(SCHEMA_VERSION, 5);
        let json = serde_json::to_string(&ParticleFxConfig::default()).unwrap();
        assert_eq!(parse_config(&json).unwrap().schema_version, 5);
    }
```

In `versions_outside_the_supported_range_are_rejected_clearly` change `[0u64, 5]` to `[0u64, 6]` and `"1-4"` to `"1-5"`.

In `core/brightfx-tracks/src/json.rs` (lines 95-99) change `json!(5)` to `json!(6)` and `"unsupported schemaVersion 5"` to `"unsupported schemaVersion 6"`.

- [ ] **Step 6: Run the schema, abi, and tracks unit tests**

Run: `cargo test --manifest-path core/Cargo.toml -p brightfx-core --lib && cargo test --manifest-path core/Cargo.toml -p brightfx-tracks --lib`
Expected: all pass, including the six new `preroll` tests.

- [ ] **Step 7: Regenerate the presets and check the diff**

Run: `(cd core && cargo run -p brightfx-tracks --example gen_presets -- ../presets) && git diff --stat presets/ && git diff presets/ | grep '^[-+]{' | cut -c1-22 | sort | uniq -c && (grep -l preroll presets/*.json || echo "no preroll key, as expected")`
Expected: seven files changed; the cut lines are seven `-{"schemaVersion":4` and seven `+{"schemaVersion":5`; the last line is `no preroll key, as expected`. Confirm nothing else moved: `git diff presets/ | grep '^[-+]{' | sed 's/"schemaVersion":[45]//' | sort | uniq -u | wc -l` prints `0`.

- [ ] **Step 8: Run the whole Rust suite**

Run: `cargo test --manifest-path core/Cargo.toml`
Expected: all pass. In particular `committed_json_matches_the_generator`, `golden_frames_match`, and every `*_fixture` test pass without regeneration, and `git status --short core/fixtures presets` shows only the seven preset files.

- [ ] **Step 9: Commit**

```bash
git add core presets
git commit -m "Schema 5: emitterTrack.preroll, clamped to [0, 60], omitted when 0

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 2: The core runs the pre-roll before t=0

**Files:**
- Modify: `core/brightfx-core/src/simulation.rs` (`grid_step` at 55-64, `BakedTrack` at 81-91, `seek` at 375-423, `restart_track` at 429-434, `build_baked_track` at 806-827, tests)

**Interfaces:**
- Consumes: `EmitterTrack.preroll`, `MAX_PREROLL` (Task 1).
- Produces: `fn signed_grid_step(target: f32) -> i64` (private); `BakedTrack { preroll: f32, preroll_steps: u32, .. }`; `seek` semantics per the spec. No public signature changes.

- [ ] **Step 1: Write the failing tests**

Add inside `mod tests` in `core/brightfx-core/src/simulation.rs`, after `seek_is_deterministic_for_the_same_time`:

```rust
    /// Every float of the buffer as bits, read as the flat floats a host
    /// sees. "Bit-identical" compares these: f32 `==` holds for -0.0 against
    /// +0.0 and the harnesses compare bitwise. Finite first, since identical
    /// NaNs share bits.
    fn buffer_bits(sim: &Simulation) -> Vec<u32> {
        let buffer = sim.buffer();
        let len = std::mem::size_of_val(buffer) / std::mem::size_of::<f32>();
        // SAFETY: `ParticleInstance` is `#[repr(C)]` and all `f32` (its
        // stride is asserted at compile time), so this is `len`
        // initialized, aligned f32s.
        let floats = unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<f32>(), len) };
        assert!(floats.iter().all(|f| f.is_finite()), "buffer contains NaN or inf");
        floats.iter().map(|f| f.to_bits()).collect()
    }

    /// The same effect two ways: authored from `-preroll` with that
    /// pre-roll, or with every time moved `shift` later and no pre-roll.
    /// `x0`/`x1` are the emitter's x at the first and last keyframe; equal
    /// values park it, so sampling the path costs no float rounding.
    fn prerolled_config(preroll: f32, shift: f32, x0: f32, x1: f32) -> ParticleFxConfig {
        let mut config = base_config();
        config.emitter.spawn_rate_while_active = 2.0;
        config.emitter.spawn_burst_size = 3;
        config.lifetime_min = 100.0;
        config.lifetime_max = 100.0;
        config.emitter_track = Some(EmitterTrack {
            duration: 1.0 + shift,
            preroll,
            keyframes: vec![
                EmitterKeyframe { time: -0.5 + shift, x: x0, y: 0.0, vx: Some(0.0), vy: Some(0.0) },
                EmitterKeyframe { time: 1.0 + shift, x: x1, y: 0.0, vx: Some(0.0), vy: Some(0.0) },
            ],
            triggers: vec![
                EmitterTrigger { time: -0.5 + shift, kind: TriggerKind::StartContinuous },
                EmitterTrigger { time: 0.25 + shift, kind: TriggerKind::Burst },
            ],
        });
        config
    }

    #[test]
    fn a_prerolled_track_is_the_same_track_started_earlier() {
        // The defining property: pre-roll is exactly "the timeline began
        // `preroll` earlier", and nothing else. With a parked emitter the
        // two run the same whole steps from reset, so they are bit-identical.
        for t in [0.0f32, 0.25, 0.5, 1.0] {
            let mut prerolled = Simulation::new(prerolled_config(0.5, 0.0, 10.0, 10.0), 5);
            prerolled.seek(t);
            let mut shifted = Simulation::new(prerolled_config(0.0, 0.5, 10.0, 10.0), 5);
            shifted.seek(t + 0.5);
            assert!(prerolled.particle_count() > 0, "vacuous at t={t}");
            assert_eq!(buffer_bits(&prerolled), buffer_bits(&shifted), "t={t}");
        }
    }

    #[test]
    fn a_prerolled_track_follows_its_path_through_the_preroll() {
        // A moving emitter: the two sample the path at times that differ by
        // the shift, which rounds differently in f32, so this holds to a
        // tolerance rather than bitwise. 1e-3 is the libm drift tolerance
        // the fixtures use; the rounding here is far below it.
        for t in [0.0f32, 0.5, 1.0] {
            let mut prerolled = Simulation::new(prerolled_config(0.5, 0.0, -30.0, 60.0), 5);
            prerolled.seek(t);
            let mut shifted = Simulation::new(prerolled_config(0.0, 0.5, -30.0, 60.0), 5);
            shifted.seek(t + 0.5);
            assert_eq!(prerolled.particle_count(), shifted.particle_count(), "t={t}");
            for (a, b) in prerolled.buffer().iter().zip(shifted.buffer()) {
                assert!((a.x - b.x).abs() <= 1e-3 && (a.y - b.y).abs() <= 1e-3, "t={t}: {a:?} vs {b:?}");
            }
        }
    }

    #[test]
    fn seek_zero_returns_the_pool_the_preroll_built() {
        let mut sim = Simulation::new(prerolled_config(0.5, 0.0, 10.0, 10.0), 5);
        sim.seek(0.0);
        // 0.5 s of continuous emission at 2 per step, all still alive.
        assert_eq!(sim.particle_count(), 60);

        // The same authored timeline with no pre-roll has run no steps at 0:
        // its triggers before the start fire at the reset, and nothing spawns
        // until a step runs.
        let mut none = Simulation::new(prerolled_config(0.0, 0.0, 10.0, 10.0), 5);
        none.seek(0.0);
        assert_eq!(none.particle_count(), 0);
    }

    #[test]
    fn forward_seek_matches_a_fresh_seek_through_a_preroll() {
        let mut forward = Simulation::new(prerolled_config(0.5, 0.0, -30.0, 60.0), 5);
        forward.seek(0.0);
        forward.seek(0.25);
        forward.seek(0.75);
        let mut fresh = Simulation::new(prerolled_config(0.5, 0.0, -30.0, 60.0), 5);
        fresh.seek(0.75);
        assert!(forward.particle_count() > 0, "vacuous");
        assert_eq!(buffer_bits(&forward), buffer_bits(&fresh));
    }

    fn burst_only_at(preroll: f32, time: f32) -> ParticleFxConfig {
        let mut config = prerolled_config(preroll, 0.0, -30.0, 60.0);
        config.emitter.spawn_rate_while_active = 0.0;
        config.emitter_track.as_mut().unwrap().triggers = vec![EmitterTrigger { time, kind: TriggerKind::Burst }];
        config
    }

    #[test]
    fn a_trigger_at_a_negative_time_fires_at_its_step() {
        // The emitter moves from x=-30 at -0.5 s to x=60 at 1.0 s (60 px/s).
        // A burst at -0.25 s spawns at x=-15, and with no initial speed or
        // inheritance the particles stay there, to within the 2 px jitter.
        let mut sim = Simulation::new(burst_only_at(0.5, -0.25), 5);
        sim.seek(0.0);
        assert_eq!(sim.particle_count(), 3);
        for p in sim.buffer() {
            assert!((p.x + 15.0).abs() < 2.5, "x was {}", p.x);
        }
    }

    #[test]
    fn a_trigger_at_or_before_the_preroll_start_fires_at_the_start() {
        for time in [-0.5f32, -5.0] {
            let mut sim = Simulation::new(burst_only_at(0.5, time), 5);
            sim.seek(0.0);
            assert_eq!(sim.particle_count(), 3, "burst at {time}");
            for p in sim.buffer() {
                assert!((p.x + 30.0).abs() < 2.5, "burst at {time}: x was {}", p.x);
            }
        }
    }

    #[test]
    fn an_unvalidated_preroll_is_capped_and_a_non_finite_one_is_zero() {
        // Built straight from the struct, so `clamp_to_bounds` never ran.
        let mut huge = Simulation::new(prerolled_config(f32::MAX, 0.0, 10.0, 10.0), 5);
        huge.seek(0.0); // at most 3600 steps, not f32::MAX * 60
        // 62, not 60: inside a 60 s pre-roll the step that *ends* at -0.5 s
        // exists, so the StartContinuous fires in it (the usual at-or-after
        // rule) and emission runs 31 steps. With a 0.5 s pre-roll the
        // trigger sits at the start and fires at the reset instead.
        assert_eq!(huge.particle_count(), 62, "emission still starts at -0.5 s inside a 60 s pre-roll");

        let mut nan = Simulation::new(prerolled_config(f32::NAN, 0.0, 10.0, 10.0), 5);
        nan.seek(0.0);
        assert_eq!(nan.particle_count(), 0, "a NaN pre-roll is no pre-roll");
    }

    #[test]
    fn negative_times_take_the_grid_point_at_or_after_them() {
        assert_eq!(signed_grid_step(-0.5), -30);
        assert_eq!(signed_grid_step(-0.49), -29);
        assert_eq!(signed_grid_step(-0.5 / 60.0), 0);
        assert_eq!(signed_grid_step(0.0), 0);
        assert_eq!(signed_grid_step(1.0 + 0.4 / 60.0), 61);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --manifest-path core/Cargo.toml -p brightfx-core --lib preroll`
Expected: compile error on `signed_grid_step`; after stubbing nothing, the behavior tests fail (`seek_zero_returns_the_pool_the_preroll_built` sees 0, not 60).

- [ ] **Step 3: Implement**

In `core/brightfx-core/src/simulation.rs`:

Add `MAX_PREROLL` to the `use crate::schema::{...}` import at the top.

Replace `grid_step` (lines 55-64) with:

```rust
/// `grid_step` for a signed time: the same snap-then-ceil rule applied in
/// f64, so -0.5 s maps to -30 and -0.49 s to -29 (the grid point at or
/// after). Trigger times before t=0 resolve through this.
fn signed_grid_step(target: f32) -> i64 {
    let steps = target as f64 * GRID_RATE;
    let nearest = steps.round();
    if (steps - nearest).abs() <= grid_tolerance(target.abs()) {
        nearest as i64
    } else {
        steps.ceil() as i64
    }
}

fn grid_step(target: f32) -> u32 {
    signed_grid_step(target.max(0.0)) as u32
}
```

Replace `BakedTrack` (lines 80-91) with:

```rust
/// `config.emitter_track` prepared for replay. Derived from `config`, so
/// it is rebuilt wherever `config` is replaced.
#[derive(Debug)]
struct BakedTrack {
    keyframes: Vec<EmitterKeyframe>,
    /// In chronological order, which is also fire-step order. Triggers
    /// must be evaluated in time order regardless of how they were
    /// authored -- an out-of-order StopContinuous/StartContinuous pair
    /// landing in the same step would otherwise apply in array order
    /// instead of time order.
    triggers: Vec<BakedTrigger>,
    /// `duration`, floored at 0 and capped at `MAX_EMITTER_TRACK_DURATION`.
    duration_cap: f32,
    /// `preroll`, floored at 0, capped at `MAX_PREROLL`, 0 if not finite.
    preroll: f32,
    /// Whole grid steps the timeline runs before authored t=0. Step
    /// indices are absolute: 0 is the start of the pre-roll and
    /// `preroll_steps` is t=0, so a `fire_step` or a `baked` cursor of
    /// `preroll_steps` means "at t=0".
    preroll_steps: u32,
}
```

Update the `BakedTrigger` doc comment (line 66-70) to say `fire_step` is absolute: replace its text with:

```rust
/// One of the track's triggers with the step it fires in resolved ahead
/// of time: `fire_step` is the absolute grid step at or after its authored
/// time (`preroll_steps` plus the signed step of the time), so the trigger
/// fires in the step that *ends* there -- exactly the step `seek` runs to
/// reach that time. `0` means it fires at the reset, before any step runs,
/// which is where a trigger at or before `-preroll` lands.
```

Replace `build_baked_track` (lines 806-827) with:

```rust
fn build_baked_track(config: &ParticleFxConfig) -> Option<Arc<BakedTrack>> {
    let track = config.emitter_track.as_ref()?;

    // `clamp_to_bounds` enforces these ranges for well-behaved hosts, but
    // `Simulation` can be built directly from unvalidated JSON, so the
    // ceilings must hold regardless. `max`/`min` rather than `clamp`: a
    // NaN duration must collapse to 0.0 here, and `clamp` would
    // propagate it.
    #[allow(clippy::manual_clamp)]
    let duration_cap = track.duration.max(0.0).min(MAX_EMITTER_TRACK_DURATION);
    let preroll = if track.preroll.is_finite() { track.preroll.clamp(0.0, MAX_PREROLL) } else { 0.0 };
    let preroll_steps = grid_step(preroll);

    let mut triggers = track.triggers.clone();
    triggers.sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap_or(std::cmp::Ordering::Equal));
    // `signed_grid_step` is monotonic in its argument, so chronological
    // order is also fire-step order. A trigger at or before the start of
    // the pre-roll fires at the reset, exactly, rather than through the
    // grid arithmetic: `-preroll` need not sit on the grid, and a
    // generator puts its StartContinuous there.
    let triggers = triggers
        .iter()
        .map(|trig| {
            let fire_step = if trig.time <= -preroll {
                0
            } else {
                (preroll_steps as i64 + signed_grid_step(trig.time)).max(0) as u32
            };
            BakedTrigger { fire_step, kind: trig.kind }
        })
        .collect();

    Some(Arc::new(BakedTrack { keyframes: track.keyframes.clone(), triggers, duration_cap, preroll, preroll_steps }))
}
```

In `seek` (lines 375-423) replace the body from `let target = ...` through the loop with:

```rust
        // `duration_cap` bounds the loop: a caller-supplied `time` (a
        // corrupt project file, a UI bug, ...) can't drive an unbounded
        // number of synchronous step iterations. The pre-roll adds at most
        // `grid_step(MAX_PREROLL)` steps in front; it is simulated, never
        // rendered, so `time` still clamps at 0.
        let target = time.max(0.0).min(track.duration_cap);
        let n = track.preroll_steps + grid_step(target);

        // On the forward path the `active` the loop starts from is
        // `self.emitter_active` as the previous seek left it. That is
        // sound because every other writer of `emitter_active` calls
        // `leave_baked` first, so a cursor can only survive a pure replay.
        let from = match self.baked {
            Some(step) if step <= n => step,
            _ => {
                self.restart_track(&track);
                0
            }
        };

        for k in from..n {
            // Grid times are computed from the absolute step index, never
            // accumulated, so step k is the same on every path. Authored
            // time is the index less the pre-roll, negative inside it; a
            // negative product is the exact negation of the positive one,
            // so a track with no pre-roll gets today's values bit for bit.
            // The sample is clamped to the duration so the final step of
            // an off-grid track does not extrapolate past the last
            // keyframe window.
            let authored = (k + 1) as i64 - track.preroll_steps as i64;
            let t_next = (authored as f32 * PLAYBACK_STEP).min(track.duration_cap);

            let (x, y, vx, vy) = sample_track(&track.keyframes, t_next);
            // Update position/velocity for this step before evaluating
            // triggers, so a `Burst` firing this step spawns at the
            // correct sampled position.
            let active = self.emitter_active;
            self.place_emitter(x, y, vx, vy, active);

            self.fire_triggers(&track, k + 1);

            self.step(PLAYBACK_STEP);
        }

        self.baked = Some(n);
```

Also update the `seek` doc comment: after the first paragraph add:

```rust
    /// With `emitterTrack.preroll` set, the replay starts that many seconds
    /// before t=0 (`preroll_steps` whole steps in front of the grid), so
    /// `seek(0)` returns the pool those steps built. `time` is still
    /// clamped at 0: the pre-roll is simulated, never rendered.
```

Replace `restart_track` (lines 429-434) with:

```rust
    /// Resets at the start of the timeline (`-preroll`, which is t=0 when
    /// there is no pre-roll) and fires the `fire_step == 0` triggers --
    /// those authored at or before the start. Baked playback requires an
    /// explicit `StartContinuous` (or a `Burst`) to spawn anything --
    /// unlike live mode, seek never implicitly emits from t=0.
    fn restart_track(&mut self, track: &BakedTrack) {
        self.reset();
        let (x0, y0, vx0, vy0) = sample_track(&track.keyframes, -track.preroll);
        self.place_emitter(x0, y0, vx0, vy0, false);
        self.fire_triggers(track, 0);
    }
```

Update the `baked` field comment (line 142-146): change `Whole steps applied since the last reset in baked mode.` to `Absolute whole steps applied since the last reset in baked mode (step 0 is the start of the pre-roll, see BakedTrack::preroll_steps).`

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test --manifest-path core/Cargo.toml -p brightfx-core --lib`
Expected: all pass, including the nine new tests and every existing seek and grid test.

- [ ] **Step 5: Prove the fixtures are untouched**

Run: `cargo test --manifest-path core/Cargo.toml && git status --short core/fixtures presets`
Expected: all pass; `git status` prints nothing (a pre-roll of 0 changed no recorded buffer).

- [ ] **Step 6: Commit**

```bash
git add core/brightfx-core/src/simulation.rs
git commit -m "Core: seek runs emitterTrack.preroll before t=0

Step indices are absolute: the pre-roll adds grid_step(preroll) whole
steps in front of authored t=0, the grid itself stays anchored at 0, and
seek still clamps time at 0. A pre-rolled track is bit-identical to the
same track started earlier.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 3: Cross-language fixture `ffi-preroll`

**Files:**
- Create: `core/fixtures/ffi-preroll.config.json`
- Create: `core/fixtures/ffi-preroll.expected.json` (recorded by the test)
- Create: `core/brightfx-core/tests/preroll_fixture.rs`
- Modify: `core/brightfx-core/tests/common/seek.rs`
- Modify: `core/harnesses/node/smoke.mjs` (after the forward-seek test, line 118)
- Modify: `core/harnesses/swift/main.swift` (after the forward-seek block, before `// --- frame protocol`, line 196)
- Modify: `core/harnesses/csharp/Program.cs` (after the forward-seek block, before `string frameConfigJson`, line 266)
- Modify: `core/README.md` (fixture list at 95-120, regenerate command at 150)

**Interfaces:**
- Consumes: `seek` with pre-roll (Task 2).
- Produces: `load_sim_for(config_file, seed)`, `run_protocol_for(config_file, seed, times)`, `check_fixture_for(config_file, expected_file, seed, times)` in `tests/common/seek.rs`; the two fixture files every harness reads.

The spec lists a second expected file for the forward seek. This task instead compares the forward seek against a fresh seek inside each harness at tolerance 0, as the `ffi-seek-forward` block already does: one file fewer and a stronger check.

- [ ] **Step 1: Write the fixture config**

Create `core/fixtures/ffi-preroll.config.json`:

```json
{
  "schemaVersion": 5,
  "id": "ffi-preroll",
  "name": "FFI Pre-roll",
  "category": "elemental",
  "description": "Shared fixture for cross-target pre-roll checks: a track that starts 1 s before t=0",
  "author": "Off By 1",
  "icon": "fire",
  "emitter": {
    "spawnRateWhileActive": 10.0,
    "spawnBurstSize": 20,
    "spawnRateIdle": 0.0,
    "emissionPattern": "trail",
    "emissionAngle": 0.0,
    "emissionSpread": 30.0,
    "velocityInheritance": 0.5
  },
  "emitterTrack": {
    "duration": 2.0,
    "preroll": 1.0,
    "keyframes": [
      {
        "time": -1.0,
        "x": -60.0,
        "y": 20.0,
        "vx": 1.5,
        "vy": -0.5
      },
      {
        "time": 0.0,
        "x": 0.0,
        "y": 0.0,
        "vx": 1.5,
        "vy": -0.5
      },
      {
        "time": 1.0,
        "x": 60.0,
        "y": -20.0,
        "vx": 1.5,
        "vy": 0.5
      },
      {
        "time": 2.0,
        "x": 120.0,
        "y": 0.0,
        "vx": 1.5,
        "vy": -0.5
      }
    ],
    "triggers": [
      {
        "time": -1.0,
        "kind": "startContinuous"
      },
      {
        "time": -0.5,
        "kind": "burst"
      },
      {
        "time": 1.0,
        "kind": "burst"
      }
    ]
  },
  "shape": "circle",
  "blendMode": "lighter",
  "glowBloom": true,
  "glowRadius": 10.0,
  "initialSpeedMin": 1.0,
  "initialSpeedMax": 3.0,
  "gravityX": 0.0,
  "gravityY": 1.0,
  "drag": 0.98,
  "turbulence": 0.5,
  "vortexAttraction": 0.0,
  "rotationSpeedMin": 0.0,
  "rotationSpeedMax": 2.0,
  "lifetimeMin": 30.0,
  "lifetimeMax": 60.0,
  "startSize": 4.0,
  "peakSize": 6.0,
  "endSize": 0.0,
  "sizeCurve": "grow-shrink",
  "colorMode": "gradient-lifetime",
  "primaryColor": "#ff6600",
  "secondaryColor": "#ffcc00",
  "accentColor": "#ffffff",
  "colorStops": null,
  "rainbowSpeed": 0.0,
  "startAlpha": 1.0,
  "peakAlpha": 1.0,
  "endAlpha": 0.0,
  "soundOnSpawn": null
}
```

- [ ] **Step 2: Generalize the shared seek scaffolding**

Replace the three functions in `core/brightfx-core/tests/common/seek.rs` with:

```rust
/// A handle loaded with the config file `config_file` from the fixtures
/// directory, ready to seek.
pub fn load_sim_for(config_file: &str, seed: u64) -> AbiSimulation {
    let config = std::fs::read_to_string(fixtures_dir().join(config_file))
        .unwrap_or_else(|_| panic!("{config_file} missing"));
    let mut sim = AbiSimulation::new(seed);
    let envelope: serde_json::Value =
        serde_json::from_str(&sim.set_config(&config)).expect("envelope is not JSON");
    assert_eq!(envelope["ok"], true, "{config_file} rejected: {envelope}");
    sim
}

/// A handle loaded with the shared seek config, ready to seek.
pub fn load_sim(seed: u64) -> AbiSimulation {
    load_sim_for("ffi-seek.config.json", seed)
}

/// The seek protocol every harness must reproduce exactly: `seek_times`
/// in order on one handle loaded with `config_file`.
pub fn run_protocol_for(config_file: &str, seed: u64, seek_times: &[f32]) -> (u32, Vec<f32>) {
    let mut sim = load_sim_for(config_file, seed);
    for &time in seek_times {
        sim.seek(time);
    }
    (sim.particle_count(), sim.buffer_slice().to_vec())
}

/// `run_protocol_for` on the shared seek config.
pub fn run_protocol(seed: u64, seek_times: &[f32]) -> (u32, Vec<f32>) {
    run_protocol_for("ffi-seek.config.json", seed, seek_times)
}

/// Runs the protocol on `config_file` and checks it against `expected_file`
/// in the fixtures directory, or rewrites that file when regenerating.
pub fn check_fixture_for(config_file: &str, expected_file: &str, seed: u64, seek_times: &[f32]) {
    let (count, buffer) = run_protocol_for(config_file, seed, seek_times);
    let record = || {
        serde_json::json!({
            "seed": seed,
            "seekTimes": seek_times,
            "particleFloats": AbiSimulation::PARTICLE_FLOATS,
            "tolerance": LIBM_DRIFT_TOLERANCE,
            "particleCount": count,
            "buffer": buffer,
        })
    };
    let Some(expected) = load_or_regenerate(&fixtures_dir().join(expected_file), record) else { return };

    assert_eq!(expected["seed"].as_u64().unwrap(), seed, "fixture seed drifted");
    assert_eq!(floats(&expected["seekTimes"]), seek_times, "seek times drifted");
    assert_eq!(
        expected["particleFloats"].as_u64().unwrap() as u32,
        AbiSimulation::PARTICLE_FLOATS,
        "stride drifted"
    );
    assert_eq!(
        expected["tolerance"].as_f64().unwrap() as f32,
        LIBM_DRIFT_TOLERANCE,
        "tolerance drifted"
    );
    assert_eq!(expected["particleCount"].as_u64().unwrap() as u32, count);
    assert_within_libm_drift(&buffer, &floats(&expected["buffer"]));
}

/// `check_fixture_for` on the shared seek config.
pub fn check_fixture(expected_file: &str, seed: u64, seek_times: &[f32]) {
    check_fixture_for("ffi-seek.config.json", expected_file, seed, seek_times)
}
```

- [ ] **Step 3: Write the Rust fixture test**

Create `core/brightfx-core/tests/preroll_fixture.rs`:

```rust
//! Generates and verifies the cross-target pre-roll fixture.
//!
//! `ffi-seek` and `ffi-seek-forward` start their track at t=0. This one
//! starts 1 s earlier (`emitterTrack.preroll`), with its StartContinuous
//! and a burst at negative times, and records the buffer at `seek(0)`:
//! the pool the pre-roll built. Every harness reproduces it, then seeks
//! the same handle on to 0.5 s and requires that to be bit-identical to a
//! fresh seek, which is the forward path leaving the pre-roll. Run with
//! `BRIGHTFX_REGENERATE=1` to rewrite `ffi-preroll.expected.json` after an
//! intentional simulation change.

mod common;

use common::bits;
use common::seek::{check_fixture_for, load_sim_for, run_protocol_for};

const CONFIG: &str = "ffi-preroll.config.json";
const EXPECTED: &str = "ffi-preroll.expected.json";
const SEED: u64 = 42;
/// Only 0: the point of the fixture is the state the pre-roll leaves at
/// t=0. The forward step out of the pre-roll is checked in-process below
/// and in each harness, against a fresh seek rather than a recording.
const SEEK_TIMES: [f32; 1] = [0.0];
const FORWARD_TIME: f32 = 0.5;

#[test]
fn the_fixture_matches_the_recorded_expectation() {
    check_fixture_for(CONFIG, EXPECTED, SEED, &SEEK_TIMES);
}

#[test]
fn seeking_to_zero_is_not_vacuous() {
    let (count, buffer) = run_protocol_for(CONFIG, SEED, &SEEK_TIMES);
    assert!(count > 0, "pre-roll fixture is vacuous -- seek(0) left no particles");
    assert!(buffer.iter().all(|v| v.is_finite()), "fixture contains NaN or inf");
}

#[test]
fn seeking_on_out_of_the_preroll_is_bit_identical_to_a_fresh_seek() {
    let mut forward = load_sim_for(CONFIG, SEED);
    for &time in &SEEK_TIMES {
        forward.seek(time);
    }
    forward.seek(FORWARD_TIME);
    let mut fresh = load_sim_for(CONFIG, SEED);
    fresh.seek(FORWARD_TIME);
    assert_eq!(fresh.particle_count(), forward.particle_count());
    assert_eq!(bits(fresh.buffer_slice()), bits(forward.buffer_slice()), "not bit-identical");
}
```

- [ ] **Step 4: Record the expectation, then verify it**

Run: `BRIGHTFX_REGENERATE=1 cargo test --manifest-path core/Cargo.toml -p brightfx-core --test preroll_fixture`
Expected: prints `regenerated .../core/fixtures/ffi-preroll.expected.json`; all three tests pass.

Run: `cargo test --manifest-path core/Cargo.toml -p brightfx-core --test preroll_fixture --test seek_fixture --test seek_forward_fixture && git status --short core/fixtures`
Expected: all pass; `git status` lists only `?? core/fixtures/ffi-preroll.config.json` and `?? core/fixtures/ffi-preroll.expected.json`. Then: `python3 -c "import json; e=json.load(open('core/fixtures/ffi-preroll.expected.json')); print(e['particleCount'])"` prints a number above 0.

- [ ] **Step 5: Add the Node harness block**

In `core/harnesses/node/smoke.mjs`, after the `seekForwardExpected` constant (line 26) add:

```js
const prerollConfigJson = readFileSync(join(fixtures, "ffi-preroll.config.json"), "utf8");
const prerollExpected = JSON.parse(readFileSync(join(fixtures, "ffi-preroll.expected.json"), "utf8"));
```

After the `forward seeks reproduce the Rust buffer and match a fresh seek exactly` test (ends line 118) add:

```js
test("a pre-rolled track is already running at t=0 and steps on bit-identically", () => {
  assert.ok(prerollExpected.particleCount > 0, "pre-roll fixture is vacuous");
  assert.deepEqual(prerollExpected.seekTimes, [0], "the pre-roll fixture records seek(0)");
  const sim = new mod.BrightFx(BigInt(prerollExpected.seed));
  applyConfig(sim, prerollConfigJson);
  for (const time of prerollExpected.seekTimes) sim.seek(time);
  assertBufferMatches(sim, prerollExpected);

  // Leaving the pre-roll is a forward seek: bit-identical to a fresh one.
  sim.seek(0.5);
  const fresh = new mod.BrightFx(BigInt(prerollExpected.seed));
  applyConfig(fresh, prerollConfigJson);
  fresh.seek(0.5);
  const forwardFloats = Array.from(readBuffer(sim, mod));
  const freshFloats = Array.from(readBuffer(fresh, mod));
  assert.ok(forwardFloats.every(Number.isFinite), "forward buffer is not finite");
  assert.ok(freshFloats.every(Number.isFinite), "fresh buffer is not finite");
  assert.deepStrictEqual(freshFloats, forwardFloats);
  sim.free();
  fresh.free();
});
```

- [ ] **Step 6: Add the Swift harness block**

In `core/harnesses/swift/main.swift`, before `// --- frame protocol` (line 196) add:

```swift
    // --- pre-roll protocol -----------------------------------------------------
    let prerollConfigJson = try text("ffi-preroll.config.json")
    let prerollExpected: SeekExpectation = try decode("ffi-preroll.expected.json")
    guard prerollExpected.particleCount > 0, prerollExpected.seekTimes.count == 1, prerollExpected.seekTimes[0] == 0 else {
        throw Failure("pre-roll fixture is vacuous or does not record seek(0)")
    }

    guard let prerollSim = bfx_simulation_new(prerollExpected.seed) else { throw Failure("bfx_simulation_new returned NULL") }
    defer { bfx_simulation_free(prerollSim) }
    try applyConfig(prerollSim, prerollConfigJson, "pre-roll config")
    for time in prerollExpected.seekTimes { bfx_seek(prerollSim, time) }
    try assertBufferMatches(
        prerollSim, count: prerollExpected.particleCount, stride: prerollExpected.particleFloats,
        buffer: prerollExpected.buffer, tolerance: prerollExpected.tolerance)

    // Leaving the pre-roll is a forward seek: bit-identical to a fresh one.
    bfx_seek(prerollSim, 0.5)
    let prerollCount = bfx_particle_count(prerollSim)
    guard let prerollBase = bfx_buffer_ptr(prerollSim) else { throw Failure("bfx_buffer_ptr returned NULL") }
    let prerollFloats = Array(
        UnsafeBufferPointer(start: prerollBase, count: Int(prerollCount * prerollExpected.particleFloats)))

    guard let prerollFresh = bfx_simulation_new(prerollExpected.seed) else { throw Failure("bfx_simulation_new returned NULL") }
    defer { bfx_simulation_free(prerollFresh) }
    try applyConfig(prerollFresh, prerollConfigJson, "pre-roll fresh config")
    bfx_seek(prerollFresh, 0.5)
    try assertBufferMatches(
        prerollFresh, count: prerollCount, stride: prerollExpected.particleFloats,
        buffer: prerollFloats, tolerance: 0)
    print("  ok  a pre-rolled track is already running at t=0 and steps on bit-identically")

```

- [ ] **Step 7: Add the C# harness block**

In `core/harnesses/csharp/Program.cs`, before `string frameConfigJson = ...` (line 266) add:

```csharp
        string prerollConfigJson = File.ReadAllText(Path.Combine(fixtures, "ffi-preroll.config.json"));
        SeekExpectation prerollExpected = JsonSerializer.Deserialize<SeekExpectation>(
            File.ReadAllText(Path.Combine(fixtures, "ffi-preroll.expected.json")))!;
        if (prerollExpected.ParticleCount == 0 || prerollExpected.SeekTimes.Length != 1 || prerollExpected.SeekTimes[0] != 0f)
            Fail("pre-roll fixture is vacuous or does not record seek(0)");

        using var prerollSim = new Simulation(prerollExpected.Seed);
        ApplyConfig(prerollSim.Ptr, prerollConfigJson, "pre-roll config");
        foreach (float time in prerollExpected.SeekTimes) Native.bfx_seek(prerollSim.Ptr, time);
        AssertBufferMatches(prerollSim.Ptr, prerollExpected.ParticleCount, prerollExpected.ParticleFloats, prerollExpected.Buffer, prerollExpected.Tolerance);

        // Leaving the pre-roll is a forward seek: bit-identical to a fresh one.
        Native.bfx_seek(prerollSim.Ptr, 0.5f);
        uint prerollCount = Native.bfx_particle_count(prerollSim.Ptr);
        IntPtr prerollBase = Native.bfx_buffer_ptr(prerollSim.Ptr);
        if (prerollBase == IntPtr.Zero) Fail("bfx_buffer_ptr returned NULL");
        float[] prerollFloats =
            Native.View<float>(prerollBase, (int)(prerollCount * prerollExpected.ParticleFloats)).ToArray();

        using var prerollFresh = new Simulation(prerollExpected.Seed);
        ApplyConfig(prerollFresh.Ptr, prerollConfigJson, "pre-roll fresh config");
        Native.bfx_seek(prerollFresh.Ptr, 0.5f);
        AssertBufferMatches(prerollFresh.Ptr, prerollCount, prerollExpected.ParticleFloats, prerollFloats, 0f);
        Console.WriteLine("  ok  a pre-rolled track is already running at t=0 and steps on bit-identically");

```

- [ ] **Step 8: Document the fixture**

In `core/README.md`, after the `fixtures/ffi-seek-forward.*` bullet (ends line 113) add:

```markdown
- `fixtures/ffi-preroll.*`: a track whose timeline starts 1 s before t=0
  (`emitterTrack.preroll`, schema version 5) with a StartContinuous and a
  burst at negative times, recorded at `seek(0)`: the pool the pre-roll
  built. Each harness reproduces it, then seeks the same handle on to
  0.5 s and requires that to be bit-identical to a fresh seek, which is
  the forward path leaving the pre-roll.
```

In the regenerate command (line 150) add `--test preroll_fixture` after `--test seek_forward_fixture`.

- [ ] **Step 9: Run the three harnesses**

Run: `core/scripts/smoke-all.sh`
Expected: ends with `all BrightFX checks passed`; the Node, Swift, and C# output each include `ok  a pre-rolled track is already running at t=0 and steps on bit-identically`. (The script also runs the package tests; they are unaffected here.)

- [ ] **Step 10: Commit**

```bash
git add core/fixtures/ffi-preroll.config.json core/fixtures/ffi-preroll.expected.json core/brightfx-core/tests core/harnesses core/README.md
git commit -m "Fixture ffi-preroll: Node, Swift, and C# replay a pre-rolled track

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 4: Generators extend backwards; every preset pre-rolls one lifetime

**Files:**
- Modify: `core/brightfx-tracks/src/sweep.rs` (`sweep_track` at 47-71 and its tests)
- Modify: `core/brightfx-tracks/src/presets.rs` (module doc lines 1-6, `scatter_sweep` at 89-112, `base` at 135-146, the seven preset functions, tests at 570-640)
- Modify: `core/brightfx-tracks/src/fit.rs` (tests after line 185)
- Modify: `core/brightfx-tracks/tests/presets.rs` (`every_preset_loads_unclamped_and_emits` at 119-150, `Census` at 219-224, `census` at 243-290, new test)
- Regenerate: `presets/*.brightfx.json`, `core/fixtures/presets/*.rgba`

**Interfaces:**
- Consumes: `EmitterTrack.preroll`, `brightfx_core::MAX_PREROLL` (Task 1).
- Produces: `pub fn sweep_track(a: (f32, f32), b: (f32, f32), period: f32, duration: f32, preroll: f32) -> EmitterTrack`; `fn scatter_sweep(y_min: f32, y_max: f32, leg: f32, preroll: f32) -> EmitterTrack`; `pub fn preroll_for(config: &ParticleFxConfig) -> f32` in `presets.rs`.

- [ ] **Step 1: Write the failing sweep tests**

In `core/brightfx-tracks/src/sweep.rs` tests, add `0.0` as a fifth argument to every existing `sweep_track(...)` call (seven calls). Rename `starts_continuous_emission_at_zero_and_nothing_else` to `starts_continuous_emission_at_the_track_start_and_nothing_else` and append after it:

```rust
    #[test]
    fn a_preroll_extends_the_sweep_backwards_and_starts_emission_there() {
        let track = sweep_track((0.0, 0.0), (10.0, 0.0), 1.0, 2.0, 1.0);
        assert_eq!(track.preroll, 1.0);
        let times: Vec<f32> = track.keyframes.iter().map(|k| k.time).collect();
        // Enough whole legs to cover the pre-roll, plus one: the path
        // reaches past -preroll rather than parking the emitter there.
        assert_eq!(times, vec![-1.5, -1.0, -0.5, 0.0, 0.5, 1.0, 1.5, 2.0]);
        // The parity alternation is anchored at t=0, so the keyframes at
        // t >= 0 are what they were without a pre-roll.
        let xs: Vec<f32> = track.keyframes.iter().map(|k| k.x).collect();
        assert_eq!(xs, vec![10.0, 0.0, 10.0, 0.0, 10.0, 0.0, 10.0, 0.0]);
        assert_eq!(
            track.triggers,
            vec![EmitterTrigger { time: -1.0, kind: TriggerKind::StartContinuous }]
        );
    }

    #[test]
    fn a_zero_preroll_is_the_old_track() {
        let track = sweep_track((0.0, 0.0), (10.0, 0.0), 1.0, 2.0, 0.0);
        assert_eq!(track.preroll, 0.0);
        assert_eq!(track.keyframes[0].time, 0.0);
        assert_eq!(track.triggers[0].time, 0.0);
    }

    #[test]
    fn a_bad_preroll_is_clamped_like_the_core_clamps_it() {
        assert_eq!(sweep_track((0.0, 0.0), (10.0, 0.0), 1.0, 2.0, -3.0).preroll, 0.0);
        assert_eq!(sweep_track((0.0, 0.0), (10.0, 0.0), 1.0, 2.0, f32::NAN).preroll, 0.0);
        let capped = sweep_track((0.0, 0.0), (10.0, 0.0), 1.0, 2.0, 1e9);
        assert_eq!(capped.preroll, MAX_PREROLL);
        assert!(capped.keyframes[0].time <= -MAX_PREROLL);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --manifest-path core/Cargo.toml -p brightfx-tracks --lib sweep`
Expected: compile error, `sweep_track` takes 4 arguments but 5 were supplied; `MAX_PREROLL` not in scope.

- [ ] **Step 3: Implement `sweep_track`**

In `core/brightfx-tracks/src/sweep.rs` change the import to `use brightfx_core::{MAX_EMITTER_TRACK_DURATION, MAX_PREROLL, PLAYBACK_STEP};` and replace `sweep_track` (lines 36-71, doc comment included) with:

```rust
/// A triangle-wave emitter path: the emitter travels from `a` to `b` and
/// back once per `period` seconds, for `duration` seconds, with a single
/// `StartContinuous` at the start of the timeline. The core spawns within
/// ±2 px of the emitter, so the path is the area the effect covers.
///
/// `preroll` seconds of the same path are laid down before t=0 and the
/// track carries it, so `seek(0)` returns a pool that has already run that
/// long. The parity alternation is anchored at t=0: the keyframes at
/// t >= 0 are the same with or without a pre-roll. `preroll` is clamped as
/// the core clamps it (`0..=MAX_PREROLL`, non-finite is 0).
///
/// `period` is floored at [`MIN_SWEEP_PERIOD`] and `duration` is sanitized
/// into `0..=MAX_EMITTER_TRACK_DURATION` (a non-finite duration becomes 0),
/// so the result always loads unclamped.
pub fn sweep_track(a: (f32, f32), b: (f32, f32), period: f32, duration: f32, preroll: f32) -> EmitterTrack {
    let half = period.max(MIN_SWEEP_PERIOD) / 2.0;
    let duration = if duration.is_finite() {
        duration.clamp(0.0, MAX_EMITTER_TRACK_DURATION)
    } else {
        0.0
    };
    let preroll = sanitize_preroll(preroll);

    // Whole legs before t=0: enough to cover the pre-roll, plus one, so
    // the path reaches past `-preroll` instead of parking the emitter at
    // the first keyframe for the opening fraction of a leg.
    let before = if preroll > 0.0 { leg_count(preroll, half) + 1 } else { 0 };
    let count = before + leg_count(duration, half) + 1;

    let mut keyframes = Vec::with_capacity(count);
    for j in 0..count {
        let i = j as i64 - before as i64;
        // `.min(duration)` because a whole leg can land a fraction past the
        // end (600 s at a 0.3 s leg ends at 600.00006): a keyframe beyond
        // the track's own duration is one the core would never reach.
        let time = (i as f32 * half).min(duration);
        let (x, y) = if i.rem_euclid(2) == 0 { a } else { b };
        keyframes.push(EmitterKeyframe { time, x, y, vx: Some(0.0), vy: Some(0.0) });
    }
    EmitterTrack {
        duration,
        preroll,
        keyframes,
        // `0.0 - preroll`, not `-preroll`: negating a zero gives -0.0, which
        // serializes as `-0.0` and would change every track without a
        // pre-roll (and the cue fixture's bytes).
        triggers: vec![EmitterTrigger { time: 0.0 - preroll, kind: TriggerKind::StartContinuous }],
    }
}

/// `preroll` as the core will read it: `0..=MAX_PREROLL`, 0 if not finite.
pub(crate) fn sanitize_preroll(preroll: f32) -> f32 {
    if preroll.is_finite() {
        preroll.clamp(0.0, MAX_PREROLL)
    } else {
        0.0
    }
}
```


- [ ] **Step 4: Run the sweep tests**

Run: `cargo test --manifest-path core/Cargo.toml -p brightfx-tracks --lib sweep`
Expected: all pass, including the three new ones. The build still fails elsewhere: `presets.rs` calls `sweep_track` with four arguments. Fix those in the next step.

- [ ] **Step 5: Write the failing preset tests**

In `core/brightfx-tracks/src/presets.rs` tests, add `0.0` as a fourth argument to the three existing `scatter_sweep(...)` calls (lines 583, 600, 609). Then append inside `mod tests`:

```rust
    #[test]
    fn a_preroll_extends_the_scatter_backwards_and_starts_emission_there() {
        let track = scatter_sweep(0.0, 100.0, 0.5, 1.2);
        assert_eq!(track.preroll, 1.2);
        let first = track.keyframes[0].time;
        // Two whole legs fit in 1.2 s at 0.5 s, plus one: -1.5, which
        // reaches past -1.2 rather than parking the emitter there.
        assert_eq!(first, -1.5);
        assert!(track.keyframes.iter().any(|k| k.time == 0.0), "a keyframe still lands on t=0");
        assert!(track.keyframes.windows(2).all(|w| w[1].time > w[0].time), "times ascend");
        assert_eq!(track.triggers, vec![EmitterTrigger { time: -1.2, kind: TriggerKind::StartContinuous }]);
        assert!(track.keyframes.iter().all(|k| k.y >= 0.0 && k.y <= 100.0));
    }

    #[test]
    fn every_preset_prerolls_one_full_lifetime() {
        // Every particle alive at t=0 was born within the last `lifetimeMax`
        // steps, so one full lifetime of warm-up is a steady-state pool for
        // any spawn rate, frame size, or cull margin. One rule, no tuning.
        for (name, config) in library() {
            let track = config.emitter_track.as_ref().expect("every preset carries a track");
            assert_eq!(track.preroll, config.lifetime_max / 60.0, "{name}");
            assert_eq!(track.preroll, preroll_for(&config), "{name}");
            assert_eq!(
                track.triggers,
                vec![EmitterTrigger { time: -track.preroll, kind: TriggerKind::StartContinuous }],
                "{name}: the one trigger starts emission at the start of the pre-roll"
            );
            let first = track.keyframes[0].time;
            assert!(first <= -track.preroll, "{name}: first keyframe at {first} does not cover the pre-roll");
        }
    }
```

- [ ] **Step 6: Run to verify it fails**

Run: `cargo test --manifest-path core/Cargo.toml -p brightfx-tracks --lib presets`
Expected: compile errors: `scatter_sweep` takes 3 arguments, `preroll_for` not found, `sweep_track` calls in the presets take 4 arguments.

- [ ] **Step 7: Implement `scatter_sweep`, `preroll_for`, and `with_track`**

In `core/brightfx-tracks/src/presets.rs`:

Replace `scatter_sweep` (lines 89-112, keep its doc comment and add one paragraph to it):

```rust
/// `preroll` seconds of the same wander are laid down before t=0 and the
/// track carries it, so `seek(0)` returns a pool that has already run that
/// long. The hash is keyed on the index from the track's start, so a
/// pre-roll moves the pseudo-random path; nothing depends on where it is.
fn scatter_sweep(y_min: f32, y_max: f32, leg: f32, preroll: f32) -> EmitterTrack {
    let leg = leg.max(PLAYBACK_STEP);
    let preroll = crate::sweep::sanitize_preroll(preroll);
    let before = if preroll > 0.0 { crate::sweep::leg_count(preroll, leg) + 1 } else { 0 };
    let count = before + crate::sweep::leg_count(TRACK_DURATION, leg) + 1;
    let mut keyframes = Vec::with_capacity(count);
    for j in 0..count {
        let i = j as i64 - before as i64;
        let j = j as u32;
        // Off the left and right edges by a margin, so the chords cover the
        // frame's own edges instead of turning back at them.
        let fx = ((j * 3571) % 10007) as f32 / 10007.0;
        let fy = ((j * 7919) % 10009) as f32 / 10009.0;
        keyframes.push(EmitterKeyframe {
            time: (i as f32 * leg).min(TRACK_DURATION),
            x: -SCATTER_MARGIN + fx * (FRAME_W + 2.0 * SCATTER_MARGIN),
            y: y_min + fy * (y_max - y_min),
            vx: Some(0.0),
            vy: Some(0.0),
        });
    }
    EmitterTrack {
        duration: TRACK_DURATION,
        preroll,
        keyframes,
        triggers: vec![EmitterTrigger { time: 0.0 - preroll, kind: TriggerKind::StartContinuous }],
    }
}
```

Replace `base` (lines 133-146) with a track-less version plus two helpers:

```rust
/// `icon` is one short word a picker can show; it is per preset, so the
/// library does not present seven identical tiles. The track is attached
/// last, by `with_track`, once the fields its pre-roll depends on are set.
fn base(id: &str, name: &str, description: &str, icon: &str) -> ParticleFxConfig {
    ParticleFxConfig {
        id: id.into(),
        name: name.into(),
        category: Category::Custom,
        description: description.into(),
        author: Some("Off By 1".into()),
        icon: icon.into(),
        ..ParticleFxConfig::default()
    }
}

/// The pre-roll a preset needs to be at steady state at t=0: one full
/// `lifetimeMax`, in seconds (lifetimes are in 1/60 s steps). Every
/// particle alive at t=0 was born within the last `lifetimeMax` steps, so
/// after that long the pool is what it will be from then on, for any spawn
/// rate, frame size, or cull margin.
pub fn preroll_for(config: &ParticleFxConfig) -> f32 {
    config.lifetime_max / 60.0
}

/// Attaches the track `make` generates for the config's own pre-roll.
/// Called last in every preset, after `lifetime_max` is set.
fn with_track(mut config: ParticleFxConfig, make: impl FnOnce(f32) -> EmitterTrack) -> ParticleFxConfig {
    let preroll = preroll_for(&config);
    config.emitter_track = Some(make(preroll));
    config
}
```

In each of the seven preset functions, remove the track argument from the `base(...)` call and replace the function's final `c` with a `with_track` call. Exact edits:

- `confetti` (line 184): `base("confetti", "Confetti", "Rainbow diamonds drifting down across the whole frame", "confetti")`; last line `with_track(c, |preroll| scatter_sweep(-40.0, FRAME_H, 0.15, preroll))`.
- `fireflies` (line 222): drop `scatter_sweep(0.0, FRAME_H * 0.85, 0.08),` from `base`; last line `with_track(c, |preroll| scatter_sweep(0.0, FRAME_H * 0.85, 0.08, preroll))`.
- `sparkles` (line 268): drop `scatter_sweep(0.0, FRAME_H, 0.08),`; last line `with_track(c, |preroll| scatter_sweep(0.0, FRAME_H, 0.08, preroll))`.
- `sprinkle_rain` (line 339): drop `scatter_sweep(-RAIN_EDGE, -RAIN_EDGE, 0.1),`; last line `with_track(c, |preroll| scatter_sweep(-RAIN_EDGE, -RAIN_EDGE, 0.1, preroll))`.
- `frosting_rain` (line 412): drop `sweep_track((0.0, -RAIN_EDGE), (FRAME_W, -RAIN_EDGE), 0.5, TRACK_DURATION),`; last line `with_track(c, |preroll| sweep_track((0.0, -RAIN_EDGE), (FRAME_W, -RAIN_EDGE), 0.5, TRACK_DURATION, preroll))`.
- `bubbles` (line 452): drop `sweep_track((0.0, FRAME_H + 20.0), (FRAME_W, FRAME_H + 20.0), 0.6, TRACK_DURATION),`; last line `with_track(c, |preroll| sweep_track((0.0, FRAME_H + 20.0), (FRAME_W, FRAME_H + 20.0), 0.6, TRACK_DURATION, preroll))`.
- `flight_arc` (line 512): drop `sweep_track((-100.0, FRAME_H * 0.43), (-100.0, FRAME_H * 0.6), 3.0, TRACK_DURATION),`; last line `with_track(c, |preroll| sweep_track((-100.0, FRAME_H * 0.43), (-100.0, FRAME_H * 0.6), 3.0, TRACK_DURATION, preroll))`.

Each function's body between stays as it is (`let mut c = base(...); c.emitter = ...; ...`). If a preset's body reads `c.emitter_track` before the end, stop: none does today, and `with_track` must stay last.

Make `leg_count` reachable: it is already `pub(crate)` in `sweep.rs`.

Update the module doc (lines 4-6) from `carries a `MAX_EMITTER_TRACK_DURATION` track with one `StartContinuous` at 0, so `amount` and `window` in the composition, not the track, decide when it shows.` to:

```
//! 1920×1080 pixel frame and carries a `MAX_EMITTER_TRACK_DURATION` track
//! with one `StartContinuous` at the start of its pre-roll, so `amount`
//! and `window` in the composition, not the track, decide when it shows.
//! The pre-roll is one full `lifetimeMax` (`preroll_for`), so the first
//! frame a composition shows is already the steady state rather than an
//! empty frame filling up.
```

- [ ] **Step 8: Run the tracks unit tests**

Run: `cargo test --manifest-path core/Cargo.toml -p brightfx-tracks --lib`
Expected: all pass, including `every_preset_prerolls_one_full_lifetime` and `a_preroll_extends_the_scatter_backwards_and_starts_emission_there`.

- [ ] **Step 9: Pin `fit_track` to pass pre-roll through**

In `core/brightfx-tracks/src/fit.rs` tests append:

```rust
    #[test]
    fn a_preroll_and_negative_times_pass_through_the_fit_untouched() {
        let mut config = authored();
        let track = config.emitter_track.as_mut().unwrap();
        track.preroll = 1.5;
        track.keyframes.insert(0, EmitterKeyframe { time: -1.5, x: -192.0, y: 0.0, vx: None, vy: None });
        track.triggers.push(EmitterTrigger { time: -1.5, kind: TriggerKind::StartContinuous });

        let fitted = fit_track(config, (1920.0, 1080.0), (3840.0, 2160.0)).unwrap();
        let track = fitted.emitter_track.unwrap();
        assert_eq!(track.preroll, 1.5);
        assert_eq!(track.keyframes[0].time, -1.5);
        assert_eq!(track.keyframes[0].x, -384.0, "x scales, time does not");
        assert_eq!(track.triggers[0].time, -1.5);
    }
```

Add `EmitterTrigger, TriggerKind` to the test module's `use brightfx_core::schema::{...}` import if they are not there.

Run: `cargo test --manifest-path core/Cargo.toml -p brightfx-tracks --lib fit`
Expected: pass.

- [ ] **Step 10: Update the preset suite and write the acceptance test**

In `core/brightfx-tracks/tests/presets.rs`:

In `every_preset_loads_unclamped_and_emits`, replace the trigger assertions (the block from `// One trigger, at 0, and nothing after it` through `assert_eq!(triggers[0]["time"], 0.0, ...)`) with:

```rust
        // One trigger, at the start of the pre-roll, and nothing after it:
        // a second trigger (a stop, a burst) would let the track decide
        // when the preset shows, which is the composition's job.
        let preroll = config["emitterTrack"]["preroll"].as_f64().unwrap() as f32;
        assert_eq!(preroll, config["lifetimeMax"].as_f64().unwrap() as f32 / 60.0, "{name}: pre-roll is one lifetime");
        let triggers = config["emitterTrack"]["triggers"].as_array().unwrap();
        assert_eq!(triggers.len(), 1, "{name}: expected exactly one trigger, got {triggers:?}");
        assert_eq!(triggers[0]["kind"], "startContinuous", "{name}: must start continuous");
        assert_eq!(triggers[0]["time"].as_f64().unwrap() as f32, -preroll, "{name}: must start at -preroll");
```

Add a `pool_min` to `Census`:

```rust
struct Census {
    /// `(x, y, alpha)` of every particle that died inside the frame while
    /// still visible.
    visible_deaths: Vec<(f32, f32, f32)>,
    deaths: usize,
    pool_peak: usize,
    pool_min: usize,
}
```

In `census`, initialize `pool_min: prev.len()` alongside `pool_peak: prev.len()`, and after `census.pool_peak = census.pool_peak.max(next.len());` add `census.pool_min = census.pool_min.min(next.len());`.

Append the acceptance test after `at_steady_state_every_preset_leaves_pool_headroom`:

```rust
/// #19: a composition that shows a preset in its first seconds must not
/// get an empty frame filling up. With `preroll` of one lifetime the pool
/// at t=0 is a sample of the steady state, so its size sits inside the
/// range the census sees from 9 s on. The band is widened by a tenth at
/// each end: the count fluctuates with the emitter's path, and 180 steps
/// is a short look at it. A miss by more than that means the pre-roll is
/// wrong, not the band.
#[test]
fn at_time_zero_every_preset_is_already_at_steady_state() {
    for (name, config) in library() {
        for (w, h) in [(FRAME_W, FRAME_H), (FRAME_H, FRAME_W)] {
            for bounded in [false, true] {
                let fitted = fit_track(config.clone(), (FRAME_W, FRAME_H), (w, h)).unwrap();
                let steady = census(fitted.clone(), w, h, bounded);
                let mut sim = Simulation::new(fitted, 7);
                if bounded {
                    sim.set_bounds(w, h);
                }
                sim.seek(0.0);
                let at_zero = sim.particle_count() as usize;
                let low = steady.pool_min * 9 / 10;
                let high = steady.pool_peak * 11 / 10;
                assert!(
                    (low..=high).contains(&at_zero),
                    "{name} {w}x{h} bounded={bounded}: {at_zero} particles at t=0, steady state holds {}..={}",
                    steady.pool_min, steady.pool_peak
                );
            }
        }
    }
}
```

- [ ] **Step 11: Regenerate the presets and the golden frames, then run the suite**

Run: `(cd core && cargo run -p brightfx-tracks --example gen_presets -- ../presets) && BRIGHTFX_REGENERATE=1 cargo test --manifest-path core/Cargo.toml -p brightfx-tracks --test presets golden_frames_match`
Expected: seven presets rewritten; `regenerated .../core/fixtures/presets/<name>.rgba` printed seven times.

Run: `cargo test --manifest-path core/Cargo.toml -p brightfx-tracks --test presets`
Expected: all pass, including `at_time_zero_every_preset_is_already_at_steady_state`. If a preset misses the band, print both numbers and look at the census before touching the band: the pre-roll rule is the deliverable, the band is only its check.

Check the preset diff carries what the spec says and nothing else:

Run: `for f in presets/*.brightfx.json; do python3 -c "import json,sys; c=json.load(open('$f')); t=c['emitterTrack']; print('$f', c['schemaVersion'], t['preroll'], c['lifetimeMax']/60, t['keyframes'][0]['time'], t['triggers'])"; done`
Expected: every line shows version 5, `preroll` equal to `lifetimeMax/60` (2.0, 2.0, 1.5, 5.0, 4.6666665, 2.0, 5.0 in library order), a first keyframe time at or below `-preroll`, and one `startContinuous` trigger at `-preroll`.

- [ ] **Step 12: Look at the golden frames**

Run (needs ImageMagick; skip the viewing if `magick` is missing but still do the pixel count):

```bash
mkdir -p "$TMPDIR/goldens" && for f in core/fixtures/presets/*.rgba; do n=$(basename "$f" .rgba); magick -size 240x135 -depth 8 "rgba:$f" "$TMPDIR/goldens/$n.png"; done; ls "$TMPDIR/goldens"
```

Open `frosting-rain.png` and `sprinkle-rain.png`: at t=3 s the drops now reach the bottom of the frame instead of stopping partway. Compare with the old frames: `git stash -q -- core/fixtures/presets && magick -size 240x135 -depth 8 rgba:core/fixtures/presets/frosting-rain.rgba "$TMPDIR/goldens/frosting-rain-before.png"; git stash pop -q`.

- [ ] **Step 13: Run the whole Rust suite and the cue fixture guard**

Run: `cargo test --manifest-path core/Cargo.toml && git status --short core/fixtures presets`
Expected: all pass; `git status` lists the seven `.rgba` files and the seven preset JSON files, and nothing under `core/fixtures/ffi-*` or `core/fixtures/tracks-cues.*`.

- [ ] **Step 14: Commit**

```bash
git add core/brightfx-tracks core/fixtures/presets presets
git commit -m "Presets pre-roll one lifetime; sweep and scatter tracks extend before t=0

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 5: Types and docs

**Files:**
- Modify: `packages/brightfx-js/src/index.ts:49-53`
- Modify: `core/README.md:38` and `:48-51`
- Modify: `core/brightfx-core/README.md:27-38` and after the Culling section (line 152)
- Modify: `packages/brightfx-remotion/README.md:104-108`

**Interfaces:**
- Consumes: everything above. No code behavior changes.

- [ ] **Step 1: The JS type**

In `packages/brightfx-js/src/index.ts` replace the `EmitterTrack` interface (lines 49-53) with:

```ts
export interface EmitterTrack {
  duration: number;
  /** Schema version 5+. Seconds the timeline runs before t=0, so `seek(0)`
   *  returns the pool after that much playback instead of an empty one.
   *  Keyframes and triggers may be authored at negative times down to
   *  `-preroll`. Omitted means 0. The pre-roll is simulated, never
   *  rendered: `seek` still clamps its time at 0. */
  preroll?: number;
  keyframes: EmitterKeyframe[];
  triggers: EmitterTrigger[];
}
```

Run: `(cd packages && npm run build -w brightfx-js)`
Expected: compiles. (`sync-wasm` needs `core/brightfx-wasm/pkg-web`, which Task 3's smoke run built.)

- [ ] **Step 2: The boundary contract**

In `core/README.md` line 38 change the comment to:
`seek(time)                           // baked mode; forward seeks step from the last seek; runs emitterTrack.preroll before t=0`

In the `set_config` note (lines 48-51) change `next `seek` replays from t=0. Re-seed with `seek(0)`.` to `next `seek` replays from the start of the track (its `preroll` before t=0). Re-seed with `seek(0)`.`

- [ ] **Step 3: The core crate README**

In `core/brightfx-core/README.md`, in the `seek(time)` bullet (lines 27-38) change `resets and replays from t=0` to `resets and replays from the start of the track`, and append this paragraph to the bullet after `No-op without a track.`:

```markdown
    With `emitter_track.preroll` (schema version 5) the track's timeline
    starts that many seconds before t=0, and a replay starts there, so
    `seek(0)` returns the pool after that much playback. Keyframes and
    triggers may be authored at negative times down to `-preroll`; a
    trigger earlier than that fires at the start. `time` still clamps at
    0: the pre-roll is simulated, never rendered. Capped at 60 s
    (`MAX_PREROLL`).
```

After the Culling section, add:

```markdown
## Pre-roll

A baked effect's pool is empty at t=0 unless its track says otherwise. A
rain takes a few seconds to fill a frame, so a composition that shows it
in its first seconds would get an empty lower frame filling up (#19).
`emitterTrack.preroll` (schema version 5) extends the timeline before t=0
by that many seconds. The grid stays anchored at t=0 and the pre-roll adds
`grid_step(preroll)` whole steps in front of it, so a pre-rolled track is
bit-identical to the same track with every time moved later by the
pre-roll (`a_prerolled_track_is_the_same_track_started_earlier`). The
presets each pre-roll one full `lifetimeMax`, which is a steady-state
pool by construction: every particle alive at t=0 was born within the last
`lifetimeMax` steps.
```

- [ ] **Step 4: The Remotion README**

In `packages/brightfx-remotion/README.md`, after the paragraph ending `replay the track's last state frozen.` (line 108) add:

```markdown
`window` and `useCurrentFrame` are composition time. An effect whose track
carries a `preroll` (every shipped preset does) has already run that long
when the composition starts, so its first frame is the steady state; the
pre-roll itself is simulated by the core, never seeked to or drawn.
```

- [ ] **Step 5: Run everything once more and commit**

Run: `core/scripts/smoke-all.sh`
Expected: ends with `all BrightFX checks passed`.

```bash
git add packages/brightfx-js/src/index.ts core/README.md core/brightfx-core/README.md packages/brightfx-remotion/README.md
git commit -m "Docs and JS type for emitterTrack.preroll

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 6: Open the pull request

- [ ] **Step 1: Push and open the PR**

```bash
git push -u origin feat/emitter-track-preroll
gh pr create --title "Emitter track pre-roll: presets start at steady state" --body "$(cat <<'PR'
Closes #19.

A baked effect started at t=0 with an empty pool, so a rain shown in a composition's first seconds had an empty lower frame filling up. The effect file can now say its timeline began earlier.

- Schema 5: `emitterTrack.preroll` (seconds, clamped to [0, 60], omitted when 0); keyframe and trigger times may be negative.
- Core: `seek` runs the pre-roll before t=0. The 1/60 s grid stays anchored at t=0 and the pre-roll adds whole steps in front, so a pre-rolled track is bit-identical to the same track started later. `seek` still clamps at 0; the pre-roll is never rendered.
- Generators: `sweep_track` and `scatter_sweep` extend backwards and start emission at `-preroll`. Every preset pre-rolls one full `lifetimeMax`, a steady-state pool by construction.
- Fixtures: new `ffi-preroll` replayed by Node, Swift, and C#. Every existing fixture is bit-identical. All seven golden frames regenerated.
- Acceptance test: each preset's pool at `seek(0)` sits inside its steady-state census band.

Spec: `docs/superpowers/specs/2026-10-08-emitter-track-preroll-design.md`.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
PR
)"
```

- [ ] **Step 2: Watch CI**

Run: `gh pr checks --watch`
Expected: the macOS smoke job and the Windows job both pass.
