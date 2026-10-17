# Sub-stepping `advance` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `Simulation::advance` splits a long `dt` into steps of at most one 60 Hz frame, and idle emission becomes a rate per frame-equivalent, so the simulation looks the same at any host refresh rate (#49, #48).

**Architecture:** `step` takes frame-equivalents (`fe`) instead of seconds and no longer rebuilds the render buffer. `advance` clamps `dt`, runs `ceil(fe)` equal steps, then rebuilds once; `seek` rebuilds once after its replay. The idle draw scales its probability by `fe`, which is a probability because every step now has `fe <= 1`.

**Tech Stack:** Rust (`core/brightfx-core`), cargo tests, `core/scripts/smoke-all.sh` (Rust + WASM/Node + Swift + C# + JS packages).

**Spec:** [`docs/specs/2026-10-17-advance-substep-design.md`](../specs/2026-10-17-advance-substep-design.md)

**Status:** after the Task 1 review, `advance` splits at `ceil(fe / MAX_STEP_FE)` with `MAX_STEP_FE = 1.1`, not `ceil(fe)`. This keeps jittery 60 Hz frames as one step. A step can now be up to 1.1 frames long. Task 1's code below shows the original `ceil(fe)`; the follow-up fix commit changed it. A whole-branch review after Task 2 then added doc fixes and edge tests: saturated idle at fe 1.09, `advance(0)` spawning no idle particles, and sub-stepping with culling matching 60 Hz.

## Global Constraints

- Steps of `fe <= 1` (a `dt` of 1/60 s or less) must behave bit-for-bit as today, except idle emission below `fe` 1, which Task 2 changes on purpose (#48). No file under `core/fixtures/` or `presets/` may change, and `core/brightfx-core/tests/golden.rs` must pass unchanged.
- `seek` output must not change. It steps at `PLAYBACK_STEP`, which is `fe == 1.0`.
- `seek` must not allocate (`core/brightfx-core/tests/seek_allocations.rs`).
- The emitter is held at its set position for every sub-step. No interpolation.
- `git add` explicit paths only, never `-A`. Commit messages carry no session links and end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- `docs/superpowers/` is gitignored. Never `git add -f` it.

All commands run from the repository root.

---

### Task 1: `advance` sub-steps; the buffer is rebuilt once per call

**Files:**
- Modify: `core/brightfx-core/src/simulation.rs`. The module doc on `REFERENCE_HZ` (~line 11), `advance` (~405), the `seek` loop (~548) and its end (~559), `restart_track` (~578-584), `step` (~609-611 and the `rebuild_buffer` call at its end, ~693).
- Modify: `core/brightfx-ffi/src/lib.rs`, the doc comment on `bfx_advance` (~line 123).
- Regenerate: `core/brightfx-ffi/include/brightfx.h` (via `core/scripts/gen-header.sh`).
- Test: the `#[cfg(test)] mod tests` in `core/brightfx-core/src/simulation.rs`, next to `long_steps_spawn_at_the_configured_rate_and_leave_no_backlog`.

**Interfaces:**
- Produces: `fn step(&mut self, fe: f32)`, which is private. `fe` is frame-equivalents, already clamped, and `<= 1` from every caller. `step` no longer calls `rebuild_buffer`. Task 2 edits the idle branch inside it.
- `pub fn advance(&mut self, dt: f32)` keeps its signature.

- [ ] **Step 1: Write the failing tests**

Add to the test module in `core/brightfx-core/src/simulation.rs`, after `long_steps_spawn_at_the_configured_rate_and_leave_no_backlog`:

```rust
    /// Exercises every per-step term, so a sub-step that differed from a
    /// 1/60 s step anywhere would show in the buffer.
    fn busy_config() -> ParticleFxConfig {
        let mut config = base_config();
        config.emitter.spawn_rate_while_active = 20.0;
        config.emitter.emission_spread = 360.0;
        config.initial_speed_min = 2.0;
        config.initial_speed_max = 6.0;
        config.gravity_y = 1.0;
        config.drag = 0.98;
        config.turbulence = 2.0;
        config.vortex_attraction = 0.5;
        config.rotation_speed_max = 0.2;
        config
    }

    #[test]
    fn a_long_advance_matches_the_same_time_in_sixtieths() {
        let mut long = Simulation::new(busy_config(), 7);
        let mut short = Simulation::new(busy_config(), 7);
        long.set_emitter(100.0, 100.0, 0.0, 0.0, true);
        short.set_emitter(100.0, 100.0, 0.0, 0.0, true);
        for _ in 0..4 {
            long.advance(3.0 / 60.0); // fe 3.0: three steps of fe 1.0
            advance_steps(&mut short, 3);
        }
        assert_eq!(long.particle_count(), short.particle_count());
        assert_eq!(long.buffer(), short.buffer());
    }

    #[test]
    fn a_long_advance_spreads_spawn_ages_over_the_frame() {
        // fe 6 is six steps; the 20 spawned in step k have aged 7 - k.
        let mut config = base_config();
        config.emitter.spawn_rate_while_active = 20.0;
        let mut sim = Simulation::new(config, 1);
        sim.set_emitter(0.0, 0.0, 0.0, 0.0, true);
        sim.advance(MAX_DELTA_SECONDS);
        let mut ages: Vec<f32> = sim.live.pool.iter().map(|p| p.life).collect();
        ages.sort_by(f32::total_cmp);
        ages.dedup();
        assert_eq!(ages, [1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        assert_eq!(sim.particle_count(), 120);
    }
```

`advance_steps(sim, n)` already exists in the test module (~line 2642). It calls `sim.advance(TICK)` n times, with `TICK = 1.0 / 60.0`.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --manifest-path core/Cargo.toml -p brightfx-core --lib -- a_long_advance`
Expected: both FAIL. The buffers differ in the first test. The second has ages `[6.0]` only, since one step ages every spawn by 6.

- [ ] **Step 3: Make `step` take `fe` and stop rebuilding**

In `step`, replace the signature and first two lines:

```rust
    fn step(&mut self, dt: f32) {
        let seconds = dt.max(0.0).min(MAX_DELTA_SECONDS);
        let fe = seconds * REFERENCE_HZ;
```

with:

```rust
    /// One step of `fe` frame-equivalents, at most 1. Does not rebuild the
    /// buffer: `advance` and `seek` rebuild once after their steps.
    fn step(&mut self, fe: f32) {
```

At the end of `step`, delete the `self.rebuild_buffer();` that follows the `retain_mut` closure (~line 693), so the function ends with `});` then `}`.

- [ ] **Step 4: Sub-step `advance`**

Replace:

```rust
    pub fn advance(&mut self, dt: f32) {
        self.leave_baked();
        self.step(dt);
    }
```

with:

```rust
    /// Advances by `dt` seconds, clamped to `0..=0.1` so a stall can't
    /// teleport or mass-expire particles. A `dt` longer than one 60 Hz
    /// frame runs as `ceil(fe)` equal steps of at most a frame each, so
    /// spawns spread over the frame's ages and the physics matches 60 Hz.
    /// The emitter stays where `set_emitter` put it for every step.
    pub fn advance(&mut self, dt: f32) {
        self.leave_baked();
        let fe = dt.max(0.0).min(MAX_DELTA_SECONDS) * REFERENCE_HZ;
        let steps = fe.ceil().max(1.0);
        for _ in 0..steps as u32 {
            self.step(fe / steps);
        }
        self.rebuild_buffer();
    }
```

`dt.max(0.0)` reads NaN as 0, as today: `f32::max` returns the non-NaN operand. `fe == 1.0` gives `steps == 1.0` and `fe / steps == fe` exactly.

- [ ] **Step 5: `seek` steps at `fe` 1 and rebuilds once**

In the `seek` loop, replace `self.step(PLAYBACK_STEP);` with:

```rust
            self.step(PLAYBACK_STEP * REFERENCE_HZ);
```

After the loop, replace:

```rust
        self.baked = Some(n);
    }
