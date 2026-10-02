# Cull Particles Outside the Viewport Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** An opt-in `cullMargin` config field removes a particle once its center is more than the margin outside host-supplied bounds, so pools match what is on screen (issue #17).

**Architecture:** `Simulation` gains `bounds: Option<(f32, f32)>` (logical px) set by `set_bounds`, and its `step` drops a particle when `config.cull_margin` and `bounds` are both set and the particle is outside `[0,w]x[0,h]` by more than the margin. The ABI's `set_viewport` feeds the bounds (`width / scale`, `height / scale`) so frame mode is automatic; sprite-mode hosts call a new `setBounds` through the WASM and JS wrappers, and the Remotion wrapper's sprite path passes its composition size.

**Tech Stack:** Rust workspace in `core/` (serde, schemars, wasm-bindgen), TypeScript wrappers in `packages/`, node:test, Remotion.

Spec: `docs/superpowers/specs/2026-10-02-cull-particles-outside-viewport-design.md`

## Global Constraints

- Work in the worktree `/Users/jason/Projects/ob1/open source/BrightFX-17-cull` on branch `feat/17-cull-viewport`. Every command below starts with `cd` into it, because the shell cwd resets between calls.
- `cullMargin` is `Option<f32>`, serialized `cullMargin`, in logical px, `#[serde(default)]`, clamped to `0..=10000`. `None` means culling off.
- `SCHEMA_VERSION` goes 3 -> 4. `MIN_SCHEMA_VERSION` stays 1; older configs load and are relabelled by `migrate`.
- Culling uses the particle's center only: out when `x < -m || x > w + m || y < -m || y > h + m`. Particle size is not added to the margin.
- With no bounds set, or `cullMargin` of `None`, nothing is culled (today's behavior).
- `Simulation::set_bounds` takes logical px; a non-finite or non-positive dimension clears the bounds. A change of bounds calls `leave_baked()`; setting identical bounds does not.
- Preset retuning is out of scope. Presets are only regenerated for the schema version and the new `cullMargin: null` field.
- The C FFI (`brightfx-ffi`) is not changed.
- Commits: end the message with `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`. Never add a Claude session link anywhere.
- `BRIGHTFX_REGENERATE=1` is used only where a step says so, and the diff is reviewed before committing.

## File Structure

| File | Change |
|---|---|
| `core/brightfx-core/src/schema.rs` | `cull_margin` field, clamp, `SCHEMA_VERSION` 4, tests |
| `core/brightfx-core/src/abi.rs` | `migrate` comment, `set_bounds`, `set_viewport` feeds bounds, version tests |
| `core/brightfx-core/src/simulation.rs` | `bounds`, `set_bounds`, `bounds()`, cull in `step`, tests |
| `core/brightfx-wasm/src/lib.rs` | `setBounds` binding |
| `core/brightfx-tracks/src/{presets,fit,json}.rs`, `core/brightfx-core/{tests/golden.rs,examples/*.rs}` | `cull_margin: None` in literals; version-test constants |
| `core/harnesses/node/smoke.mjs` | future-version constant 4 -> 5 |
| `presets/*.brightfx.json` | regenerated (`schemaVersion` 4, `cullMargin: null`) |
| `core/brightfx-core/README.md` | document `cullMargin` |
| `packages/brightfx-js/src/index.ts`, `test/wrapper.test.mjs` | `cullMargin` type, `setBounds`, tests |
| `packages/brightfx-remotion/src/index.tsx`, `test/src/Root.tsx`, `test/fixtures/probe-effects.json`, `test/still.test.mjs` | sprite mode passes bounds; probe test |

---

### Task 1: `cullMargin` config field and schema version 4

**Files:**
- Modify: `core/brightfx-core/src/schema.rs` (struct ~line 198-250, `clamp_to_bounds` ~line 260-335, `Default` ~line 340, tests)
- Modify: `core/brightfx-core/src/abi.rs` (`migrate` ~line 366; tests ~lines 512-590)
- Modify (add `cull_margin: None,`): `core/brightfx-tracks/src/presets.rs:123`, `core/brightfx-tracks/src/fit.rs:42`, `core/brightfx-core/tests/golden.rs:8`, `core/brightfx-core/examples/frame_dump.rs:23`, `core/brightfx-core/examples/render_bench.rs:24`, `core/brightfx-core/src/simulation.rs:710`, `core/brightfx-core/src/schema.rs` (`example_config`), `core/brightfx-core/src/abi.rs` test literals
- Modify: `core/brightfx-tracks/src/json.rs` (~line 95), `core/harnesses/node/smoke.mjs` (~lines 290-294)
- Regenerate: `presets/*.brightfx.json`

**Interfaces:**
- Produces: `ParticleFxConfig.cull_margin: Option<f32>`; `SCHEMA_VERSION == 4`. Task 2 reads `config.cull_margin`.

- [ ] **Step 1: Write the failing schema tests**

In the `tests` module of `core/brightfx-core/src/schema.rs`, add:

```rust
    #[test]
    fn cull_margin_defaults_to_off_when_absent() {
        let mut value = serde_json::to_value(ParticleFxConfig::default()).unwrap();
        value.as_object_mut().unwrap().remove("cullMargin");
        let config: ParticleFxConfig = serde_json::from_value(value).unwrap();
        assert_eq!(config.cull_margin, None);
    }

    #[test]
    fn cull_margin_round_trips_as_camel_case() {
        let mut config = ParticleFxConfig::default();
        config.cull_margin = Some(40.0);
        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(json["cullMargin"], 40.0);
        let back: ParticleFxConfig = serde_json::from_value(json).unwrap();
        assert_eq!(back.cull_margin, Some(40.0));
    }

    #[test]
    fn a_negative_cull_margin_is_raised_to_zero_and_reported() {
        let mut config = ParticleFxConfig::default();
        config.cull_margin = Some(-5.0);
        let changed = config.clamp_to_bounds();
        assert_eq!(config.cull_margin, Some(0.0));
        assert!(changed.contains(&"cullMargin"));
    }

    #[test]
    fn an_absurd_cull_margin_is_capped_and_an_absent_one_is_left_alone() {
        let mut config = ParticleFxConfig::default();
        config.cull_margin = Some(1.0e9);
        assert!(config.clamp_to_bounds().contains(&"cullMargin"));
        assert_eq!(config.cull_margin, Some(10_000.0));

        let mut config = ParticleFxConfig::default();
        assert!(!config.clamp_to_bounds().contains(&"cullMargin"));
        assert_eq!(config.cull_margin, None);
    }
```

In `core/brightfx-core/src/abi.rs` tests, update the version tests for 4 (do this now so they fail until the bump):

- `the_version_is_checked_before_the_body_is_deserialized`: change `{"schemaVersion": 4, ...}` to `{"schemaVersion": 5, ...}`.
- `older_versions_still_load_and_read_back_as_the_current_version`: change `for version in [1u64, 2]` to `for version in [1u64, 2, 3]`, update its comment to "v2, v3 and v4 only added vocabulary and defaulted fields", and also `.remove("cullMargin")` next to the `spinDirection` remove, then add `assert_eq!(config.cull_margin, None, "v{version}");` after the `spin_direction` assert.
- `the_current_version_is_3_and_loads`: rename to `the_current_version_is_4_and_loads`, with `assert_eq!(SCHEMA_VERSION, 4);` and `.schema_version, 4`.
- `versions_outside_the_supported_range_are_rejected_clearly`: `for version in [0u64, 5]`, and `message.contains("1-4")`.

In `core/brightfx-tracks/src/json.rs` (~line 95) change `json!(4)` to `json!(5)` and the expected text to `unsupported schemaVersion 5`. In `core/harnesses/node/smoke.mjs` (~lines 290-294) change `schemaVersion: 4` to `schemaVersion: 5` and the regex to `/unsupported schemaVersion 5/`.

- [ ] **Step 2: Run to verify it fails**

Run: `cd "/Users/jason/Projects/ob1/open source/BrightFX-17-cull/core" && cargo test -p brightfx-core --lib schema 2>&1 | tail -20`
Expected: compile error, `no field cull_margin on type ParticleFxConfig`.

- [ ] **Step 3: Add the field, the clamp and the version**

In `schema.rs`, replace the `SCHEMA_VERSION` doc and value:

```rust
/// The config schema version this build writes. Version 2 added the
/// `random-palette` colour mode; version 3 the `capsule` shape and
/// `spinDirection`; version 4 `cullMargin`. Nothing was renamed or removed,
/// so an older config is a valid current body once relabelled
/// (`spinDirection` and `cullMargin` default).
pub const SCHEMA_VERSION: u32 = 4;
```

Add the largest margin next to the other constants near the top of the file:

```rust
/// Upper bound on `cullMargin`, in logical px. Far past any real frame, so
/// it only stops an absurd value from overflowing the comparison.
const MAX_CULL_MARGIN: f32 = 10_000.0;
```

Add the field to `ParticleFxConfig`, directly after `lifetime_max`:

```rust
    /// Optional; schema version 3 and earlier omit it. Logical px. When set
    /// and the host has given the simulation bounds, a particle is removed
    /// once its center is more than this far outside them. `None` never
    /// culls.
    #[serde(default)]
    pub cull_margin: Option<f32>,
```

In `clamp_to_bounds`, after the `if let Some(track) = self.emitter_track.as_mut()` block add:

```rust
        if let Some(margin) = self.cull_margin.as_mut() {
            clamp(margin, 0.0, MAX_CULL_MARGIN, "cullMargin");
        }
```

In `impl Default`, add `cull_margin: None,` after `lifetime_max: 60.0,`.

In `abi.rs`, update `migrate`'s comment to `// 1 -> 2 -> 3 -> 4 only added vocabulary (and defaulted fields): the body is already valid.`

- [ ] **Step 4: Fix the remaining struct literals**

Run: `cd "/Users/jason/Projects/ob1/open source/BrightFX-17-cull/core" && cargo build --workspace --all-targets 2>&1 | grep -A4 "missing field"`

For every reported literal, add `cull_margin: None,` right after its `lifetime_max: ...,` line (the sites are listed under Files). Repeat the command until it prints nothing.

- [ ] **Step 5: Run the schema and ABI tests**

Run: `cd "/Users/jason/Projects/ob1/open source/BrightFX-17-cull/core" && cargo test -p brightfx-core 2>&1 | tail -20`
Expected: all pass.

- [ ] **Step 6: Regenerate the presets and check the diff**

Run:
```bash
cd "/Users/jason/Projects/ob1/open source/BrightFX-17-cull/core" && cargo run -q -p brightfx-tracks --example gen_presets && cd .. && git diff --stat presets && git diff presets | grep '^[+-] ' | sort | uniq -c
```
Expected: 7 files changed; the only changed lines are `"schemaVersion": 3` -> `4` and one added `"cullMargin": null,` per file (7 each).

- [ ] **Step 7: Run the whole Rust suite**

Run: `cd "/Users/jason/Projects/ob1/open source/BrightFX-17-cull/core" && cargo test --workspace 2>&1 | tail -30`
Expected: all pass. If a fixture-staleness test fails, read what differs. Regenerate with `BRIGHTFX_REGENERATE=1` only if the difference is purely the schema version or the new `cullMargin` field, then re-run without it.

- [ ] **Step 8: Commit**

```bash
cd "/Users/jason/Projects/ob1/open source/BrightFX-17-cull" && git add -A core presets && git commit -m "Config: add cullMargin and bump the schema to version 4

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Simulation bounds and culling

**Files:**
- Modify: `core/brightfx-core/src/simulation.rs` (struct ~line 124-147, `new` ~line 150, `step` ~line 380-450, tests module)

**Interfaces:**
- Consumes: `ParticleFxConfig.cull_margin: Option<f32>` (Task 1).
- Produces: `Simulation::set_bounds(&mut self, width: f32, height: f32)`, `Simulation::bounds(&self) -> Option<(f32, f32)>`. Task 3 calls both.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module of `simulation.rs`:

```rust
    /// One particle moving +x at 10 px per step from about (50, 50): spawned
    /// by the first advance, after which the emitter goes quiet.
    fn one_runner(margin: Option<f32>) -> Simulation {
        let mut config = base_config();
        config.emitter.spawn_rate_while_active = 1.0;
        config.initial_speed_min = 10.0;
        config.initial_speed_max = 10.0;
        config.cull_margin = margin;
        let mut sim = Simulation::new(config, 7);
        sim.set_emitter(50.0, 50.0, 0.0, 0.0, true);
        sim.advance(TICK);
        sim.set_emitter(50.0, 50.0, 0.0, 0.0, false);
        assert_eq!(sim.particle_count(), 1, "test is vacuous without the runner");
        sim
    }

    #[test]
    fn a_particle_past_the_margin_is_culled_and_one_inside_it_is_kept() {
        let mut sim = one_runner(Some(5.0));
        sim.set_bounds(100.0, 100.0);
        // x is 60 +/- 2 after the spawn step; four more steps put it at
        // 100 +/- 2, inside the 105 edge.
        for _ in 0..4 {
            sim.advance(TICK);
        }
        assert_eq!(sim.particle_count(), 1, "culled while inside the margin");
        // One more step puts it at 110 +/- 2, past 105.
        sim.advance(TICK);
        assert_eq!(sim.particle_count(), 0, "kept past the margin");
    }

    #[test]
    fn every_edge_culls() {
        // Moves the runner along each axis by rotating the emission angle.
        for (angle, label) in [(0.0, "right"), (90.0, "down"), (180.0, "left"), (270.0, "up")] {
            let mut config = base_config();
            config.emitter.spawn_rate_while_active = 1.0;
            config.emitter.emission_angle = angle;
            config.initial_speed_min = 10.0;
            config.initial_speed_max = 10.0;
            config.cull_margin = Some(0.0);
            let mut sim = Simulation::new(config, 7);
            sim.set_bounds(100.0, 100.0);
            sim.set_emitter(50.0, 50.0, 0.0, 0.0, true);
            sim.advance(TICK);
            sim.set_emitter(50.0, 50.0, 0.0, 0.0, false);
            assert_eq!(sim.particle_count(), 1, "{label}: vacuous");
            for _ in 0..8 {
                sim.advance(TICK);
            }
            assert_eq!(sim.particle_count(), 0, "{label}: never culled");
        }
    }

    #[test]
    fn nothing_is_culled_without_bounds() {
        let mut sim = one_runner(Some(0.0));
        for _ in 0..20 {
            sim.advance(TICK);
        }
        assert_eq!(sim.particle_count(), 1);
    }

    #[test]
    fn nothing_is_culled_without_a_margin() {
        let mut sim = one_runner(None);
        sim.set_bounds(100.0, 100.0);
        for _ in 0..20 {
            sim.advance(TICK);
        }
        assert_eq!(sim.particle_count(), 1);
    }

    #[test]
    fn invalid_bounds_clear_the_bounds() {
        let mut sim = Simulation::new(base_config(), 1);
        sim.set_bounds(100.0, 50.0);
        assert_eq!(sim.bounds(), Some((100.0, 50.0)));
        for (w, h) in [(0.0, 50.0), (100.0, -1.0), (f32::NAN, 50.0), (100.0, f32::INFINITY)] {
            sim.set_bounds(100.0, 50.0);
            sim.set_bounds(w, h);
            assert_eq!(sim.bounds(), None, "({w}, {h})");
        }
    }

    #[test]
    fn changing_the_bounds_clears_the_baked_position_but_repeating_them_does_not() {
        let mut sim = Simulation::new(track_config(), 5);
        sim.seek(0.5);
        assert!(sim.baked.is_some());
        sim.set_bounds(100.0, 100.0);
        assert!(sim.baked.is_none(), "a new size left the cursor in place");

        sim.seek(0.5);
        assert!(sim.baked.is_some());
        sim.set_bounds(100.0, 100.0);
        assert!(sim.baked.is_some(), "identical bounds cleared the cursor");
    }

    /// The seek fixture's burst at 0.5 s: 12 radial particles from about
    /// (30, 0), culled at the top edge when the bounds are set.
    fn culling_track_config() -> ParticleFxConfig {
        let mut config = track_config();
        config.emitter.spawn_burst_size = 12;
        config.initial_speed_min = 5.0;
        config.initial_speed_max = 5.0;
        config.cull_margin = Some(0.0);
        config
    }

    fn xy(sim: &Simulation) -> Vec<(u32, u32)> {
        sim.buffer().iter().map(|p| (p.x.to_bits(), p.y.to_bits())).collect()
    }

    #[test]
    fn culling_removes_some_but_not_all_of_a_burst() {
        let mut open = Simulation::new(culling_track_config(), 5);
        open.seek(0.55);
        let mut culled = Simulation::new(culling_track_config(), 5);
        culled.set_bounds(200.0, 200.0);
        culled.seek(0.55);

        assert_eq!(open.particle_count(), 12, "test is vacuous without the burst");
        assert!(culled.particle_count() > 0, "everything was culled");
        assert!(culled.particle_count() < open.particle_count(), "nothing was culled");
        assert!(culled.buffer().iter().all(|p| p.y >= 0.0 && p.x >= 0.0 && p.x <= 200.0));
    }

    #[test]
    fn culling_is_deterministic_under_seek() {
        let make = || {
            let mut sim = Simulation::new(culling_track_config(), 5);
            sim.set_bounds(200.0, 200.0);
            sim
        };
        let mut fresh = make();
        fresh.seek(0.55);

        // Backward: overshoot, then seek back, which replays from zero.
        let mut rewound = make();
        rewound.seek(0.75);
        rewound.seek(0.55);

        // Forward: step on from an earlier seek.
        let mut stepped = make();
        stepped.seek(0.5);
        stepped.seek(0.55);

        assert!(fresh.particle_count() > 0, "test is vacuous with no particles");
        assert_eq!(xy(&fresh), xy(&rewound));
        assert_eq!(xy(&fresh), xy(&stepped));
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd "/Users/jason/Projects/ob1/open source/BrightFX-17-cull/core" && cargo test -p brightfx-core --lib simulation 2>&1 | tail -20`
Expected: compile error, `no method named set_bounds found for struct Simulation`.

- [ ] **Step 3: Implement**

In `struct Simulation`, add after `baked`:

```rust
    /// Logical-pixel size of the rectangle `[0, w] x [0, h]` that
    /// `config.cull_margin` is measured from. `None` until the host sets
    /// it, and while it is `None` nothing is culled. Part of the state a
    /// replay depends on, so changing it clears `baked` (see `set_bounds`).
    bounds: Option<(f32, f32)>,
```

In `Simulation::new`, add `bounds: None,` after `baked: None,`.

Add these methods after `set_config`:

```rust
    /// Sets the logical-pixel rectangle `[0, width] x [0, height]` that
    /// `cullMargin` is measured from. A non-finite or non-positive
    /// dimension clears it, and with no bounds nothing is culled.
    ///
    /// The ABI's `set_viewport` calls this itself. A host with no viewport
    /// (sprite mode) calls it with its container size.
    ///
    /// A change clears the baked position: culling makes the pool depend
    /// on the bounds, so the next `seek` must replay from zero rather than
    /// step forward from a pool built for another size. Setting the bounds
    /// the simulation already has changes nothing and leaves it alone.
    pub fn set_bounds(&mut self, width: f32, height: f32) {
        let valid = width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0;
        let bounds = valid.then_some((width, height));
        if bounds != self.bounds {
            self.bounds = bounds;
            self.leave_baked();
        }
    }

    /// The bounds `set_bounds` last accepted, in logical px.
    pub fn bounds(&self) -> Option<(f32, f32)> {
        self.bounds
    }
```

In `step`, just before `self.pool.retain_mut(|p| {` add:

```rust
        // Only culls when the author asked for it and the host said how big
        // the frame is.
        let cull = match (self.config.cull_margin, self.bounds) {
            (Some(margin), Some((w, h))) => Some((margin, w, h)),
            _ => None,
        };
```

and inside the closure, directly after `p.y += p.vy * fe;` add:

```rust
            if let Some((margin, w, h)) = cull {
                if p.x < -margin || p.x > w + margin || p.y < -margin || p.y > h + margin {
                    return false;
                }
            }
```

- [ ] **Step 4: Run to verify it passes**

Run: `cd "/Users/jason/Projects/ob1/open source/BrightFX-17-cull/core" && cargo test -p brightfx-core 2>&1 | tail -20`
Expected: all pass, including the existing seek and golden suites (nothing culls without a margin).

If `culling_removes_some_but_not_all_of_a_burst` reports "everything was culled" or "nothing was culled", the burst geometry differs from this plan's arithmetic. Print `culled.buffer()` and fix the bounds, not the cull code.

- [ ] **Step 5: Commit**

```bash
cd "/Users/jason/Projects/ob1/open source/BrightFX-17-cull" && git add core/brightfx-core/src/simulation.rs && git commit -m "Core: cull particles that leave the host-set bounds

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 3: ABI and WASM bounds

**Files:**
- Modify: `core/brightfx-core/src/abi.rs` (`set_viewport` ~line 301; new `set_bounds`; tests beside `set_viewport_returns_an_ok_envelope_with_clamped_fields`, ~line 827)
- Modify: `core/brightfx-wasm/src/lib.rs` (after `set_viewport`, ~line 85)

**Interfaces:**
- Consumes: `Simulation::set_bounds`, `Simulation::bounds` (Task 2).
- Produces: `AbiSimulation::set_bounds(&mut self, width: f32, height: f32)`; wasm export `setBounds(width: number, height: number): void`. Task 4 calls `setBounds`.

- [ ] **Step 1: Write the failing tests**

In `abi.rs`, in the same test module as `a_zero_viewport_is_rejected_with_a_message`:

```rust
        #[test]
        fn set_viewport_feeds_the_bounds_in_logical_units() {
            let mut sim = AbiSimulation::new(1);
            sim.set_viewport(200, 100, 2.0);
            assert_eq!(sim.sim.bounds(), Some((100.0, 50.0)));
        }

        #[test]
        fn the_bounds_follow_the_clamped_scale() {
            let mut sim = AbiSimulation::new(1);
            // 100.0 clamps to MAX_SCALE (8.0).
            sim.set_viewport(80, 40, 100.0);
            assert_eq!(sim.sim.bounds(), Some((10.0, 5.0)));
        }

        #[test]
        fn a_rejected_viewport_leaves_the_bounds_alone() {
            let mut sim = AbiSimulation::new(1);
            sim.set_viewport(80, 40, 1.0);
            sim.set_viewport(0, 40, 1.0);
            assert_eq!(sim.sim.bounds(), Some((80.0, 40.0)));
        }

        #[test]
        fn set_bounds_sets_them_without_a_viewport() {
            let mut sim = AbiSimulation::new(1);
            sim.set_bounds(320.0, 180.0);
            assert_eq!(sim.sim.bounds(), Some((320.0, 180.0)));
            assert_eq!(sim.frame_len(), 0, "set_bounds must not allocate a frame");
        }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd "/Users/jason/Projects/ob1/open source/BrightFX-17-cull/core" && cargo test -p brightfx-core --features render --lib abi 2>&1 | tail -20`
Expected: compile error, `no method named set_bounds` on `AbiSimulation` (and the viewport tests would fail on `None`). If the crate's feature name differs, use the one in `core/brightfx-core/Cargo.toml`.

- [ ] **Step 3: Implement**

In `abi.rs`, replace `set_viewport`'s body and doc tail so it also feeds the simulation (add one doc paragraph before the `#[cfg(feature = "render")]` line):

```rust
    ///
    /// Also gives the simulation its bounds, `width / scale` by
    /// `height / scale` logical units, so `cullMargin` works with no further
    /// call. A rejected viewport leaves the bounds as they were.
    #[cfg(feature = "render")]
    pub fn set_viewport(&mut self, width: u32, height: u32, scale: f32) -> String {
        self.poisoning(|this| match this.renderer.set_viewport(width, height, scale) {
            Ok(clamped) => {
                // The renderer clamps width, height and scale, so read back
                // what it holds rather than the arguments.
                if let Some(vp) = this.renderer.viewport() {
                    this.sim.set_bounds(vp.width as f32 / vp.scale, vp.height as f32 / vp.scale);
                }
                ok_envelope(&clamped)
            }
            Err(message) => err_envelope(message),
        })
        .unwrap_or_else(|| err_envelope(POISONED_MESSAGE))
    }
```

Add, next to `set_emitter`:

```rust
    /// Sets the logical-unit rectangle `cullMargin` is measured from, for
    /// a host with no viewport (sprite mode). `set_viewport` sets it itself.
    /// See `Simulation::set_bounds`.
    pub fn set_bounds(&mut self, width: f32, height: f32) {
        self.guard_mut(|sim| sim.set_bounds(width, height));
    }
```

In `core/brightfx-wasm/src/lib.rs`, after `set_viewport`:

```rust
    /// Sets the logical-pixel bounds `cullMargin` is measured from, for a
    /// host that draws its own sprites and never calls `setViewport` (which
    /// sets them itself).
    #[wasm_bindgen(js_name = setBounds)]
    pub fn set_bounds(&mut self, width: f32, height: f32) {
        self.inner.set_bounds(width, height);
    }
```

- [ ] **Step 4: Run to verify it passes**

Run: `cd "/Users/jason/Projects/ob1/open source/BrightFX-17-cull/core" && cargo test --workspace 2>&1 | tail -20`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
cd "/Users/jason/Projects/ob1/open source/BrightFX-17-cull" && git add core && git commit -m "ABI: set_viewport feeds the simulation bounds; add set_bounds

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 4: JS wrapper `setBounds`

**Files:**
- Modify: `packages/brightfx-js/src/index.ts` (`EffectConfig` ~line 62-68, `setViewport` ~line 213)
- Modify: `packages/brightfx-js/test/wrapper.test.mjs`

**Interfaces:**
- Consumes: wasm export `setBounds(width, height)` (Task 3).
- Produces: `Simulation.setBounds(width: number, height: number): void`, `EffectConfig.cullMargin?: number | null`. Task 5 calls `setBounds`.

- [ ] **Step 1: Set up and write the failing tests**

The worktree has no `node_modules` or wasm build. Run:

```bash
cd "/Users/jason/Projects/ob1/open source/BrightFX-17-cull" && wasm-pack build core/brightfx-wasm --target web --out-dir pkg-web && cd packages && npm ci
```
Expected: the build finishes and `npm ci` installs. (`sync-wasm` reads `core/brightfx-wasm/pkg-web`, which is gitignored.)

Append to `packages/brightfx-js/test/wrapper.test.mjs`:

```js
const cullConfig = { ...JSON.parse(read("ffi-seek.config.json")), cullMargin: 0 };

function seekCount(setup) {
  const sim = engine.create(42);
  assert.equal(sim.setConfig(cullConfig).ok, true);
  setup(sim);
  sim.seek(1.5);
  const count = sim.particleCount();
  const particles = sim.particleList();
  sim.dispose();
  return { count, particles };
}

test("setBounds lets cullMargin remove particles that leave the bounds", () => {
  const open = seekCount(() => {});
  const bounded = seekCount((sim) => sim.setBounds(200, 120));
  assert.ok(open.count > 0, "test is vacuous: nothing to cull");
  assert.ok(bounded.count < open.count, `nothing culled: ${bounded.count} of ${open.count}`);
  for (const p of bounded.particles) {
    assert.ok(p.x >= 0 && p.x <= 200 && p.y >= 0 && p.y <= 120, `(${p.x}, ${p.y}) is outside the bounds`);
  }
});

test("setViewport sets the same bounds, in logical units", () => {
  const viaBounds = seekCount((sim) => sim.setBounds(100, 60));
  // 200x120 device pixels at scale 2 is a 100x60 logical frame.
  const viaViewport = seekCount((sim) => sim.setViewport(200, 120, 2));
  assert.equal(viaViewport.count, viaBounds.count);
  assert.deepEqual(viaViewport.particles, viaBounds.particles);
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd "/Users/jason/Projects/ob1/open source/BrightFX-17-cull/packages" && npm test -w brightfx-js 2>&1 | tail -20`
Expected: the build fails with `Property 'setBounds' does not exist` (the wasm typings have it, the wrapper does not).

- [ ] **Step 3: Implement**

In `packages/brightfx-js/src/index.ts`, extend `EffectConfig`:

```ts
export type EffectConfig = {
  schemaVersion: number;
  emitterTrack?: EmitterTrack | null;
  /** Schema version 3+. Omitted means `"fixed"`. */
  spinDirection?: SpinDirection;
  /** Schema version 4+. Logical px. Removes a particle once its center is
   *  more than this far outside the simulation's bounds (`setViewport` or
   *  `setBounds`). Omitted or `null` never culls. */
  cullMargin?: number | null;
} & Record<string, unknown>;
```

Add after `setViewport`:

```ts
  /** The logical-pixel size `cullMargin` is measured from, for a host that
   *  draws its own sprites and never calls `setViewport` (which sets it
   *  itself, as `width / scale` by `height / scale`). Changing it makes the
   *  next `seek` replay from zero. */
  setBounds(width: number, height: number): void {
    this.sim.setBounds(width, height);
  }
```

- [ ] **Step 4: Run to verify it passes**

Run: `cd "/Users/jason/Projects/ob1/open source/BrightFX-17-cull/packages" && npm test -w brightfx-js 2>&1 | tail -20`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
cd "/Users/jason/Projects/ob1/open source/BrightFX-17-cull" && git add packages/brightfx-js && git commit -m "JS: add setBounds and the cullMargin config type

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Remotion sprite mode passes its bounds

**Files:**
- Modify: `packages/brightfx-remotion/src/index.tsx` (`useBrightFX` ~lines 55-125, `BrightFX` ~line 139)
- Modify: `packages/brightfx-remotion/test/fixtures/probe-effects.json`, `test/src/Root.tsx`, `test/still.test.mjs`

**Interfaces:**
- Consumes: `Simulation.setBounds(width, height)` (Task 4).
- Produces: `useBrightFX(effect, seed, wasmSrc, viewport, bounds = null)`, with `bounds: { width: number; height: number } | null`. Backwards compatible: the new parameter is optional.

- [ ] **Step 1: Write the failing test**

In `test/fixtures/probe-effects.json`, add a probe after `seek`:

```json
    "bounds": { "right": 16, "color": [255, 0, 255] }
```
(and a comma after the `seek` entry).

In `test/src/Root.tsx`:
- change `PROBE_METHODS` to `{ raster: ["render", "frame"], seek: ["seek"], bounds: ["setBounds"] } as const;`
- add a composition below `SpriteModeTest`:

```tsx
const SpriteBoundsTest: React.FC = () => (
  <>
    <BrightFX
      effect={effect}
      seed={42}
      mode="sprite"
      wasmSrc={WASM}
      render={() => <div style={{ width: 4, height: 4, background: "#ff0000" }} />}
    />
    <Probes />
  </>
);
```
- register it in `Root`: `<Composition id="SpriteBoundsTest" component={SpriteBoundsTest} durationInFrames={60} {...size} />`.

Append to `test/still.test.mjs`:

```js
test("sprite mode gives the simulation the composition's bounds", () => {
  const png = still("SpriteBoundsTest");
  assert.ok(probeShown(png, "bounds"), "setBounds never ran in sprite mode, so cullMargin has nothing to cull against");
});

test("frame mode leaves the bounds to setViewport", () => {
  const png = still("FrameModeTest");
  assert.ok(probeShown(png, "raster"), "the frame never rasterized, so this proves nothing");
  assert.ok(!probeShown(png, "bounds"), "setBounds ran in frame mode, where setViewport already sets the bounds");
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd "/Users/jason/Projects/ob1/open source/BrightFX-17-cull/packages" && npm test -w brightfx-remotion 2>&1 | tail -30`
Expected: `sprite mode gives the simulation the composition's bounds` fails (`setBounds never ran`); the frame-mode test passes. If the run fails earlier (no Chrome for Remotion), report that instead of working around it.

- [ ] **Step 3: Implement**

In `packages/brightfx-remotion/src/index.tsx`, replace the doc comment and signature of `useBrightFX` and the state and effect inside it:

```tsx
/** One simulation per mounted component. Holds Remotion's render until the
 *  wasm has initialized and the config is loaded, then releases it.
 *
 *  `viewport` allocates a frame (frame mode). `bounds` is for a host that
 *  draws its own sprites: it gives the simulation a size for `cullMargin`
 *  without allocating a frame. A viewport sets the bounds itself.
 *
 *  Returns null until a simulation built for exactly this `viewport` and
 *  `bounds` has loaded. When either changes (a sprite/frame mode switch, or a new
 *  composition size), the render that sees the change still holds the old
 *  simulation -- the effect that replaces it runs after that render -- and
 *  drawing with it would rasterize the wrong size, or 0x0 for a sprite-mode
 *  simulation. That render gets null instead; the replacement's
 *  `delayRender` holds the frame until it arrives. */
export function useBrightFX(
  effect: EffectConfig,
  seed: number,
  wasmSrc: string,
  viewport: { width: number; height: number } | null,
  bounds: { width: number; height: number } | null = null,
): Simulation | null {
  const [loaded, setLoaded] = useState<{
    sim: Simulation;
    width: number;
    height: number;
    boundsWidth: number;
    boundsHeight: number;
  } | null>(null);
  const effectJson = useMemo(() => JSON.stringify(effect), [effect]);
  const viewportWidth = viewport?.width ?? 0;
  const viewportHeight = viewport?.height ?? 0;
  const boundsWidth = bounds?.width ?? 0;
  const boundsHeight = bounds?.height ?? 0;
```

In the effect, after the `if (viewportWidth > 0 && viewportHeight > 0) { ... }` block add:

```tsx
        if (boundsWidth > 0 && boundsHeight > 0) {
          created.setBounds(boundsWidth, boundsHeight);
        }
```

Change the `setLoaded` call to
`setLoaded({ sim: created, width: viewportWidth, height: viewportHeight, boundsWidth, boundsHeight });`,
the dependency array to `[effectJson, seed, wasmSrc, viewportWidth, viewportHeight, boundsWidth, boundsHeight]`, and the return to:

```tsx
  return loaded &&
    loaded.width === viewportWidth &&
    loaded.height === viewportHeight &&
    loaded.boundsWidth === boundsWidth &&
    loaded.boundsHeight === boundsHeight
    ? loaded.sim
    : null;
```

In `BrightFX`, change the hook call to:

```tsx
  const sim = useBrightFX(
    effect,
    seed,
    src,
    mode === "frame" ? { width, height } : null,
    mode === "sprite" ? { width, height } : null,
  );
```

- [ ] **Step 4: Run to verify it passes**

Run: `cd "/Users/jason/Projects/ob1/open source/BrightFX-17-cull/packages" && npm test -w brightfx-remotion 2>&1 | tail -30`
Expected: all pass, including `switching from sprite to frame mode mid-render draws the frame-mode frame` (#16).

- [ ] **Step 5: Commit**

```bash
cd "/Users/jason/Projects/ob1/open source/BrightFX-17-cull" && git add packages/brightfx-remotion && git commit -m "Remotion: sprite mode gives the simulation its bounds

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Document `cullMargin` and verify everything

**Files:**
- Modify: `core/brightfx-core/README.md` (the config-field prose near `spinDirection`, ~line 136)

- [ ] **Step 1: Document the field**

Read `core/brightfx-core/README.md` around line 136 and add a short paragraph in the same style next to the `spinDirection` note:

```markdown
`cullMargin` (schema version 4; omitted or `null` means off) removes a
particle once its center is more than that many logical px outside the
simulation's bounds, so an edge-spawned effect need not keep a lifetime long
enough to cross every aspect ratio. The bounds come from `set_viewport`
(`width / scale` by `height / scale`) or, for a host with no viewport such as
sprite mode, from `set_bounds`. Without bounds nothing is culled. Culling is
permanent: a particle that leaves and would fall back in is gone, so pick a
margin that covers its excursion.
```

- [ ] **Step 2: Full verification**

Run each and confirm the output:

```bash
cd "/Users/jason/Projects/ob1/open source/BrightFX-17-cull/core" && cargo test --workspace 2>&1 | tail -15
cd "/Users/jason/Projects/ob1/open source/BrightFX-17-cull/core" && cargo clippy --workspace --all-targets 2>&1 | tail -15
cd "/Users/jason/Projects/ob1/open source/BrightFX-17-cull/packages" && npm test --workspaces 2>&1 | tail -15
cd "/Users/jason/Projects/ob1/open source/BrightFX-17-cull" && git status --short && git log --oneline main..HEAD
```
Expected: all tests pass, clippy shows no new warnings, the tree is clean, and the log shows the spec commit plus one commit per task. Also run `bash core/scripts/smoke-all.sh` if swiftc, dotnet and the pinned cbindgen are installed (the header must be unchanged because the FFI did not change); otherwise say it was not run.

- [ ] **Step 3: Commit**

```bash
cd "/Users/jason/Projects/ob1/open source/BrightFX-17-cull" && git add core/brightfx-core/README.md && git commit -m "Docs: describe cullMargin

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 4: Hand off**

Use superpowers:finishing-a-development-branch to open the PR. The PR description should say the preset retune (frosting-rain and the others) is a follow-up, and that the C FFI has no `bfx_set_bounds` yet. Swift and C# hosts get the bounds from `bfx_set_viewport`; a sprite-mode host there would need one.