```

(the end of `seek`) with:

```rust
        // Steps don't rebuild the buffer, so it is built once here: from
        // the replayed state, or unchanged state on a forward seek that
        // ran no steps.
        self.rebuild_buffer();
        self.baked = Some(n);
    }
```

In `restart_track`, replace:

```rust
                self.live.copy_from(saved);
                // Every step ends by rebuilding the buffer, so only a
                // replay that runs none (a seek to t=0) rebuilds it here:
                // as the last pre-roll step left it.
                if n == track.preroll_steps {
                    self.rebuild_buffer();
                }
                return track.preroll_steps;
```

with:

```rust
                self.live.copy_from(saved);
                return track.preroll_steps;
```

`n` is now unused in `restart_track`. Remove the parameter, and update its doc line ("Returns the step the replay continues from; `n` is the step it runs to.") to "Returns the step the replay continues from." Update the single call in `seek`, `_ => self.restart_track(&track, n),`, to `_ => self.restart_track(&track),`.

- [ ] **Step 6: Update the module doc and `bfx_advance`**

In `simulation.rs`, the doc comment above `const REFERENCE_HZ` ends "`dt` is clamped so a stall can't teleport or mass-expire particles." Append:

```rust
/// A `dt` past one frame is run as several steps of at most a frame (see
/// `Simulation::advance`), so no step has `fe > 1`.
```

In `core/brightfx-ffi/src/lib.rs`, replace the `bfx_advance` doc line `/// Advances the simulation by `dt` seconds.` with:

```rust
/// Advances the simulation by `dt` seconds, clamped to 0..=0.1. A `dt`
/// longer than 1/60 s runs as several steps of at most 1/60 s each.
```

Run: `./core/scripts/gen-header.sh`
Expected: `wrote .../brightfx-ffi/include/brightfx.h`, and `git diff core/brightfx-ffi/include/brightfx.h` shows only the `bfx_advance` comment change.

- [ ] **Step 7: Run the tests**

Run: `cargo test --manifest-path core/Cargo.toml`
Expected: all pass, including both new tests, `golden.rs`, every `*_fixture.rs`, `seek_allocations.rs`, and the existing `long_steps_spawn_at_the_configured_rate_and_leave_no_backlog` (240 then 20) and `the_fraction_of_a_spawn_carries_across_long_steps` (124). Then `git status --short core/fixtures presets` must print nothing.

If `the_fraction_of_a_spawn_carries_across_long_steps` fails, the cause is in the sub-step slicing, not the test. Six sub-steps of 6.9 sum to the same budget per advance, and floors of a running sum total the floor of the sum, 124.

- [ ] **Step 8: Commit**

```bash
git add core/brightfx-core/src/simulation.rs core/brightfx-ffi/src/lib.rs core/brightfx-ffi/include/brightfx.h
git commit -F - <<'EOF'
Simulation: run a long advance as steps of at most one frame

`advance` integrated a long dt as one step, so its spawns all aged by the
full fe and landed in one clump, and turbulence, gravity and vortex were
stepped coarser than at 60 Hz. It now runs ceil(fe) equal steps, each at
most a frame; a dt of 1/60 s or less is one step, exactly as before, so
seek, the goldens and the fixtures do not move.

`step` takes fe and no longer rebuilds the buffer: advance and seek
rebuild once after their steps, which also stops seek's replay rebuilding
on every step (#37).

Part of #49.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
```

---

### Task 2: Idle emission per frame-equivalent

**Files:**
- Modify: `core/brightfx-core/src/simulation.rs`, the idle branch in `step` (`} else if self.config.emitter.spawn_rate_idle > 0.0 && self.live.rng.f32() < self.config.emitter.spawn_rate_idle * 0.3 {`).
- Modify: `core/brightfx-core/src/schema.rs`, the comment in `pool_warnings` (~line 323).
- Test: the test module in `core/brightfx-core/src/simulation.rs`, after `idle_emission_spawns_when_rate_guarantees_it` (~line 1147).

**Interfaces:**
- Consumes: `fn step(&mut self, fe: f32)` from Task 1, with `fe <= MAX_STEP_FE` (1.1) guaranteed.

- [ ] **Step 1: Write the failing tests**

Add after `idle_emission_spawns_when_rate_guarantees_it`:

```rust
    /// Live particles after `steps` advances of `fe` frame-equivalents each,
    /// with the emitter idle. Lifetimes are at the schema's 300 ceiling so
    /// nothing expires inside the 250 fe these tests run.
    fn idle_count(rate: f32, fe: f32, steps: usize) -> usize {
        let mut config = base_config();
        config.emitter.spawn_rate_idle = rate;
        config.lifetime_min = 300.0;
        config.lifetime_max = 300.0;
        let mut sim = Simulation::new(config, 11);
        sim.set_emitter(0.0, 0.0, 0.0, 0.0, false);
        for _ in 0..steps {
            sim.advance(fe / 60.0);
        }
        sim.particle_count()
    }

    #[test]
    fn idle_emission_is_a_rate_per_frame_not_per_step() {
        // Rate 1 is 0.3 per frame. 250 fe at fe 0.4 (a 144 Hz-like host)
        // expects 75, as 250 steps at fe 1 do; a per-step draw gave ~188.
        // The band is about 3 standard deviations either side.
        let fast = idle_count(1.0, 0.4, 625);
        assert!((50..=100).contains(&fast), "fe 0.4: {fast}");
        let sixty = idle_count(1.0, 1.0, 250);
        assert!((50..=100).contains(&sixty), "fe 1: {sixty}");
    }

    #[test]
    fn saturated_idle_emission_is_one_per_frame_at_any_rate() {
        // Rate 5 asks for 1.5 per frame and gets 1. At fe 0.5 that is one
        // every other step, about 100 in 200 steps; a per-step draw gave 200.
        let half = idle_count(5.0, 0.5, 200);
        assert!((80..=120).contains(&half), "fe 0.5: {half}");
        assert_eq!(idle_count(5.0, 1.0, 100), 100, "fe 1: every step");
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --manifest-path core/Cargo.toml -p brightfx-core --lib -- idle_emission_is_a_rate saturated_idle`
Expected: `idle_emission_is_a_rate_per_frame_not_per_step` FAILS on `fe 0.4` with a count near 188. `saturated_idle_emission_is_one_per_frame_at_any_rate` FAILS on `fe 0.5` with 200.

- [ ] **Step 3: Scale the idle draw by `fe`**

Replace:

```rust
        } else if self.config.emitter.spawn_rate_idle > 0.0
            && self.live.rng.f32() < self.config.emitter.spawn_rate_idle * 0.3
        {
```

with:

```rust
        } else if self.config.emitter.spawn_rate_idle > 0.0
            // `rate * 0.3` per frame, saturating at 1, scaled to the
            // step: `fe <= MAX_STEP_FE` (see `advance`). At fe 1 this is
            // the draw Mouseflare made once per 60 Hz frame.
            && self.live.rng.f32() < (self.config.emitter.spawn_rate_idle * 0.3).min(1.0) * fe
        {
```

- [ ] **Step 4: Update the `pool_warnings` comment**

In `core/brightfx-core/src/schema.rs`, replace:

```rust
        // Idle emission is one particle at most per step, with probability
        // `rate * 0.3` (see `Simulation::step`).
```

with:

```rust
        // Idle emission is `rate * 0.3` particles per frame-equivalent,
        // at most one (see `Simulation::step`), at any host refresh rate.
```

- [ ] **Step 5: Run the tests**

Run: `cargo test --manifest-path core/Cargo.toml`
Expected: all pass, including `idle_emission_spawns_when_rate_guarantees_it` and the schema tests that assert "idle spawns at most one per step" (`schema.rs` ~547-550). Their arithmetic is unchanged. Then `git status --short core/fixtures presets` prints nothing.

- [ ] **Step 6: Commit**

```bash
git add core/brightfx-core/src/simulation.rs core/brightfx-core/src/schema.rs
git commit -F - <<'EOF'
Simulation: make idle emission a rate per frame-equivalent

The idle draw was one per step with probability `spawnRateIdle * 0.3`,
so a 144 Hz host spawned about 2.4 times the idle particles of a 60 Hz
one. Every step is now at most 1.1 frames, so the probability is scaled
by fe (saturating at one per frame). At fe 1 the draw is bit-for-bit what it
was, so seek and the fixtures do not move.

Closes #48. Closes #49.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
```

---

### Task 3: Full verification and ship-time docs

**Files:**
- Create: `docs/specs/2026-10-17-advance-substep-design.md` (the spec, moved).
- Create: `docs/plans/2026-10-17-advance-substep.md` (this plan, moved).
- Modify: `docs/README.md` (the index table).
- Modify: the GitHub issue #37 checklist (tick the `rebuild_buffer` item).

- [ ] **Step 1: Run the full suite**

Run: `./core/scripts/smoke-all.sh`
Expected: it ends with `all BrightFX checks passed`. That covers the header check, the fixtures-unchanged check, Node/WASM, Swift, C#, and the JS packages.

- [ ] **Step 2: Move the spec and plan into `docs/`**

```bash
cp docs/superpowers/specs/2026-10-17-advance-substep-design.md docs/specs/
cp docs/superpowers/plans/2026-10-17-advance-substep.md docs/plans/
```

In both copies, take out anything local to one machine: absolute paths and the worktree name.

- [ ] **Step 3: Add index rows**

PR numbers are shared with issues, so take the next free one before writing the rows: `gh api repos/OffBy1-tech/BrightFX/issues?state=all\&per_page=1 --jq '.[0].number + 1'`. Use that as `<pr>`, and if the PR opens with a different number, fix the rows in the same push. Append to the table in `docs/README.md`:

```markdown
| [Sub-stepping advance](specs/2026-10-17-advance-substep-design.md) | spec | shipped with PR #<pr> |
| [Sub-stepping advance](plans/2026-10-17-advance-substep.md) | plan | shipped with PR #<pr> |
```

- [ ] **Step 4: Commit**

```bash
git add docs/specs/2026-10-17-advance-substep-design.md docs/plans/2026-10-17-advance-substep.md docs/README.md
git commit -F - <<'EOF'
docs: spec and plan for sub-stepping advance

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
```

- [ ] **Step 5: Tick #37**

After the PR is open, edit issue #37's body to check `` `rebuild_buffer` runs on every replay step `` and note "done in #<pr>".
