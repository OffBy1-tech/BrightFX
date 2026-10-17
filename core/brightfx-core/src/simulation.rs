use crate::color::{hex_to_rgb, hsl_to_rgb, interpolate_hex, palette_index, sample_palette, Palette};
use crate::particle::{Particle, ParticlePool, MAX_PARTICLES};
use crate::rng::Rng;
use crate::schema::{
    ColorMode, SpinDirection, EmissionPattern, EmitterKeyframe, ParticleFxConfig, TriggerKind,
    MAX_EMITTER_TRACK_DURATION,
    clamp_or_default, sanitize_preroll,
};
use std::sync::Arc;

/// Presets store lifetimes/speeds/gravity/drag/turbulence in 60fps
/// frame-equivalent units (matching Mouseflare's `customFxRenderer.ts`), so
/// `step` converts a wall-clock `dt` (seconds) into frame-equivalents (`fe`)
/// before applying it. `dt` is clamped so a stall can't teleport or
/// mass-expire particles.
/// A `dt` past `MAX_STEP_FE` frames is run as several equal steps (see
/// `Simulation::advance`), so no step has `fe > MAX_STEP_FE`.
const REFERENCE_HZ: f32 = 60.0;
const MAX_DELTA_SECONDS: f32 = 0.1;
/// The longest step `advance` runs, in frame-equivalents. 10% past one
/// frame, so a 60 Hz frame that jitters past 16.667 ms is still one step
/// rather than two half-steps.
const MAX_STEP_FE: f32 = 1.1;
/// Fixed step size used when replaying an `EmitterTrack` in `seek`, so that
/// scrubbing to the same time always reproduces the same buffer. Public so
/// track generators (`brightfx-tracks`) derive their own limits from it
/// rather than hard-coding a sampling rate that can drift from this one.
pub const PLAYBACK_STEP: f32 = 1.0 / 60.0;

/// Grid points per second. The grid is defined by this integer rather than
/// by `1.0 / PLAYBACK_STEP`, which is 59.999997 because `PLAYBACK_STEP` is
/// `1/60` rounded to f32 -- dividing by that is what used to need a fudge
/// factor. `the_grid_rate_is_the_reciprocal_of_the_playback_step` pins the
/// two together.
pub(crate) const GRID_RATE: f64 = 60.0;

/// How far a baked time may sit from a grid point and still snap to it, in
/// steps. The relative term tracks the f32 ULP of `target` (an f32's ULP
/// grows with its magnitude, and `target * 60` scales that error by 60);
/// the absolute term covers times near zero.
const GRID_TOLERANCE_ABS: f64 = 1e-6;
const GRID_TOLERANCE_REL: f64 = 8e-6;

fn grid_tolerance(target: f32) -> f64 {
    GRID_TOLERANCE_ABS + GRID_TOLERANCE_REL * target as f64
}

/// The same snap-then-ceil rule as `grid_step`, applied to a signed value
/// in f64: -0.5 s maps to -30 and -0.49 s to -29 (the grid point at or
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

/// The grid step a baked time maps to: the grid point *at or after*
/// `target`. Computed in f64 so the f32 rounding of `target` itself is
/// the only error -- a time within a few of its own ULPs of a grid point
/// snaps to that point (from either side), and any other time takes the
/// next point up. Baked state always sits exactly on a grid point, so a
/// forward seek and a from-zero seek run the same sequence of whole steps
/// and produce the same buffer.
///
/// Rounding up rather than down is what makes the contract "nothing
/// authored at or before `target` is still pending": an off-grid host
/// (24 fps frames never land on the 1/60 grid) would otherwise see every
/// trigger one frame late. The snap keeps that contract even when it
/// rounds down: a trigger authored between the grid point and `target`
/// is nearer the point than `target` is, so it snaps to the same step and
/// has fired. What a downward snap does move is the particle state, which
/// then sits slightly *before* `target` -- see `seek`.
pub(crate) fn grid_step(target: f32) -> u32 {
    // Saturates both ways, as the trigger steps do: a negative or NaN time
    // is step 0, and a plain `as u32` on the i64 would truncate, wrapping a
    // huge time back onto the track.
    signed_grid_step(target).clamp(0, u32::MAX as i64) as u32
}

/// One of the track's triggers with the step it fires in resolved ahead
/// of time: `fire_step` is the absolute grid step at or after its authored
/// time (`preroll_steps` plus the signed step of the time), so the trigger
/// fires in the step that *ends* there -- exactly the step `seek` runs to
/// reach that time. `0` means it fires at the reset, before any step runs,
/// which is where a trigger at or before `-preroll` lands.
#[derive(Debug, Clone, Copy)]
struct BakedTrigger {
    fire_step: u32,
    kind: TriggerKind,
}

/// Everything a step can change. The render buffer is not here: it is
/// rebuilt from this and the config. Kept in one struct so the copy of it
/// `seek` saves at the end of a track's pre-roll cannot miss a field.
struct StepState {
    pool: ParticlePool,
    /// Drives every motion draw: speed, angle, life, jitter, rotation.
    rng: Rng,
    /// Drives the palette pick, and nothing else. Separate from `rng` so
    /// a colour-only config change never shifts a motion draw, and reset
    /// with it so a replay deals the same colours.
    color_rng: Rng,
    spawn_budget: f32,
    global_hue: f32,
    emitter_x: f32,
    emitter_y: f32,
    emitter_vx: f32,
    emitter_vy: f32,
    emitter_speed: f32,
    emitter_active: bool,
}

impl StepState {
    /// The state at a reset. The pool's deque is allocated here, at its
    /// full `capacity`, and never again.
    fn new(seed: u64, capacity: usize) -> Self {
        Self {
            pool: ParticlePool::new(capacity),
            rng: Rng::new(seed),
            color_rng: color_rng(seed),
            spawn_budget: 0.0,
            global_hue: 0.0,
            emitter_x: 0.0,
            emitter_y: 0.0,
            emitter_vx: 0.0,
            emitter_vy: 0.0,
            emitter_speed: 0.0,
            emitter_active: false,
        }
    }

    /// Back to `new(seed, _)`, keeping the pool's allocation.
    fn reset(&mut self, seed: u64) {
        let mut pool = std::mem::replace(&mut self.pool, ParticlePool::new(0));
        pool.clear();
        *self = Self { pool, ..Self::new(seed, 0) };
    }

    /// `clone_from` without allocating: the pool copies into its own
    /// deque. The destructuring has no `..`, so a field added to the
    /// struct does not compile until it is copied here too.
    fn copy_from(&mut self, source: &Self) {
        let Self {
            pool,
            rng,
            color_rng,
            spawn_budget,
            global_hue,
            emitter_x,
            emitter_y,
            emitter_vx,
            emitter_vy,
            emitter_speed,
            emitter_active,
        } = source;
        self.pool.clone_from(pool);
        self.rng = rng.clone();
        self.color_rng = color_rng.clone();
        self.spawn_budget = *spawn_budget;
        self.global_hue = *global_hue;
        self.emitter_x = *emitter_x;
        self.emitter_y = *emitter_y;
        self.emitter_vx = *emitter_vx;
        self.emitter_vy = *emitter_vy;
        self.emitter_speed = *emitter_speed;
        self.emitter_active = *emitter_active;
    }
}

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
    /// `preroll` through `sanitize_preroll`: in `0..=MAX_PREROLL` and on
    /// the grid, so it is exactly `preroll_steps` steps. NaN is 0.
    preroll: f32,
    /// Whole grid steps the timeline runs before authored t=0. Step
    /// indices are absolute: 0 is the start of the pre-roll and
    /// `preroll_steps` is t=0, so a `fire_step` or a `baked` cursor of
    /// `preroll_steps` means "at t=0".
    preroll_steps: u32,
}

/// One particle's render state. `#[repr(C)]` is load-bearing: hosts read
/// the buffer as a flat `f32` array across the FFI boundary, so the field
/// order and 32-byte stride are part of the public ABI.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParticleInstance {
    pub x: f32,
    pub y: f32,
    pub size: f32,
    pub rotation: f32,
    /// Normalized RGBA, each channel 0..1.
    pub color: [f32; 4],
}

/// Adding a field here changes the ABI stride that every host relies on.
/// This turns that into a compile error rather than corrupted rendering.
const _: () = assert!(std::mem::size_of::<ParticleInstance>() == 32);

pub struct Simulation {
    config: ParticleFxConfig,
    /// `config.color_stops` parsed and sorted for `ColorMode::MultiPalette`
    /// and `ColorMode::RandomPalette`.
    /// Empty when the config carries no stops. Derived from `config`, so it
    /// is rebuilt wherever `config` is replaced.
    palette: Palette,
    /// The replay-ready track, so `seek` neither clones nor re-sorts per
    /// call. Behind an `Arc` so `seek` can hold a handle to it while
    /// borrowing `self` mutably for the step loop, without ever emptying
    /// the field -- a panic mid-replay would otherwise leave the
    /// simulation trackless. `Arc` rather than `Rc` so `Simulation` stays
    /// `Send`.
    baked_track: Option<Arc<BakedTrack>>,
    seed: u64,
    live: StepState,
    buffer: Vec<ParticleInstance>,
    /// Absolute whole steps applied since the last reset in baked mode
    /// (step 0 is the start of the pre-roll, see
    /// `BakedTrack::preroll_steps`). `None`
    /// whenever the pool is not the product of a pure replay, so the next
    /// `seek` replays from the start of the track. Only `seek` sets it; everything else
    /// clears it through `leave_baked`.
    baked: Option<u32>,
    /// Logical-pixel size of the rectangle `[0, w] x [0, h]` that
    /// `config.cull_margin` is measured from. `None` until the host sets
    /// it, and while it is `None` nothing is culled. Part of the state a
    /// replay depends on, so changing it clears `baked` (see `set_bounds`).
    bounds: Option<(f32, f32)>,
    /// Room for the state at the end of the track's pre-roll, which
    /// depends only on the config, the seed, and the bounds. `Some` exactly
    /// when the track has a pre-roll, allocated where the config is set so
    /// that `seek`, which fills it, never allocates: on wasm an allocation
    /// can grow memory and detach a host's view of the buffer.
    preroll_store: Option<StepState>,
    /// Whether `preroll_store` holds the state for the current inputs. Only
    /// `seek` sets it; `replay_inputs_changed` clears it.
    preroll_saved: bool,
}

impl Simulation {
    /// The config is run through `clamp_to_bounds` first, so one built in
    /// code that skipped it plays as the ABI would play it (a NaN or a
    /// negative `drag`, say, cannot make positions NaN). `config()` returns
    /// the clamped config. The report is dropped: call `clamp_to_bounds`
    /// beforehand to see it.
    pub fn new(mut config: ParticleFxConfig, seed: u64) -> Self {
        config.clamp_to_bounds();
        let mut sim = Self {
            palette: build_palette(&config),
            baked_track: build_baked_track(&config),
            config,
            seed,
            live: StepState::new(seed, MAX_PARTICLES),
            // Preallocated so the pointer handed across the FFI boundary is
            // stable for the life of the simulation. `rebuild_buffer` only
            // ever clears and re-pushes, never exceeding MAX_PARTICLES, so
            // this Vec never reallocates.
            buffer: Vec::with_capacity(MAX_PARTICLES),
            baked: None,
            bounds: None,
            preroll_store: None,
            preroll_saved: false,
        };
        sim.reserve_preroll_store();
        sim
    }

    /// Swaps the config. Live particles keep their state, except that the
    /// cull rule is re-applied to them at once, so a smaller `cullMargin`
    /// takes effect without waiting for a step. Any config does this, not
    /// only one that changes the margin, and the cull is permanent. The
    /// config is clamped as `new` clamps it.
    pub fn set_config(&mut self, mut config: ParticleFxConfig) {
        config.clamp_to_bounds();
        self.palette = build_palette(&config);
        self.baked_track = build_baked_track(&config);
        self.config = config;
        // Live particles keep their state (that is the boundary's contract),
        // but the track may have changed, so the next seek replays, and
        // the pre-roll it would resume from was built for the old config.
        self.replay_inputs_changed();
        self.reserve_preroll_store();
        // The (possibly new) `cullMargin` applies to what is already there,
        // so a render without a step does not show particles it would drop.
        self.recull();
    }

    /// Sets the logical-pixel rectangle `[0, width] x [0, height]` that
    /// `cullMargin` is measured from. Until it is set nothing is culled.
    /// A non-finite or non-positive dimension is an error and changes
    /// nothing, so a host measuring a container before layout is told
    /// rather than having culling turned off. To stop culling, set
    /// `cullMargin` to `None`.
    ///
    /// The ABI's `set_viewport` calls this itself. A host with no viewport
    /// (sprite mode) calls it with its container size.
    ///
    /// A change clears the baked position: culling makes the pool depend
    /// on the bounds, so the next `seek` must replay from the start of the track rather than
    /// step forward from a pool built for another size. It also culls the
    /// live pool against the new bounds now, so a render without a step
    /// does not show particles the new frame would have dropped. That is
    /// permanent: growing the bounds again does not bring them back, so a
    /// transient small size (a host measuring a container twice) costs
    /// the particles outside it. Setting
    /// the bounds the simulation already has changes nothing and leaves it
    /// alone.
    pub fn set_bounds(&mut self, width: f32, height: f32) -> Result<(), String> {
        let valid = width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0;
        if !valid {
            return Err(format!("bounds must be finite and positive, got {width} x {height}"));
        }
        let bounds = Some((width, height));
        if bounds != self.bounds {
            self.bounds = bounds;
            self.replay_inputs_changed();
            self.recull();
        }
        Ok(())
    }

    /// Applies the cull rule to the live pool at its current positions, so
    /// a resize or a new `cullMargin` takes effect on what `buffer` already
    /// holds rather than at the next step. A no-op when culling is off.
    fn recull(&mut self) {
        let Some((margin, w, h)) = self.cull_rule() else {
            return;
        };
        let before = self.live.pool.len();
        self.live.pool.retain_mut(|p| apply_cull(p, margin, w, h));
        if self.live.pool.len() != before {
            self.rebuild_buffer();
        }
    }

    /// `(margin, width, height)` when culling is active: the author asked
    /// for it (`cullMargin`) and the host said how big the frame is.
    fn cull_rule(&self) -> Option<(f32, f32, f32)> {
        match (self.config.cull_margin, self.bounds) {
            (Some(margin), Some((w, h))) => Some((margin, w, h)),
            _ => None,
        }
    }

    /// The bounds `set_bounds` last accepted, in logical px.
    pub fn bounds(&self) -> Option<(f32, f32)> {
        self.bounds
    }

    /// Sets the emitter's position, velocity, and active state.
    ///
    /// A non-finite `x`, `y`, `vx`, or `vy` is treated as zero for that
    /// component: a host feeding `dx/dt` with `dt == 0` gets a stationary
    /// emitter rather than a poisoned frame (a NaN velocity would otherwise
    /// flow into `emitter_speed` and poison every particle's
    /// `ColorMode::SpeedResponsive` color).
    ///
    /// Clears the baked position: the host has moved the emitter out from
    /// under the track, so the next `seek` must replay and re-sample it
    /// rather than step forward from a state the track did not produce.
    pub fn set_emitter(&mut self, x: f32, y: f32, vx: f32, vy: f32, active: bool) {
        self.leave_baked();
        self.place_emitter(x, y, vx, vy, active);
    }

    /// The emitter write itself, shared by the host-facing `set_emitter`
    /// and by baked playback.
    fn place_emitter(&mut self, x: f32, y: f32, vx: f32, vy: f32, active: bool) {
        let x = if x.is_finite() { x } else { 0.0 };
        let y = if y.is_finite() { y } else { 0.0 };
        let vx = if vx.is_finite() { vx } else { 0.0 };
        let vy = if vy.is_finite() { vy } else { 0.0 };
        self.live.emitter_x = x;
        self.live.emitter_y = y;
        self.live.emitter_vx = vx;
        self.live.emitter_vy = vy;
        self.live.emitter_speed = (vx * vx + vy * vy).sqrt();
        self.live.emitter_active = active;
    }

    pub fn trigger_burst(&mut self) {
        self.leave_baked();
        self.spawn_burst();
    }

    /// The burst itself, shared by the host-facing `trigger_burst` and by
    /// baked playback.
    fn spawn_burst(&mut self) {
        let count = self.config.emitter.spawn_burst_size;
        for _ in 0..count {
            self.spawn_particle(true);
        }
        self.rebuild_buffer();
    }

    /// Advances by `dt` seconds, clamped to `0..=0.1` so a stall can't
    /// teleport or mass-expire particles. A `dt` longer than 1.1 60 Hz
    /// frames runs as `ceil(fe / 1.1)` equal steps of at most 1.1 frames
    /// each, so spawns spread over the frame's ages and the physics matches
    /// 60 Hz.
    /// The emitter stays where `set_emitter` put it for every step.
    pub fn advance(&mut self, dt: f32) {
        self.leave_baked();
        let fe = dt.max(0.0).min(MAX_DELTA_SECONDS) * REFERENCE_HZ;
        let steps = (fe / MAX_STEP_FE).ceil().max(1.0);
        for _ in 0..steps as u32 {
            self.step(fe / steps);
        }
        self.rebuild_buffer();
    }

    pub fn buffer(&self) -> &[ParticleInstance] {
        &self.buffer
    }

    pub fn config(&self) -> &ParticleFxConfig {
        &self.config
    }

    pub fn particle_count(&self) -> usize {
        self.live.pool.len()
    }

    /// The loaded config's name. Lets hosts confirm which effect is active
    /// without round-tripping the whole config back across the boundary.
    pub fn config_name(&self) -> &str {
        &self.config.name
    }

    /// Drops the baked position, so the next `seek` replays from the start of the track
    /// (its pre-roll before t=0) instead of stepping forward. The one place `baked` is cleared. By
    /// convention -- enforced by tests, not the compiler -- every host-facing call that
    /// changes simulation state calls this: `set_emitter`, `trigger_burst`,
    /// and `advance` before their own writes, `set_config` after swapping
    /// the config, `set_bounds` when the size changes, and `reset`. A new such call must do the same, or a
    /// forward seek would step on from a state the track did not produce.
    /// `every_public_mutator_is_known_to_this_suite` fails when a public
    /// `&mut self` method is added, and `every_state_mutator_invalidates_forward_seek`
    /// checks each listed one against a fresh seek. A call that changes a
    /// replay input calls `replay_inputs_changed` instead, which also drops
    /// the saved pre-roll.
    fn leave_baked(&mut self) {
        self.baked = None;
    }

    /// For a call that changes what a replay computes -- the config or the
    /// bounds; the seed is fixed -- rather than only the live state: drops
    /// the baked position and the saved pre-roll, which was built from the
    /// old inputs. `every_state_mutator_keeps_a_backward_seek_pure` checks
    /// that each mutator that needs this calls it.
    fn replay_inputs_changed(&mut self) {
        self.leave_baked();
        self.preroll_saved = false;
    }

    /// Allocates `preroll_store` when the track has a pre-roll and frees it
    /// when it has none, so a pre-roll costs its memory up front.
    fn reserve_preroll_store(&mut self) {
        let wanted = self.baked_track.as_ref().is_some_and(|track| track.preroll_steps > 0);
        if !wanted {
            self.preroll_store = None;
        } else if self.preroll_store.is_none() {
            self.preroll_store = Some(StepState::new(self.seed, MAX_PARTICLES));
        }
    }

    fn reset(&mut self) {
        self.leave_baked();
        self.live.reset(self.seed);
        self.buffer.clear();
    }

    /// Runs baked playback *through* the grid point at or after `time`:
    /// when `time` is rendered, every trigger authored at or before it
    /// has fired, and the simulation may sit up to one step (1/60 s)
    /// past `time`. A `time` within tolerance of a grid point snaps to
    /// it; `time` is clamped to `[0, duration]` first.
    ///
    /// Never late is the point: a host rendering at any frame rate sees
    /// a trigger on the first frame at or after its authored time, and a
    /// trigger authored at the very end of a track fires even though no
    /// frame time lands exactly on an off-grid duration.
    ///
    /// The snap tolerance grows with `time`, so a `time` just *after* a
    /// grid point can snap down to it and the particle state sits before
    /// `time` -- by at most 80 µs, the tolerance at the 600 s cap.
    /// Triggers are unaffected (see `grid_step`). In practice only NTSC
    /// rates hit this, late in a track; the crate README has the onsets.
    ///
    /// With `emitterTrack.preroll` set, the replay starts that many seconds
    /// before t=0 (`preroll_steps` whole steps in front of the grid), so
    /// `seek(0)` returns the pool those steps built. `time` is still
    /// clamped at 0: the pre-roll is simulated, never rendered.
    ///
    /// If the last call was a seek to an earlier or equal grid step, only
    /// the steps in between are applied; otherwise the simulation resets
    /// and replays from the start of the track (its pre-roll before t=0).
    /// Both paths run the same whole steps from a
    /// reset, so the buffer is a pure function of `time` regardless of
    /// call history. A no-op (empty buffer) if the config has no
    /// `emitter_track`.
    pub fn seek(&mut self, time: f32) {
        // A handle, not a `take`: the step loop needs `self` mutably, and
        // leaving the field populated means a panic mid-replay cannot
        // strand the simulation without its track.
        let track = match &self.baked_track {
            Some(track) => Arc::clone(track),
            None => return,
        };

        // `duration_cap` bounds the loop: a caller-supplied `time` (a
        // corrupt project file, a UI bug, ...) can't drive an unbounded
        // number of synchronous step iterations. The pre-roll adds at most
        // `grid_step(MAX_PREROLL)` steps in front; it is simulated, never
        // rendered, so `time` still clamps at 0.
        let target = time.max(0.0).min(track.duration_cap);
        let n = track.preroll_steps + grid_step(target);

        // On the forward path the `active` the loop starts from is
        // `self.live.emitter_active` as the previous seek left it. That is
        // sound because every other writer of `emitter_active` calls
        // `leave_baked` first, so a cursor can only survive a pure replay.
        let from = match self.baked {
            Some(step) if step <= n => step,
            _ => self.restart_track(&track),
        };

        for k in from..n {
            // Grid times are computed from the absolute step index, never
            // accumulated, so step k is the same on every path. `authored`
            // is the step measured from t=0: negative inside the pre-roll,
            // exactly `k + 1` without one, so a track with no pre-roll
            // samples the same small exact integers times the step as it
            // did before pre-roll existed.
            // The sample is clamped to the duration so the final step of
            // an off-grid track does not extrapolate past the last
            // keyframe window.
            let authored = (k + 1) as i64 - track.preroll_steps as i64;
            let t_next = (authored as f32 * PLAYBACK_STEP).min(track.duration_cap);

            let (x, y, vx, vy) = sample_track(&track.keyframes, t_next);
            // Update position/velocity for this step before evaluating
            // triggers, so a `Burst` firing this step spawns at the
            // correct sampled position.
            let active = self.live.emitter_active;
            self.place_emitter(x, y, vx, vy, active);

            self.fire_triggers(&track, k + 1);

            self.step(PLAYBACK_STEP * REFERENCE_HZ);

            if k + 1 == track.preroll_steps && !self.preroll_saved {
                if let Some(store) = self.preroll_store.as_mut() {
                    store.copy_from(&self.live);
                    self.preroll_saved = true;
                }
            }
        }

        // Steps don't rebuild the buffer, so it is built once here: from
        // the replayed state, or unchanged state on a forward seek that
        // ran no steps.
        self.rebuild_buffer();
        self.baked = Some(n);
    }

    /// Resets at the start of the timeline (`-preroll`, which is t=0 when
    /// there is no pre-roll) and fires the `fire_step == 0` triggers --
    /// those authored at or before the start. Baked playback requires an
    /// explicit `StartContinuous` (or a `Burst`) to spawn anything --
    /// unlike live mode, seek never implicitly emits from t=0. The emitter
    /// is placed at `-preroll`, the grid point the reset stands for.
    ///
    /// A replay resumes from the saved end of the pre-roll when there is
    /// one: every seek target is at or past it, and that state is what the
    /// steps it skips would have produced. The copy overwrites every field
    /// a reset would, so there is no reset first. Returns the step the
    /// replay continues from.
    fn restart_track(&mut self, track: &BakedTrack) -> u32 {
        if self.preroll_saved {
            self.leave_baked();
            if let Some(saved) = &self.preroll_store {
                self.live.copy_from(saved);
                return track.preroll_steps;
            }
        }
        self.reset();
        let (x0, y0, vx0, vy0) = sample_track(&track.keyframes, -track.preroll);
        self.place_emitter(x0, y0, vx0, vy0, false);
        self.fire_triggers(track, 0);
        0
    }

    /// Applies the triggers that fire in `step`, in time order. Each
    /// trigger names exactly one step, so there is no bookkeeping of what
    /// has fired.
    fn fire_triggers(&mut self, track: &BakedTrack, step: u32) {
        for trig in &track.triggers {
            if trig.fire_step == step {
                match trig.kind {
                    TriggerKind::Burst => self.spawn_burst(),
                    TriggerKind::StartContinuous => self.live.emitter_active = true,
                    TriggerKind::StopContinuous => self.live.emitter_active = false,
                }
            }
        }
    }

    /// One step of `fe` frame-equivalents, at most `MAX_STEP_FE`. Does not
    /// rebuild the buffer: `advance` and `seek` rebuild once after their steps.
    fn step(&mut self, fe: f32) {
        debug_assert!(fe <= MAX_STEP_FE, "step takes at most MAX_STEP_FE frame-equivalents, got {fe}");
        if self.live.emitter_active {
            self.live.spawn_budget += self.config.emitter.spawn_rate_while_active * fe;
            // Every whole spawn asked for is made, so a long step emits as
            // many as the same time at 60 Hz, and only the fraction carries.
            // Mouseflare capped this at 40 per step, which lost spawns on
            // steps past ~34 ms and, carried over, built a backlog. Past the
            // pool a spawn only evicts this step's own, so that is the bound.
            let wanted = self.live.spawn_budget.floor();
            self.live.spawn_budget -= wanted;
            let count = wanted.clamp(0.0, MAX_PARTICLES as f32) as u32;
            for _ in 0..count {
                self.spawn_particle(false);
            }
        } else if self.config.emitter.spawn_rate_idle > 0.0
            // `rate * 0.3` per frame, saturating at 1, scaled to the
            // step: `fe <= MAX_STEP_FE` (see `advance`). At fe 1 this is
            // the draw Mouseflare made once per 60 Hz frame.
            && self.live.rng.f32() < (self.config.emitter.spawn_rate_idle * 0.3).min(1.0) * fe
        {
            self.spawn_particle(false);
        }

        let gravity_x = self.config.gravity_x * 0.1;
        let gravity_y = self.config.gravity_y * 0.1;
        let drag = self.config.drag;
        let turbulence = self.config.turbulence;
        let vortex = self.config.vortex_attraction;
        let emitter_x = self.live.emitter_x;
        let emitter_y = self.live.emitter_y;
        let size_curve = self.config.size_curve;

        // Only culls when the author asked for it and the host said how big
        // the frame is.
        let cull = self.cull_rule();
        self.live.pool.retain_mut(|p| {
            p.life += fe;
            if p.life >= p.max_life {
                return false;
            }

            p.vx += gravity_x * fe;
            p.vy += gravity_y * fe;

            let drag_step = if drag == 1.0 { 1.0 } else { drag.powf(fe) };
            p.vx *= drag_step;
            p.vy *= drag_step;

            if turbulence > 0.0 {
                let time = (p.life + p.turbulence_seed) * 0.1;
                p.vx += time.sin() * turbulence * 0.15 * fe;
                p.vy += (time * 0.8).cos() * turbulence * 0.15 * fe;
            }

            if vortex != 0.0 {
                let to_x = emitter_x - p.x;
                let to_y = emitter_y - p.y;
                let dist = (to_x * to_x + to_y * to_y).sqrt();
                if dist > 5.0 && dist < 300.0 {
                    let norm_x = to_x / dist;
                    let norm_y = to_y / dist;
                    let tan_x = -norm_y;
                    let tan_y = norm_x;
                    p.vx += (tan_x * vortex * 0.8 + norm_x * 0.2) * fe;
                    p.vy += (tan_y * vortex * 0.8 + norm_y * 0.2) * fe;
                }
            }

            p.x += p.vx * fe;
            p.y += p.vy * fe;
            if let Some((margin, w, h)) = cull {
                if !apply_cull(p, margin, w, h) {
                    return false;
                }
            }
            p.rotation += p.rotation_speed * fe;

            let progress = if p.max_life > 0.0 { p.life / p.max_life } else { 1.0 };
            p.size = evaluate_size_curve(size_curve, p, progress).max(0.2);
            p.alpha = evaluate_alpha_curve(p, progress).clamp(0.0, 1.0);

            true
        });
    }

    fn spawn_particle(&mut self, is_burst: bool) {
        let emitter = self.config.emitter.clone();
        let dx = self.live.emitter_vx;
        let dy = self.live.emitter_vy;

        let pattern = if is_burst {
            EmissionPattern::RadialBurst
        } else {
            emitter.emission_pattern
        };

        let mut speed = self
            .live
            .rng
            .range(self.config.initial_speed_min, self.config.initial_speed_max);

        let angle = if is_burst {
            speed *= 1.4;
            self.live.rng.range(0.0, std::f32::consts::TAU)
        } else {
            match pattern {
                EmissionPattern::RadialBurst
                | EmissionPattern::VortexSpiral
                | EmissionPattern::Orbit => self.live.rng.range(0.0, std::f32::consts::TAU),
                EmissionPattern::Fountain => {
                    let base = (emitter.emission_angle - 90.0).to_radians();
                    let spread = emitter.emission_spread.to_radians();
                    base + (self.live.rng.f32() - 0.5) * spread
                }
                EmissionPattern::DirectionalCone => {
                    let base = emitter.emission_angle.to_radians();
                    let spread = emitter.emission_spread.to_radians();
                    base + (self.live.rng.f32() - 0.5) * spread
                }
                EmissionPattern::Trail => {
                    if (dx * dx + dy * dy).sqrt() > 0.1 {
                        let move_angle = dy.atan2(dx);
                        move_angle
                            + std::f32::consts::PI
                            + (self.live.rng.f32() - 0.5) * emitter.emission_spread.to_radians()
                    } else {
                        self.live.rng.range(0.0, std::f32::consts::TAU)
                    }
                }
            }
        };

        let vx = angle.cos() * speed + self.live.emitter_vx * emitter.velocity_inheritance * 0.15;
        let vy = angle.sin() * speed + self.live.emitter_vy * emitter.velocity_inheritance * 0.15;

        let max_life = self
            .live
            .rng
            .range(self.config.lifetime_min, self.config.lifetime_max);

        self.live.global_hue = (self.live.global_hue + self.config.rainbow_speed * 0.5) % 360.0;
        let color_rgb = if self.config.color_mode == ColorMode::RainbowCycle {
            hsl_to_rgb(self.live.global_hue, 0.9, 0.6)
        } else {
            hex_to_rgb(&self.config.primary_color)
        };
        // Drawn in every mode, so switching into random-palette later finds
        // a pick already on every live particle.
        let palette_pick = self.live.color_rng.f32();

        let rotation_speed = self
            .live
            .rng
            .range(self.config.rotation_speed_min, self.config.rotation_speed_max);
        // Drawn only in random mode, so a fixed-spin config keeps the
        // motion sequence it has always had.
        let rotation_speed = if self.config.spin_direction == SpinDirection::Random && self.live.rng.f32() < 0.5 {
            -rotation_speed
        } else {
            rotation_speed
        };

        let particle = Particle {
            x: self.live.emitter_x + (self.live.rng.f32() - 0.5) * 4.0,
            y: self.live.emitter_y + (self.live.rng.f32() - 0.5) * 4.0,
            vx,
            vy,
            size: self.config.start_size,
            start_size: self.config.start_size,
            peak_size: self.config.peak_size,
            end_size: self.config.end_size,
            alpha: self.config.start_alpha,
            start_alpha: self.config.start_alpha,
            peak_alpha: self.config.peak_alpha,
            end_alpha: self.config.end_alpha,
            color_rgb,
            palette_pick,
            hue: self.live.global_hue,
            life: 0.0,
            max_life,
            rotation: self.live.rng.range(0.0, std::f32::consts::TAU),
            rotation_speed,
            turbulence_seed: self.live.rng.range(0.0, 100.0),
            entered: false,
        };

        self.live.pool.spawn(particle);
    }

    fn rebuild_buffer(&mut self) {
        self.buffer.clear();
        let color_mode = self.config.color_mode;
        let primary = &self.config.primary_color;
        let secondary = &self.config.secondary_color;
        let accent = &self.config.accent_color;
        let palette = &self.palette;
        let emitter_speed = self.live.emitter_speed;

        for p in self.live.pool.iter() {
            let progress = if p.max_life > 0.0 { p.life / p.max_life } else { 1.0 };
            let rgb = match color_mode {
                ColorMode::GradientLifetime => {
                    if progress < 0.5 {
                        interpolate_hex(primary, secondary, progress / 0.5)
                    } else {
                        interpolate_hex(secondary, accent, (progress - 0.5) / 0.5)
                    }
                }
                ColorMode::RainbowCycle => {
                    let h = (p.hue + progress * 120.0).rem_euclid(360.0);
                    hsl_to_rgb(h, 0.9, 0.6)
                }
                ColorMode::SpeedResponsive => {
                    let speed_ratio = (emitter_speed / 1200.0).clamp(0.0, 1.0);
                    interpolate_hex(primary, accent, speed_ratio)
                }
                ColorMode::MultiPalette if !palette.is_empty() => sample_palette(palette, progress),
                ColorMode::RandomPalette if !palette.is_empty() => {
                    palette[palette_index(p.palette_pick, palette.len())].1
                }
                ColorMode::Single | ColorMode::MultiPalette | ColorMode::RandomPalette => p.color_rgb,
            };

            self.buffer.push(ParticleInstance {
                x: p.x,
                y: p.y,
                size: p.size,
                rotation: p.rotation,
                color: [rgb[0], rgb[1], rgb[2], p.alpha],
            });
        }
    }
}

/// The cull rule, applied after a particle's position is final. Returns
/// whether the particle survives, and as a side effect marks it `entered`
/// when it is inside the margin rectangle -- so call it once per position,
/// never as a dry-run query.
///
/// A position that is not finite cannot be on screen, entered or not. (It
/// is also never "inside", so it would otherwise be exempt, as a particle
/// that has not entered yet.) Outside the rectangle, a particle is culled
/// if it was inside before or is heading away from it. One that is outside
/// but moving toward the rectangle is an emitter placed off-screen, and is
/// kept so it can enter.
fn apply_cull(p: &mut Particle, margin: f32, w: f32, h: f32) -> bool {
    if !p.x.is_finite() || !p.y.is_finite() {
        return false;
    }
    if within_margin(p.x, p.y, margin, w, h) {
        p.entered = true;
        true
    } else {
        !(p.entered || moving_away(p, margin, w, h))
    }
}

/// Whether `(x, y)` lies in the cull rectangle `[-margin, w + margin] x
/// [-margin, h + margin]`. False for a NaN coordinate, since every
/// comparison with NaN is false.
fn within_margin(x: f32, y: f32, margin: f32, w: f32, h: f32) -> bool {
    x >= -margin && x <= w + margin && y >= -margin && y <= h + margin
}

/// Whether a particle outside the cull rectangle is moving away from it: its
/// velocity has a positive component along the offset from the rectangle's
/// nearest point. False for a particle inside the rectangle or at rest.
fn moving_away(p: &Particle, margin: f32, w: f32, h: f32) -> bool {
    // Signed distance past the nearest edge on one axis, 0 when inside it.
    let overshoot = |v: f32, size: f32| {
        if v < -margin {
            v + margin
        } else if v > size + margin {
            v - size - margin
        } else {
            0.0
        }
    };
    overshoot(p.x, w) * p.vx + overshoot(p.y, h) * p.vy > 0.0
}

fn evaluate_size_curve(curve: crate::schema::SizeCurve, p: &Particle, progress: f32) -> f32 {
    use crate::schema::SizeCurve::*;
    match curve {
        GrowShrink => {
            if progress < 0.3 {
                p.start_size + (p.peak_size - p.start_size) * (progress / 0.3)
            } else {
                p.peak_size - (p.peak_size - p.end_size) * ((progress - 0.3) / 0.7)
            }
        }
        LinearShrink => p.start_size + (p.end_size - p.start_size) * progress,
        PopFade => {
            if progress < 0.15 {
                p.peak_size
            } else {
                p.start_size * (1.0 - progress)
            }
        }
        Constant => p.start_size,
    }
}

fn evaluate_alpha_curve(p: &Particle, progress: f32) -> f32 {
    if progress < 0.2 {
        p.start_alpha + (p.peak_alpha - p.start_alpha) * (progress / 0.2)
    } else {
        p.peak_alpha - (p.peak_alpha - p.end_alpha) * ((progress - 0.2) / 0.8)
    }
}

fn sample_track(kfs: &[EmitterKeyframe], t: f32) -> (f32, f32, f32, f32) {
    if kfs.is_empty() {
        return (0.0, 0.0, 0.0, 0.0);
    }
    if t <= kfs[0].time {
        let k = &kfs[0];
        return (k.x, k.y, k.vx.unwrap_or(0.0), k.vy.unwrap_or(0.0));
    }
    for w in kfs.windows(2) {
        let (a, b) = (&w[0], &w[1]);
        if t >= a.time && t <= b.time {
            let span = (b.time - a.time).max(f32::EPSILON);
            let f = (t - a.time) / span;
            let x = a.x + (b.x - a.x) * f;
            let y = a.y + (b.y - a.y) * f;
            let vx = a.vx.unwrap_or((b.x - a.x) / span);
            let vy = a.vy.unwrap_or((b.y - a.y) / span);
            return (x, y, vx, vy);
        }
    }
    let k = kfs.last().unwrap();
    (k.x, k.y, k.vx.unwrap_or(0.0), k.vy.unwrap_or(0.0))
}

/// Mixed into the seed for the colour RNG, so its sequence is unrelated to
/// the motion RNG's while both still follow from the one seed a host sets.
const COLOR_SEED_SALT: u64 = 0xC0_10_52_5E_ED_00_00_01;

fn color_rng(seed: u64) -> Rng {
    Rng::new(seed ^ COLOR_SEED_SALT)
}

/// Parses `config.color_stops` into a sorted `Palette`. Offsets are
/// clamped into [0, 1] here as well as in `clamp_to_bounds`, by the same
/// rule (NaN is 0), so a config that skipped clamping samples the same.
fn build_palette(config: &ParticleFxConfig) -> Palette {
    let mut palette: Palette = config
        .color_stops
        .iter()
        .flatten()
        .map(|stop| (clamp_or_default(stop.offset, 0.0, 1.0, 0.0), hex_to_rgb(&stop.color)))
        .collect();
    palette.sort_by(|a, b| a.0.total_cmp(&b.0));
    palette
}

/// The config's `emitter_track` prepared for replay: triggers put in
/// chronological order (a stable sort, so triggers authored at the same
/// time keep their authoring order) with each one's fire step resolved,
/// and the duration capped. Ready for `seek` to replay without
/// re-sorting or re-snapping per call.
fn build_baked_track(config: &ParticleFxConfig) -> Option<Arc<BakedTrack>> {
    let track = config.emitter_track.as_ref()?;

    // `clamp_to_bounds` enforces these ranges for well-behaved hosts, but
    // `Simulation` can be built directly from unvalidated JSON, so the
    // ceilings must hold regardless.
    let duration_cap = clamp_or_default(track.duration, 0.0, MAX_EMITTER_TRACK_DURATION, 0.0);
    let preroll = sanitize_preroll(track.preroll);
    let preroll_steps = grid_step(preroll);

    // `sample_track` assumes ascending time. Negative times (schema 5) make
    // the authoring order free: a pre-roll keyframe is naturally appended
    // after the t >= 0 ones, and a negative keyframe left at the end would
    // become the path's last point. A NaN time has no place on the path,
    // and leaving it in would make the partial-order comparison
    // non-transitive, which the standard sort may panic on. A stable sort,
    // like the triggers'. A track with no pre-roll and no negative time is
    // one a schema 4 file could hold, so it keeps its keyframes exactly as
    // authored and plays as it always did, out-of-order keyframes included.
    let mut keyframes = track.keyframes.clone();
    if preroll_steps > 0 || keyframes.iter().any(|k| k.time < 0.0) {
        keyframes.retain(|k| !k.time.is_nan());
        keyframes.sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap_or(std::cmp::Ordering::Equal));
    }

    let mut triggers = track.triggers.clone();
    // A NaN time makes the partial-order sort non-transitive and would fire
    // at t=0, so it is dropped. The infinities keep their meaning: -inf
    // (what serde reads from a JSON time below about -3.4e38) is "before
    // the start" and fires at the reset, +inf saturates and never fires.
    triggers.retain(|t| !t.time.is_nan());
    triggers.sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap_or(std::cmp::Ordering::Equal));
    // `signed_grid_step` is monotonic in its argument, so chronological
    // order is also fire-step order. `preroll` is on the grid, so a
    // trigger at or before the start of the pre-roll lands on step 0 or
    // below and fires at the reset. Saturating, like `grid_step`: an
    // unclamped time near `i64::MAX` steps must neither overflow the add
    // nor wrap back onto the track.
    let triggers = triggers
        .iter()
        .map(|trig| {
            let fire_step = (preroll_steps as i64)
                .saturating_add(signed_grid_step(trig.time))
                .clamp(0, u32::MAX as i64) as u32;
            BakedTrigger { fire_step, kind: trig.kind }
        })
        .collect();

    Some(Arc::new(BakedTrack { keyframes, triggers, duration_cap, preroll, preroll_steps }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::*;

    /// A config with every physics/emission term zeroed out, so individual
    /// tests can turn on exactly the one term they're exercising.
    fn base_config() -> ParticleFxConfig {
        ParticleFxConfig {
            schema_version: SCHEMA_VERSION,
            id: "test".into(),
            name: "Test".into(),
            category: Category::Custom,
            description: "".into(),
            author: None,
            icon: "".into(),
            emitter: EmitterConfig {
                spawn_rate_while_active: 0.0,
                spawn_burst_size: 0,
                spawn_rate_idle: 0.0,
                emission_pattern: EmissionPattern::DirectionalCone,
                emission_angle: 0.0,
                emission_spread: 0.0,
                velocity_inheritance: 0.0,
            },
            emitter_track: None,
            shape: ParticleShape::Circle,
            blend_mode: BlendMode::SourceOver,
            glow_bloom: false,
            glow_radius: 0.0,
            initial_speed_min: 0.0,
            initial_speed_max: 0.0,
            gravity_x: 0.0,
            gravity_y: 0.0,
            drag: 1.0,
            turbulence: 0.0,
            vortex_attraction: 0.0,
            rotation_speed_min: 0.0,
            rotation_speed_max: 0.0,
            spin_direction: SpinDirection::Fixed,
            lifetime_min: 100.0,
            lifetime_max: 100.0,
            cull_margin: None,
            start_size: 10.0,
            peak_size: 10.0,
            end_size: 0.0,
            size_curve: SizeCurve::LinearShrink,
            color_mode: ColorMode::Single,
            primary_color: "#ff0000".into(),
            secondary_color: "#00ff00".into(),
            accent_color: "#0000ff".into(),
            color_stops: None,
            rainbow_speed: 0.0,
            start_alpha: 1.0,
            peak_alpha: 1.0,
            end_alpha: 0.0,
            sound_on_spawn: None,
        }
    }

    const TICK: f32 = 1.0 / 60.0;

    #[test]
    fn continuous_emission_spawns_by_frame_equivalent_budget() {
        let mut config = base_config();
        config.emitter.spawn_rate_while_active = 10.0;
        let mut sim = Simulation::new(config, 1);

        sim.set_emitter(0.0, 0.0, 0.0, 0.0, true);
        sim.advance(TICK); // fe == 1.0 exactly, so budget == 10.0

        assert_eq!(sim.particle_count(), 10);
    }

    #[test]
    fn long_steps_spawn_at_the_configured_rate_and_leave_no_backlog() {
        // At the longest step (0.1 s, fe = 6) the schema's top rate of 20
        // asks for 120 spawns, and all 120 come. A fixed per-step cap of 40
        // used to spawn 40 and carry the other 80, so the next 1/60 s tick
        // spent the backlog (40) instead of the rate (20).
        let mut config = base_config();
        config.emitter.spawn_rate_while_active = 20.0;
        let mut sim = Simulation::new(config, 1);
        sim.set_emitter(0.0, 0.0, 0.0, 0.0, true);
        sim.advance(MAX_DELTA_SECONDS);
        sim.advance(MAX_DELTA_SECONDS);
        assert_eq!(sim.particle_count(), 2 * 120);

        sim.advance(TICK);
        assert_eq!(sim.particle_count(), 2 * 120 + 20);
    }

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

    #[test]
    fn a_frame_a_little_past_one_is_one_step_and_longer_ones_split_evenly() {
        let mut config = base_config();
        config.emitter.spawn_rate_while_active = 20.0;

        // fe 1.05 is one step: every particle has aged the whole step.
        let mut sim = Simulation::new(config.clone(), 1);
        sim.set_emitter(0.0, 0.0, 0.0, 0.0, true);
        sim.advance(1.05 / 60.0);
        assert_eq!(sim.particle_count(), 21);
        for p in sim.live.pool.iter() {
            assert!((p.life - 1.05).abs() < 1e-4, "{}", p.life);
        }

        // fe 2.5 is three equal steps of ~0.8333; the ages are 3, 2, 1 of them.
        let mut sim = Simulation::new(config, 1);
        sim.set_emitter(0.0, 0.0, 0.0, 0.0, true);
        sim.advance(2.5 / 60.0);
        let mut ages: Vec<f32> = sim.live.pool.iter().map(|p| p.life).collect();
        ages.sort_by(f32::total_cmp);
        ages.dedup_by(|a, b| (*a - *b).abs() < 1e-4);
        let h = 2.5 / 3.0;
        assert_eq!(ages.len(), 3, "{ages:?}");
        for (age, k) in ages.iter().zip([1.0, 2.0, 3.0]) {
            assert!((age - k * h).abs() < 1e-4, "{ages:?}");
        }
        // 20 * 2.5 = 50 asked for; the running f32 sum of three 16.67s can
        // land a hair under 50, so the last spawn may carry to the next call.
        assert!((49..=50).contains(&sim.particle_count()), "{}", sim.particle_count());
    }

    #[test]
    fn the_fraction_of_a_spawn_carries_across_long_steps() {
        // 6.9 * fe 6 = 41.4 per step: 41, 41 (41.8), 42 (42.2).
        let mut config = base_config();
        config.emitter.spawn_rate_while_active = 6.9;
        let mut sim = Simulation::new(config, 1);
        sim.set_emitter(0.0, 0.0, 0.0, 0.0, true);
        for _ in 0..3 {
            sim.advance(MAX_DELTA_SECONDS);
        }
        assert_eq!(sim.particle_count(), 41 + 41 + 42);
    }

    #[test]
    fn short_steps_keep_the_rate() {
        // fe 0.1 at rate 2 is 0.2 per step: a spawn every 5th step, so 600
        // steps (60 frame-equivalents) make 120 -- give or take one to f32
        // rounding in the running sum.
        let mut config = base_config();
        config.emitter.spawn_rate_while_active = 2.0;
        config.lifetime_min = 1000.0;
        config.lifetime_max = 1000.0;
        let mut sim = Simulation::new(config, 1);
        sim.set_emitter(0.0, 0.0, 0.0, 0.0, true);
        for _ in 0..600 {
            sim.advance(TICK / 10.0);
        }
        assert!((119..=120).contains(&sim.particle_count()), "{}", sim.particle_count());
    }

    #[test]
    fn idle_emission_spawns_when_rate_guarantees_it() {
        let mut config = base_config();
        // spawn_rate_idle * 0.3 > 1.0 guarantees rng.f32() < threshold every
        // call, regardless of the RNG's actual draw.
        config.emitter.spawn_rate_idle = 4.0;
        let mut sim = Simulation::new(config, 1);

        sim.set_emitter(0.0, 0.0, 0.0, 0.0, false);
        sim.advance(TICK);

        assert_eq!(sim.particle_count(), 1);
    }

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

    #[test]
    fn directional_cone_with_zero_spread_fires_along_emission_angle() {
        let mut config = base_config();
        // Use continuous emission (not burst) so the configured
        // emission_pattern is honored -- trigger_burst always forces
        // radial-burst, same as Mouseflare's isBurst branch.
        config.emitter.spawn_rate_while_active = 1.0;
        config.emitter.emission_pattern = EmissionPattern::DirectionalCone;
        config.emitter.emission_angle = 0.0; // straight +x
        config.emitter.emission_spread = 0.0;
        config.initial_speed_min = 5.0;
        config.initial_speed_max = 5.0;
        let mut sim = Simulation::new(config, 1);

        sim.set_emitter(0.0, 0.0, 0.0, 0.0, true);
        sim.advance(TICK); // budget == 1.0 => spawns exactly 1 particle, plus one physics tick
        sim.set_emitter(0.0, 0.0, 0.0, 0.0, false); // stop further spawning
        let x0 = sim.buffer()[0].x;
        sim.advance(TICK); // no new spawn; vx stays 5.0 (no gravity/drag/turbulence)
        let x1 = sim.buffer()[0].x;

        // vx == 5.0 exactly (angle 0 => cos(0)=1), no gravity/drag/turbulence.
        assert!((x1 - x0 - 5.0 * TICK * 60.0).abs() < 1e-4);
    }

    #[test]
    fn gravity_accumulates_velocity_each_tick() {
        let mut config = base_config();
        config.emitter.spawn_burst_size = 1;
        config.gravity_x = 1.0; // *0.1 internally => +0.1 velocity per fe
        let mut sim = Simulation::new(config, 1);

        sim.trigger_burst();
        sim.advance(0.0);
        let x0 = sim.buffer()[0].x;
        sim.advance(TICK); // fe=1: vx 0 -> 0.1, dx = 0.1*1 = 0.1
        let x1 = sim.buffer()[0].x;
        sim.advance(TICK); // fe=1: vx 0.1 -> 0.2, dx = 0.2*1 = 0.2
        let x2 = sim.buffer()[0].x;

        assert!((x1 - x0 - 0.1).abs() < 1e-4, "delta1 was {}", x1 - x0);
        assert!((x2 - x1 - 0.2).abs() < 1e-4, "delta2 was {}", x2 - x1);
    }

    #[test]
    fn particle_expires_after_max_life() {
        let mut config = base_config();
        config.emitter.spawn_burst_size = 1;
        // The shortest lifetime `clamp_to_bounds` allows.
        config.lifetime_min = 10.0;
        config.lifetime_max = 10.0;
        let mut sim = Simulation::new(config, 1);

        sim.trigger_burst();
        assert_eq!(sim.particle_count(), 1);
        for _ in 0..9 {
            sim.advance(TICK); // life up to 9, still alive
        }
        assert_eq!(sim.particle_count(), 1);
        sim.advance(TICK); // life 9 -> 10, 10 >= max_life => removed
        assert_eq!(sim.particle_count(), 0);
    }

    #[test]
    fn trigger_burst_immediately_rebuilds_the_buffer() {
        let mut config = base_config();
        config.emitter.spawn_burst_size = 5;
        let mut sim = Simulation::new(config, 1);

        sim.trigger_burst();

        assert_eq!(sim.particle_count(), 5);
        assert_eq!(
            sim.buffer().len(),
            sim.particle_count(),
            "buffer() should already reflect the spawned particles right after trigger_burst"
        );
    }

    #[test]
    fn a_non_finite_emitter_velocity_does_not_poison_speed_responsive_color() {
        let mut config = base_config();
        config.emitter.spawn_burst_size = 5;
        config.color_mode = ColorMode::SpeedResponsive;
        let mut sim = Simulation::new(config, 1);

        sim.set_emitter(0.0, 0.0, f32::NAN, 0.0, true);
        sim.trigger_burst();
        sim.advance(TICK);

        for p in sim.buffer() {
            for channel in p.color {
                assert!(channel.is_finite(), "non-finite color channel: {:?}", p.color);
            }
        }
    }

    #[test]
    fn a_bad_drag_from_a_config_that_skipped_clamping_leaves_positions_finite() {
        // A negative drag is NaN under `powf` at a non-integer frame
        // equivalent, as at 144 Hz.
        for drag in [f32::NAN, -0.5] {
            let mut config = base_config();
            config.emitter.spawn_burst_size = 5;
            config.drag = drag;
            let mut sim = Simulation::new(config, 1);

            sim.set_emitter(0.0, 0.0, 0.0, 0.0, true);
            sim.trigger_burst();
            for _ in 0..10 {
                sim.advance(1.0 / 144.0);
            }
            assert!(sim.particle_count() > 0);
            for p in sim.buffer() {
                assert!(p.x.is_finite() && p.y.is_finite(), "drag {drag}: non-finite position: {p:?}");
            }
        }
    }

    #[test]
    fn a_bad_spawn_rate_does_not_leave_a_bad_budget_for_the_next_config() {
        // `set_config` keeps the spawn budget. A NaN rate used to make it
        // NaN, so a valid config afterwards never spawned; a negative one
        // drove it ever lower, and +MAX then -MAX made it inf - inf.
        for rates in [vec![f32::NAN], vec![-20.0], vec![f32::MAX, -f32::MAX]] {
            let mut sim = Simulation::new(base_config(), 1);
            sim.set_emitter(0.0, 0.0, 0.0, 0.0, true);
            for rate in &rates {
                let mut config = base_config();
                config.emitter.spawn_rate_while_active = *rate;
                sim.set_config(config);
                for _ in 0..60 {
                    sim.advance(TICK);
                }
            }

            let mut valid = base_config();
            valid.emitter.spawn_rate_while_active = 10.0;
            sim.set_config(valid);
            let before = sim.particle_count();
            sim.advance(TICK);
            assert_eq!(sim.particle_count(), (before + 10).min(MAX_PARTICLES), "after {rates:?}");
        }
    }

    #[test]
    fn size_alpha_and_gradient_color_match_expected_values_at_half_life() {
        let mut config = base_config();
        config.emitter.spawn_burst_size = 1;
        config.size_curve = SizeCurve::LinearShrink;
        config.start_size = 10.0;
        config.end_size = 0.0;
        config.start_alpha = 1.0;
        config.peak_alpha = 1.0;
        config.end_alpha = 0.0;
        config.color_mode = ColorMode::GradientLifetime;
        config.primary_color = "#ff0000".into();
        config.secondary_color = "#00ff00".into();
        config.accent_color = "#0000ff".into();
        config.lifetime_min = 100.0;
        config.lifetime_max = 100.0;
        let mut sim = Simulation::new(config, 1);

        sim.trigger_burst();
        for _ in 0..50 {
            sim.advance(TICK); // fe=1 each tick => life reaches 50, progress=0.5
        }

        let p = sim.buffer()[0];
        assert!((p.size - 5.0).abs() < 1e-3, "size was {}", p.size);
        assert!((p.color[3] - 0.625).abs() < 1e-3, "alpha was {}", p.color[3]);
        // progress==0.5 falls in the secondary->accent half at factor 0 =>
        // exactly secondary (green).
        assert!((p.color[0] - 0.0).abs() < 1e-3, "r was {}", p.color[0]);
        assert!((p.color[1] - 1.0).abs() < 1e-3, "g was {}", p.color[1]);
        assert!((p.color[2] - 0.0).abs() < 1e-3, "b was {}", p.color[2]);
    }

    fn stop(offset: f32, color: &str) -> ColorStop {
        ColorStop { offset, color: color.into() }
    }

    /// One particle with a 100-frame life, so `ticks` advances = progress
    /// in percent. Returns the particle's RGB after that many ticks.
    fn rgb_after(config: ParticleFxConfig, ticks: usize) -> [f32; 3] {
        let mut sim = Simulation::new(config, 1);
        sim.trigger_burst();
        for _ in 0..ticks {
            sim.advance(TICK);
        }
        let c = sim.buffer()[0].color;
        [c[0], c[1], c[2]]
    }

    fn multi_palette_config(stops: Option<Vec<ColorStop>>) -> ParticleFxConfig {
        let mut config = base_config();
        config.emitter.spawn_burst_size = 1;
        config.lifetime_min = 100.0;
        config.lifetime_max = 100.0;
        config.color_mode = ColorMode::MultiPalette;
        config.primary_color = "#ff0000".into();
        config.color_stops = stops;
        config
    }

    fn assert_rgb(actual: [f32; 3], expected: [f32; 3]) {
        for i in 0..3 {
            assert!(
                (actual[i] - expected[i]).abs() < 1e-3,
                "channel {i}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    #[test]
    fn multi_palette_lands_on_the_middle_stop_at_half_life() {
        let config = multi_palette_config(Some(vec![
            stop(0.0, "#ff0000"),
            stop(0.5, "#00ff00"),
            stop(1.0, "#0000ff"),
        ]));
        assert_rgb(rgb_after(config, 50), [0.0, 1.0, 0.0]);
    }

    #[test]
    fn multi_palette_interpolates_between_adjacent_stops() {
        let config = multi_palette_config(Some(vec![
            stop(0.0, "#ff0000"),
            stop(0.5, "#00ff00"),
            stop(1.0, "#0000ff"),
        ]));
        // progress 0.25 is halfway from red to green
        assert_rgb(rgb_after(config, 25), [0.5, 0.5, 0.0]);
    }

    #[test]
    fn multi_palette_holds_the_last_stop_past_its_offset() {
        let config = multi_palette_config(Some(vec![
            stop(0.0, "#ff0000"),
            stop(0.5, "#00ff00"),
        ]));
        assert_rgb(rgb_after(config, 75), [0.0, 1.0, 0.0]);
    }

    #[test]
    fn multi_palette_holds_the_first_stop_before_its_offset() {
        let config = multi_palette_config(Some(vec![
            stop(0.5, "#00ff00"),
            stop(1.0, "#0000ff"),
        ]));
        assert_rgb(rgb_after(config, 25), [0.0, 1.0, 0.0]);
    }

    #[test]
    fn multi_palette_sorts_stops_by_offset_before_sampling() {
        let config = multi_palette_config(Some(vec![
            stop(1.0, "#0000ff"),
            stop(0.0, "#ff0000"),
            stop(0.5, "#00ff00"),
        ]));
        assert_rgb(rgb_after(config, 50), [0.0, 1.0, 0.0]);
    }

    #[test]
    fn multi_palette_without_stops_uses_the_primary_color() {
        assert_rgb(rgb_after(multi_palette_config(None), 50), [1.0, 0.0, 0.0]);
        assert_rgb(rgb_after(multi_palette_config(Some(vec![])), 50), [1.0, 0.0, 0.0]);
    }

    #[test]
    fn set_config_picks_up_new_color_stops() {
        let mut sim = Simulation::new(multi_palette_config(None), 1);
        sim.set_config(multi_palette_config(Some(vec![
            stop(0.0, "#ff0000"),
            stop(1.0, "#0000ff"),
        ])));
        sim.trigger_burst();
        for _ in 0..50 {
            sim.advance(TICK);
        }
        let c = sim.buffer()[0].color;
        assert_rgb([c[0], c[1], c[2]], [0.5, 0.0, 0.5]);
    }

    fn track_config() -> ParticleFxConfig {
        let mut config = base_config();
        config.emitter.spawn_burst_size = 1;
        config.emitter_track = Some(EmitterTrack {
            duration: 1.0,
            preroll: 0.0,
            keyframes: vec![
                EmitterKeyframe { time: 0.0, x: 0.0, y: 0.0, vx: Some(0.0), vy: Some(0.0) },
                EmitterKeyframe { time: 1.0, x: 60.0, y: 0.0, vx: Some(60.0), vy: Some(0.0) },
            ],
            triggers: vec![EmitterTrigger { time: 0.5, kind: TriggerKind::Burst }],
        });
        config
    }

    #[test]
    fn seek_fires_burst_trigger_and_samples_track_position() {
        let mut sim = Simulation::new(track_config(), 5);
        sim.seek(0.5);

        assert_eq!(sim.particle_count(), 1);
        let x = sim.buffer()[0].x;
        // Track puts the emitter at x=30 at t=0.5; spawn jitter is +/-2px.
        assert!((x - 30.0).abs() < 2.5, "x was {x}");
    }

    #[test]
    fn seek_is_deterministic_for_the_same_time() {
        // Two handles rather than one: re-seeking the same handle to the
        // same time applies zero steps, so it would pass however wrong
        // the replay is. The second handle takes the backward path
        // (0.75 then 0.5) so the two reach 0.5 by different routes.
        let mut fresh = Simulation::new(track_config(), 5);
        fresh.seek(0.5);

        let mut rewound = Simulation::new(track_config(), 5);
        rewound.seek(0.75);
        rewound.seek(0.5);

        assert!(fresh.particle_count() > 0, "test is vacuous with no particles");
        assert_eq!(buffer_bits(&fresh), buffer_bits(&rewound));
    }

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
        // tolerance rather than bitwise. 1e-3 is half the fixtures' libm
        // drift tolerance (`LIBM_DRIFT_TOLERANCE`, 2e-3); the rounding here
        // is far below either.
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

    #[test]
    fn a_replay_that_resumes_from_the_saved_preroll_is_bit_identical() {
        // seek(1.0) runs through the pre-roll and saves its end state; each
        // later backward seek restarts from it and must match a fresh
        // handle. t=0 is the end of the pre-roll itself, where the replay
        // runs no step and the buffer is rebuilt from the restored state.
        // A live call
        // in between leaves the saved state valid: it depends only on the
        // config, the seed, and the bounds.
        let config = prerolled_config(0.5, 0.0, -30.0, 60.0);
        let mut sim = Simulation::new(config.clone(), 5);
        sim.seek(1.0);
        assert!(sim.preroll_saved, "a seek through the pre-roll saves its end");
        for time in [0.75f32, 0.0, 0.3, 0.6] {
            sim.trigger_burst();
            sim.advance(PLAYBACK_STEP);
            sim.seek(time);
            let mut fresh = Simulation::new(config.clone(), 5);
            fresh.seek(time);
            assert!(fresh.particle_count() > 0, "vacuous at {time}");
            assert_eq!(buffer_bits(&sim), buffer_bits(&fresh), "t={time}");
        }
    }

    #[test]
    fn a_new_config_drops_the_saved_preroll() {
        let mut sim = Simulation::new(prerolled_config(0.5, 0.0, -30.0, 60.0), 5);
        sim.seek(1.0);
        let other = prerolled_config(0.5, 0.0, 200.0, 100.0);
        sim.set_config(other.clone());
        assert!(!sim.preroll_saved);
        sim.seek(0.25);
        let mut fresh = Simulation::new(other, 5);
        fresh.seek(0.25);
        assert_eq!(buffer_bits(&sim), buffer_bits(&fresh));
    }

    #[test]
    fn new_bounds_drop_the_saved_preroll_and_the_same_bounds_keep_it() {
        // The emitter runs from x=-30 to 60 and the particles fly off, so
        // a 20 px frame with no margin culls much of what the unbounded
        // pre-roll kept.
        let mut config = prerolled_config(0.5, 0.0, -30.0, 60.0);
        config.cull_margin = Some(0.0);
        config.initial_speed_min = 2.0;
        config.initial_speed_max = 2.0;
        let mut sim = Simulation::new(config.clone(), 5);
        sim.seek(1.0);
        sim.set_bounds(20.0, 20.0).unwrap();
        assert!(!sim.preroll_saved);
        sim.seek(1.0);
        sim.set_bounds(20.0, 20.0).unwrap();
        assert!(sim.preroll_saved, "unchanged bounds keep it");
        sim.seek(0.25);
        let mut fresh = Simulation::new(config.clone(), 5);
        fresh.set_bounds(20.0, 20.0).unwrap();
        fresh.seek(0.25);
        let mut unbounded = Simulation::new(config, 5);
        unbounded.seek(0.25);
        assert_ne!(fresh.particle_count(), unbounded.particle_count(), "the bounds must matter for this to test anything");
        assert_eq!(buffer_bits(&sim), buffer_bits(&fresh));
    }

    #[test]
    fn restoring_the_saved_preroll_keeps_the_buffer_where_it_is() {
        // The buffer's pointer crosses the FFI; a restore must copy into
        // it, not swap in the saved Vec.
        let mut sim = Simulation::new(prerolled_config(0.5, 0.0, -30.0, 60.0), 5);
        let ptr = sim.buffer().as_ptr();
        sim.seek(1.0);
        sim.seek(0.25);
        assert_eq!(sim.buffer().as_ptr(), ptr);
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
    fn a_trigger_far_beyond_any_track_never_fires_without_preroll() {
        // 71582792.0 s is exact in f32 and is 2^32 + 224 grid steps: a
        // wrapping i64 -> u32 cast would fire it at step 224 (3.73 s), so
        // the track is lengthened to let `seek` reach that far.
        let mut config = burst_only_at(0.0, 71582792.0);
        config.emitter_track.as_mut().unwrap().duration = 10.0;
        let mut sim = Simulation::new(config, 5);
        sim.seek(4.0); // particles live 100 steps: stop just past step 224
        assert_eq!(sim.particle_count(), 0);
    }

    #[test]
    fn an_infinite_trigger_time_neither_panics_nor_fires() {
        let mut config = burst_only_at(0.5, f32::MAX);
        config
            .emitter_track
            .as_mut()
            .unwrap()
            .triggers
            .push(EmitterTrigger { time: f32::INFINITY, kind: TriggerKind::Burst });
        let mut sim = Simulation::new(config, 5);
        sim.seek(1.0);
        assert_eq!(sim.particle_count(), 0);
    }

    #[test]
    fn an_off_grid_preroll_runs_as_the_grid_preroll_above_it() {
        // 0.505 s is 30.3 steps; the pre-roll is 31 whole steps, starting
        // on the grid at -31/60 s, as if it had been authored that way. The
        // burst fires at the reset, where the emitter is placed at the
        // start of the pre-roll; it moves fast there (x=0 at -0.52 s to
        // x=1000 at -0.4 s), so placing it at -0.505 s instead would show.
        let make = |preroll: f32| {
            let mut config = burst_only_at(preroll, -1.0);
            config.emitter_track.as_mut().unwrap().keyframes = vec![
                EmitterKeyframe { time: -0.52, x: 0.0, y: 0.0, vx: Some(0.0), vy: Some(0.0) },
                EmitterKeyframe { time: -0.4, x: 1000.0, y: 0.0, vx: Some(0.0), vy: Some(0.0) },
            ];
            let mut sim = Simulation::new(config, 5);
            sim.seek(0.25);
            sim
        };
        let (off, on) = (make(0.505), make((31.0f64 / 60.0) as f32));
        assert_eq!(on.particle_count(), 3, "vacuous");
        assert_eq!(buffer_bits(&off), buffer_bits(&on));
    }

    #[test]
    fn a_negative_keyframe_without_a_preroll_is_sorted_into_place() {
        // Schema 5 allows a negative keyframe on a track with no pre-roll.
        // Appended last and left there, it would be the path's end, and
        // the emitter would jump to x=-60 once t passes 1.
        let mut config = burst_only_at(0.0, 1.5);
        let track = config.emitter_track.as_mut().unwrap();
        track.duration = 2.0;
        track.keyframes = vec![
            EmitterKeyframe { time: 0.0, x: 0.0, y: 0.0, vx: Some(0.0), vy: Some(0.0) },
            EmitterKeyframe { time: 1.0, x: 60.0, y: 0.0, vx: Some(0.0), vy: Some(0.0) },
            EmitterKeyframe { time: -1.0, x: -60.0, y: 0.0, vx: Some(0.0), vy: Some(0.0) },
        ];
        let mut sim = Simulation::new(config, 5);
        sim.seek(1.5);
        assert_eq!(sim.particle_count(), 3);
        for p in sim.buffer() {
            assert!((p.x - 60.0).abs() < 2.5, "x was {}", p.x);
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

    fn random_palette_config(stops: Option<Vec<ColorStop>>) -> ParticleFxConfig {
        let mut config = base_config();
        config.emitter.spawn_burst_size = 40;
        config.lifetime_min = 100.0;
        config.lifetime_max = 100.0;
        config.color_mode = ColorMode::RandomPalette;
        config.primary_color = "#ffffff".into();
        config.color_stops = stops;
        config
    }

    fn colors(sim: &Simulation) -> Vec<[f32; 3]> {
        sim.buffer().iter().map(|p| [p.color[0], p.color[1], p.color[2]]).collect()
    }

    #[test]
    fn random_palette_gives_each_particle_one_stop_and_holds_it_for_life() {
        let stops = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        let mut sim = Simulation::new(
            random_palette_config(Some(vec![
                stop(0.0, "#ff0000"),
                stop(0.5, "#00ff00"),
                stop(1.0, "#0000ff"),
            ])),
            3,
        );
        sim.trigger_burst();
        sim.advance(TICK);
        let born = colors(&sim);
        assert_eq!(born.len(), 40);
        for c in &born {
            assert!(stops.contains(c), "{c:?} is not a stop, so it was interpolated");
        }
        // Every stop is used: offsets do not weight the pick.
        for s in &stops {
            assert!(born.contains(s), "stop {s:?} never picked out of 40");
        }
        // Unlike multi-palette, the colour does not move with progress.
        for _ in 0..80 {
            sim.advance(TICK);
        }
        assert_eq!(colors(&sim), born);
    }

    #[test]
    fn random_palette_without_stops_uses_the_primary_color() {
        for stops in [None, Some(vec![])] {
            let mut sim = Simulation::new(random_palette_config(stops), 3);
            sim.trigger_burst();
            sim.advance(TICK);
            assert!(colors(&sim).iter().all(|c| *c == [1.0, 1.0, 1.0]));
        }
    }

    #[test]
    fn random_palette_is_deterministic_under_seek() {
        // The pick comes from the seeded RNG at spawn, so a replay must
        // land every particle on the same stop, by either route.
        let mut config = track_config();
        config.emitter.spawn_burst_size = 30;
        config.lifetime_min = 100.0;
        config.lifetime_max = 100.0;
        config.color_mode = ColorMode::RandomPalette;
        config.color_stops = Some(vec![stop(0.0, "#ff0000"), stop(0.5, "#00ff00"), stop(1.0, "#0000ff")]);

        let mut fresh = Simulation::new(config.clone(), 5);
        fresh.seek(0.9);
        let mut rewound = Simulation::new(config, 5);
        rewound.seek(0.95);
        rewound.seek(0.6);
        rewound.seek(0.9);

        assert_eq!(fresh.particle_count(), 30, "test is vacuous without the burst");
        assert_eq!(fresh.buffer().to_vec(), rewound.buffer().to_vec());
        assert!(
            colors(&fresh).windows(2).any(|w| w[0] != w[1]),
            "all 30 particles were dealt the same stop, so the pick is not random"
        );
    }

    /// A config whose every RNG-driven motion field is a range, so a
    /// shifted random sequence would show in the positions.
    fn motion_config() -> ParticleFxConfig {
        let mut config = base_config();
        config.emitter.spawn_rate_while_active = 2.5;
        config.emitter.emission_spread = 90.0;
        config.initial_speed_min = 1.0;
        config.initial_speed_max = 4.0;
        config.turbulence = 1.0;
        config.rotation_speed_min = 0.01;
        config.rotation_speed_max = 0.1;
        config.lifetime_min = 80.0;
        config.lifetime_max = 120.0;
        config.primary_color = "#ffffff".into();
        config
    }

    fn with_colors(mut config: ParticleFxConfig, mode: ColorMode, stops: Option<Vec<ColorStop>>) -> ParticleFxConfig {
        config.color_mode = mode;
        config.color_stops = stops;
        config
    }

    fn rgb_stops() -> Option<Vec<ColorStop>> {
        Some(vec![stop(0.0, "#ff0000"), stop(0.5, "#00ff00"), stop(1.0, "#0000ff")])
    }

    /// Everything about each particle except its colour.
    fn motion(sim: &Simulation) -> Vec<[f32; 4]> {
        sim.buffer().iter().map(|p| [p.x, p.y, p.size, p.rotation]).collect()
    }

    fn run(sim: &mut Simulation, ticks: usize) {
        sim.set_emitter(50.0, 50.0, 0.0, 0.0, true);
        for _ in 0..ticks {
            sim.advance(TICK);
        }
    }

    #[test]
    fn the_colour_mode_and_stops_never_move_a_particle() {
        // Colour and motion draw from separate RNGs, so switching a config
        // into random-palette, or giving it its first stop, leaves every
        // particle exactly where it would have been.
        let reference = {
            let mut sim = Simulation::new(with_colors(motion_config(), ColorMode::MultiPalette, rgb_stops()), 9);
            run(&mut sim, 90);
            motion(&sim)
        };
        assert!(reference.len() > 100, "test is vacuous with {} particles", reference.len());
        for (mode, stops) in [
            (ColorMode::RandomPalette, rgb_stops()),
            (ColorMode::RandomPalette, None),
            (ColorMode::Single, None),
            (ColorMode::RainbowCycle, None),
        ] {
            let mut sim = Simulation::new(with_colors(motion_config(), mode, stops.clone()), 9);
            run(&mut sim, 90);
            assert!(motion(&sim) == reference, "{mode:?} with {stops:?} moved particles");
        }
    }

    #[test]
    fn a_live_switch_into_random_palette_never_moves_a_particle() {
        let mut steady = Simulation::new(with_colors(motion_config(), ColorMode::MultiPalette, rgb_stops()), 9);
        run(&mut steady, 90);

        let mut switched = Simulation::new(with_colors(motion_config(), ColorMode::MultiPalette, rgb_stops()), 9);
        run(&mut switched, 30);
        switched.set_config(with_colors(motion_config(), ColorMode::RandomPalette, None));
        run(&mut switched, 30);
        switched.set_config(with_colors(motion_config(), ColorMode::RandomPalette, rgb_stops()));
        run(&mut switched, 30);

        assert!(motion(&switched) == motion(&steady), "a colour-only set_config moved particles");
    }

    #[test]
    fn editing_the_stops_recolours_live_random_palette_particles() {
        let mut sim = Simulation::new(random_palette_config(rgb_stops()), 3);
        sim.trigger_burst();
        sim.advance(TICK);
        let before = colors(&sim);

        // Same count, new colours: each particle keeps its place in the
        // palette and takes that stop's new colour on the next frame.
        sim.set_config(random_palette_config(Some(vec![
            stop(0.0, "#00ffff"),
            stop(0.5, "#ff00ff"),
            stop(1.0, "#ffff00"),
        ])));
        sim.advance(TICK);

        let mapping = |c: [f32; 3]| match c {
            [1.0, 0.0, 0.0] => [0.0, 1.0, 1.0],
            [0.0, 1.0, 0.0] => [1.0, 0.0, 1.0],
            [0.0, 0.0, 1.0] => [1.0, 1.0, 0.0],
            other => panic!("{other:?} was not a stop"),
        };
        assert_eq!(colors(&sim), before.into_iter().map(mapping).collect::<Vec<_>>());
    }

    #[test]
    fn switching_into_random_palette_recolours_live_particles() {
        let mut config = random_palette_config(None);
        config.color_mode = ColorMode::Single;
        let mut sim = Simulation::new(config, 3);
        sim.trigger_burst();
        sim.advance(TICK);
        assert!(colors(&sim).iter().all(|c| *c == [1.0, 1.0, 1.0]));

        sim.set_config(random_palette_config(rgb_stops()));
        sim.advance(TICK);

        let now = colors(&sim);
        let stops = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        assert!(now.iter().all(|c| stops.contains(c)), "a live particle kept a stale colour: {now:?}");
        assert!(now.windows(2).any(|w| w[0] != w[1]), "every live particle took the same stop");
    }

    fn spin_config(direction: SpinDirection) -> ParticleFxConfig {
        let mut config = base_config();
        config.emitter.spawn_burst_size = 40;
        config.lifetime_min = 100.0;
        config.lifetime_max = 100.0;
        config.rotation_speed_min = 0.04;
        config.rotation_speed_max = 0.1;
        config.spin_direction = direction;
        config
    }

    /// Each live particle's rotation speed, read off two frames.
    fn spins(sim: &mut Simulation) -> Vec<f32> {
        let before: Vec<f32> = sim.buffer().iter().map(|p| p.rotation).collect();
        sim.advance(TICK);
        sim.buffer().iter().zip(before).map(|(p, r)| p.rotation - r).collect()
    }

    #[test]
    fn fixed_spin_is_the_default_and_turns_every_particle_the_same_way() {
        assert_eq!(ParticleFxConfig::default().spin_direction, SpinDirection::Fixed);
        let mut sim = Simulation::new(spin_config(SpinDirection::Fixed), 4);
        sim.trigger_burst();
        sim.advance(TICK);
        let spins = spins(&mut sim);
        assert!(spins.iter().all(|s| (0.04 - 1e-4..=0.1 + 1e-4).contains(s)), "{spins:?}");
    }

    #[test]
    fn random_spin_turns_particles_both_ways_at_the_same_speeds() {
        let mut sim = Simulation::new(spin_config(SpinDirection::Random), 4);
        sim.trigger_burst();
        sim.advance(TICK);
        let spins = spins(&mut sim);
        assert_eq!(spins.len(), 40);
        assert!(spins.iter().all(|s| (0.04 - 1e-4..=0.1 + 1e-4).contains(&s.abs())), "{spins:?}");
        assert!(spins.iter().any(|s| *s > 0.0), "no particle spun clockwise");
        assert!(spins.iter().any(|s| *s < 0.0), "no particle spun counter-clockwise");
    }

    #[test]
    fn random_spin_is_deterministic_under_seek() {
        let mut config = spin_config(SpinDirection::Random);
        config.emitter.spawn_burst_size = 30;
        config.emitter_track = track_config().emitter_track;

        let mut fresh = Simulation::new(config.clone(), 5);
        fresh.seek(0.9);
        let mut rewound = Simulation::new(config, 5);
        rewound.seek(0.95);
        rewound.seek(0.6);
        rewound.seek(0.9);

        assert_eq!(fresh.particle_count(), 30, "test is vacuous without the burst");
        assert_eq!(fresh.buffer().to_vec(), rewound.buffer().to_vec());
        // And the replay really is of a random spin, not a fixed one.
        let spins = spins(&mut fresh);
        assert!(spins.iter().any(|s| *s < 0.0), "no particle spun backwards, so the flip never happened");
        assert!(spins.iter().any(|s| *s > 0.0), "no particle spun forwards");
    }

    #[test]
    fn a_minus_infinite_trigger_time_fires_at_the_start() {
        // serde reads any JSON time below about -3.4e38 as -inf, and a host
        // may write -Number.MAX_VALUE for "since forever". It is before the
        // pre-roll's start, so it fires at the reset like any such trigger.
        for preroll in [0.0f32, 0.5] {
            let mut sim = Simulation::new(burst_only_at(preroll, f32::NEG_INFINITY), 5);
            sim.seek(0.0);
            assert_eq!(sim.particle_count(), 3, "preroll {preroll}");
        }
    }

    #[test]
    fn a_nan_keyframe_time_is_dropped_without_panicking() {
        // A NaN time made the keyframe comparison non-transitive, and the
        // standard sort may panic on that once there are more than about
        // twenty elements; whether it does depends on the order. So: many
        // deterministic shuffles of 23 keyframes, two of them NaN. Every
        // keyframe sits at x=0, so a surviving NaN would show as a NaN x.
        let times: Vec<f32> =
            (-6..15).map(|i| i as f32).chain([f32::NAN, f32::NAN]).collect();
        let mut state = 0x2545_f491_u32;
        for _ in 0..200 {
            let mut order = times.clone();
            for i in (1..order.len()).rev() {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                order.swap(i, (state >> 8) as usize % (i + 1));
            }
            let mut config = burst_only_at(6.0, -0.5);
            config.emitter_track.as_mut().unwrap().keyframes = order
                .iter()
                .map(|&time| EmitterKeyframe { time, x: 0.0, y: 0.0, vx: Some(0.0), vy: Some(0.0) })
                .collect();
            let mut sim = Simulation::new(config, 5);
            sim.seek(0.0);
            assert_eq!(sim.particle_count(), 3, "order {order:?}");
            for p in sim.buffer() {
                assert!(p.x.abs() < 2.5, "order {order:?}: x was {}", p.x);
            }
        }
    }

    #[test]
    fn seek_with_no_track_is_a_no_op() {
        let mut sim = Simulation::new(base_config(), 1);
        sim.seek(1.0);
        assert_eq!(sim.particle_count(), 0);
    }

    #[test]
    fn a_track_with_no_preroll_samples_its_keyframes_as_authored() {
        // Before pre-roll, keyframes were never sorted: out of order, the
        // first window that brackets `t` wins. Here [0, 2] brackets t=1,
        // halfway from x=0 to x=60, where sorted keyframes would give 0.
        let mut config = burst_only_at(0.0, 1.0);
        let track = config.emitter_track.as_mut().unwrap();
        track.duration = 2.0;
        track.keyframes = vec![
            EmitterKeyframe { time: 0.0, x: 0.0, y: 0.0, vx: Some(0.0), vy: Some(0.0) },
            EmitterKeyframe { time: 2.0, x: 60.0, y: 0.0, vx: Some(0.0), vy: Some(0.0) },
            EmitterKeyframe { time: 1.0, x: 0.0, y: 0.0, vx: Some(0.0), vy: Some(0.0) },
        ];
        let mut sim = Simulation::new(config, 5);
        sim.seek(1.0);
        assert_eq!(sim.particle_count(), 3);
        for p in sim.buffer() {
            assert!((p.x - 30.0).abs() < 2.5, "x was {}", p.x);
        }
    }

    #[test]
    fn keyframes_are_sampled_in_time_order_however_authored() {
        // A pre-roll keyframe appended after the t >= 0 ones is the natural
        // edit; `sample_track` needs ascending time, so baking sorts them.
        // Sorted, the path runs x=-60 (t=-1) -> 0 (t=0) -> 60 (t=1), so a
        // burst at -0.5 s spawns at x=-30.
        let mut config = base_config();
        config.emitter.spawn_rate_while_active = 0.0;
        config.emitter.spawn_burst_size = 3;
        config.lifetime_min = 100.0;
        config.lifetime_max = 100.0;
        config.emitter_track = Some(EmitterTrack {
            duration: 1.0,
            preroll: 1.0,
            keyframes: vec![
                EmitterKeyframe { time: 0.0, x: 0.0, y: 0.0, vx: Some(0.0), vy: Some(0.0) },
                EmitterKeyframe { time: 1.0, x: 60.0, y: 0.0, vx: Some(0.0), vy: Some(0.0) },
                EmitterKeyframe { time: -1.0, x: -60.0, y: 0.0, vx: Some(0.0), vy: Some(0.0) },
            ],
            triggers: vec![EmitterTrigger { time: -0.5, kind: TriggerKind::Burst }],
        });
        let mut sim = Simulation::new(config, 3);
        sim.seek(0.0);
        assert_eq!(sim.particle_count(), 3);
        for (p, inst) in sim.buffer().iter().enumerate() {
            assert!((inst.x - -30.0).abs() < 2.5, "particle {p} at x={}, expected about -30", inst.x);
        }
    }

    #[test]
    fn a_nan_trigger_time_is_dropped() {
        // A NaN trigger used to sort as "equal" to everything and resolve to
        // step 0, firing a burst at the reset. It has no step, so it is
        // dropped.
        let mut config = base_config();
        config.emitter.spawn_rate_while_active = 2.0;
        config.emitter.spawn_burst_size = 3;
        config.lifetime_min = 100.0;
        config.lifetime_max = 100.0;
        config.emitter_track = Some(EmitterTrack {
            duration: 1.0,
            preroll: 0.0,
            keyframes: vec![
                EmitterKeyframe { time: 0.0, x: 0.0, y: 0.0, vx: Some(0.0), vy: Some(0.0) },
                EmitterKeyframe { time: 1.0, x: 0.0, y: 0.0, vx: Some(0.0), vy: Some(0.0) },
            ],
            triggers: vec![
                EmitterTrigger { time: f32::NAN, kind: TriggerKind::Burst },
                EmitterTrigger { time: 0.2, kind: TriggerKind::StopContinuous },
                EmitterTrigger { time: 0.1, kind: TriggerKind::StartContinuous },
            ],
        });
        let mut sim = Simulation::new(config, 3);
        sim.seek(0.0);
        assert_eq!(sim.particle_count(), 0, "the NaN burst must not fire at the reset");

        // Start fires at step 6 and Stop at step 12; a trigger at step k
        // applies before the step that spawns, so steps 6..12 are active:
        // 6 steps. `step` adds rate * fe = 2.0 * 1.0 to the spawn budget
        // and spawns floor(2.0) = 2 per step: 6 * 2 = 12.
        sim.seek(0.5);
        assert_eq!(sim.particle_count(), 12);
    }

    #[test]
    fn seek_start_continuous_then_stop_continuous_actually_toggles_spawning() {
        let mut config = base_config();
        config.emitter.spawn_rate_while_active = 10.0;
        config.emitter_track = Some(EmitterTrack {
            duration: 2.0,
            preroll: 0.0,
            keyframes: vec![
                EmitterKeyframe { time: 0.0, x: 0.0, y: 0.0, vx: Some(0.0), vy: Some(0.0) },
                EmitterKeyframe { time: 2.0, x: 0.0, y: 0.0, vx: Some(0.0), vy: Some(0.0) },
            ],
            triggers: vec![
                EmitterTrigger { time: 0.2, kind: TriggerKind::StartContinuous },
                EmitterTrigger { time: 1.0, kind: TriggerKind::StopContinuous },
            ],
        });
        let mut sim = Simulation::new(config, 5);

        // Before StartContinuous fires: no particles yet.
        sim.seek(0.1);
        assert_eq!(sim.particle_count(), 0, "should not spawn before StartContinuous fires");

        // During the active window: particle count should have grown.
        sim.seek(0.5);
        let count_during_active = sim.particle_count();
        assert!(count_during_active > 0, "should be spawning during the active window");

        // Well after StopContinuous: count should stop growing further.
        sim.seek(1.0);
        let count_at_stop = sim.particle_count();
        sim.seek(1.5);
        let count_after_stop = sim.particle_count();
        assert_eq!(
            count_after_stop, count_at_stop,
            "particle count should stop growing once StopContinuous has fired"
        );
    }

    #[test]
    fn seek_burst_trigger_at_time_zero_fires_on_first_step() {
        let mut config = base_config();
        config.emitter.spawn_burst_size = 3;
        config.emitter_track = Some(EmitterTrack {
            duration: 1.0,
            preroll: 0.0,
            keyframes: vec![
                EmitterKeyframe { time: 0.0, x: 0.0, y: 0.0, vx: Some(0.0), vy: Some(0.0) },
                EmitterKeyframe { time: 1.0, x: 0.0, y: 0.0, vx: Some(0.0), vy: Some(0.0) },
            ],
            triggers: vec![EmitterTrigger { time: 0.0, kind: TriggerKind::Burst }],
        });
        let mut sim = Simulation::new(config, 5);

        sim.seek(0.01);

        assert!(sim.particle_count() > 0, "a trigger authored at time 0.0 should fire immediately");
    }

    #[test]
    fn seek_zero_fires_a_burst_trigger_authored_at_time_zero() {
        let mut config = base_config();
        config.emitter.spawn_burst_size = 3;
        config.emitter_track = Some(EmitterTrack {
            duration: 1.0,
            preroll: 0.0,
            keyframes: vec![
                EmitterKeyframe { time: 0.0, x: 0.0, y: 0.0, vx: Some(0.0), vy: Some(0.0) },
                EmitterKeyframe { time: 1.0, x: 0.0, y: 0.0, vx: Some(0.0), vy: Some(0.0) },
            ],
            triggers: vec![EmitterTrigger { time: 0.0, kind: TriggerKind::Burst }],
        });
        let mut sim = Simulation::new(config, 5);

        sim.seek(0.0);

        assert!(
            sim.particle_count() > 0,
            "seek(0.0) should still fire a trigger authored at time 0.0"
        );
    }

    #[test]
    fn seek_applies_out_of_order_triggers_by_time_not_authoring_order() {
        let mut config = base_config();
        config.emitter.spawn_rate_while_active = 10.0;
        config.emitter_track = Some(EmitterTrack {
            duration: 1.0,
            preroll: 0.0,
            keyframes: vec![
                EmitterKeyframe { time: 0.0, x: 0.0, y: 0.0, vx: Some(0.0), vy: Some(0.0) },
                EmitterKeyframe { time: 1.0, x: 0.0, y: 0.0, vx: Some(0.0), vy: Some(0.0) },
            ],
            // Authored out of chronological order: StopContinuous appears
            // before the StartContinuous that precedes it in time. Both
            // resolve to fire step 19, so a single step's trigger loop
            // must evaluate them in time order to reach the correct
            // final `active` state.
            triggers: vec![
                EmitterTrigger { time: 0.305, kind: TriggerKind::StopContinuous },
                EmitterTrigger { time: 0.301, kind: TriggerKind::StartContinuous },
            ],
        });
        let mut sim = Simulation::new(config, 5);

        // 19/60 = 0.3167, the grid point both triggers snap up to.
        // Seeking to 0.30 would land on step 18 and never run the step
        // they fire in.
        sim.seek(19.0 / 60.0);

        assert_eq!(
            sim.particle_count(),
            0,
            "StopContinuous at 0.305 should win over StartContinuous at 0.301 regardless of authoring order"
        );
    }

    #[test]
    fn seek_is_capped_at_the_track_duration() {
        let mut config = base_config();
        config.emitter.spawn_burst_size = 1;
        config.emitter_track = Some(EmitterTrack {
            duration: 0.5,
            preroll: 0.0,
            keyframes: vec![
                EmitterKeyframe { time: 0.0, x: 0.0, y: 0.0, vx: Some(0.0), vy: Some(0.0) },
                EmitterKeyframe { time: 0.5, x: 0.0, y: 0.0, vx: Some(0.0), vy: Some(0.0) },
            ],
            triggers: vec![EmitterTrigger { time: 0.4, kind: TriggerKind::Burst }],
        });
        let mut sim = Simulation::new(config, 5);

        // A wildly oversized time (corrupt data, host bug, ...) must not
        // drive an unbounded number of step iterations -- seek should clamp
        // to the track's own duration instead.
        sim.seek(1_000_000.0);

        assert_eq!(sim.particle_count(), 1, "seek should have stopped advancing at track.duration");
    }

    #[test]
    fn seek_is_capped_even_when_duration_itself_is_unvalidated_and_huge() {
        // Simulation can be built directly from unvalidated JSON without
        // ever calling ParticleFxConfig::clamp_to_bounds() (a supported,
        // tested path -- see config_parses_from_a_raw_json_string_like_a
        // _host_app_would_send in tests/golden.rs), so a corrupt/malicious
        // `duration` must not be able to drive an unbounded number of step
        // iterations even though clamp_to_bounds normally bounds it.
        let mut config = base_config();
        config.emitter.spawn_burst_size = 1;
        config.emitter_track = Some(EmitterTrack {
            duration: f32::MAX,
            preroll: 0.0,
            keyframes: vec![
                EmitterKeyframe { time: 0.0, x: 0.0, y: 0.0, vx: Some(0.0), vy: Some(0.0) },
                EmitterKeyframe { time: 1.0, x: 0.0, y: 0.0, vx: Some(0.0), vy: Some(0.0) },
            ],
            triggers: vec![EmitterTrigger { time: MAX_EMITTER_TRACK_DURATION + 1.0, kind: TriggerKind::Burst }],
        });
        let mut sim = Simulation::new(config, 5);

        sim.seek(f32::MAX);

        assert_eq!(
            sim.particle_count(),
            0,
            "seek should never advance past MAX_EMITTER_TRACK_DURATION regardless of an unvalidated duration"
        );
    }

    #[test]
    fn rebuild_buffer_does_not_produce_nan_when_max_life_is_zero() {
        let mut config = base_config();
        config.lifetime_min = 0.0;
        config.lifetime_max = 0.0;
        config.color_mode = ColorMode::GradientLifetime;
        config.emitter.spawn_burst_size = 1;
        let mut sim = Simulation::new(config, 1);

        // spawn_particle + rebuild_buffer run directly here, without an
        // intervening step() that would otherwise cull a max_life == 0.0
        // particle before rebuild_buffer ever sees it.
        sim.trigger_burst();

        let p = sim.buffer()[0];
        assert!(!p.color[0].is_nan(), "r channel was NaN");
        assert!(!p.color[1].is_nan(), "g channel was NaN");
        assert!(!p.color[2].is_nan(), "b channel was NaN");
        assert!(!p.color[3].is_nan(), "alpha was NaN");
        assert!(!p.size.is_nan(), "size was NaN");
    }

    #[test]
    fn the_grid_rate_is_the_reciprocal_of_the_playback_step() {
        // GRID_RATE is written as the integer 60 rather than derived, so
        // this pins it to PLAYBACK_STEP: if the step size ever changes,
        // this fails instead of the grid silently drifting off it.
        assert_eq!((1.0 / PLAYBACK_STEP as f64).round(), GRID_RATE);
    }

    #[test]
    fn every_host_frame_time_maps_to_its_own_grid_step() {
        // A host renders frame k of an fps-rate timeline by seeking to
        // k/fps, computed in f32. That quotient can sit either side of the
        // exact grid point, and the gap grows with k: at 30 fps the first
        // divergence is around t = 256 s. Every frame of a full-length
        // track must still land on the step it names.
        for fps in [24u32, 30, 60] {
            let frames = fps * MAX_EMITTER_TRACK_DURATION as u32;
            for k in 0..=frames {
                // Ceil: at 24 fps the odd frames land halfway between two
                // grid points and must take the one *above*, so the frame
                // is never rendered before a trigger authored at its own
                // time has fired.
                let expected = (k as u64 * 60).div_ceil(fps as u64) as u32;
                assert_eq!(
                    grid_step(k as f32 / fps as f32),
                    expected,
                    "frame {k} at {fps} fps"
                );
            }
        }
    }

    #[test]
    fn every_grid_point_maps_to_itself() {
        for j in 0..=(60 * MAX_EMITTER_TRACK_DURATION as u32) {
            assert_eq!(grid_step(j as f32 / 60.0), j, "grid point {j}");
        }
    }

    #[test]
    fn an_off_grid_time_takes_the_grid_point_above_it() {
        assert_eq!(grid_step(1.0 + 0.4 / 60.0), 61);
        assert_eq!(grid_step(0.5 / 60.0), 1);
    }

    #[test]
    fn a_trigger_late_in_a_long_track_fires_on_a_thirty_fps_frame_time() {
        // The reviewer's case: at 30 fps frame 7689 of a 300 s track, the
        // f32 quotient sits far enough below 15378/60 that a naive floor
        // drops a step and the burst authored there never fires.
        let mut config = base_config();
        config.emitter.spawn_burst_size = 4;
        config.emitter_track = Some(EmitterTrack {
            duration: 300.0,
            preroll: 0.0,
            keyframes: vec![
                EmitterKeyframe { time: 0.0, x: 0.0, y: 0.0, vx: Some(0.0), vy: Some(0.0) },
                EmitterKeyframe { time: 300.0, x: 0.0, y: 0.0, vx: Some(0.0), vy: Some(0.0) },
            ],
            triggers: vec![EmitterTrigger { time: 15378.0 / 60.0, kind: TriggerKind::Burst }],
        });
        let mut sim = Simulation::new(config, 5);

        sim.seek(7689.0 / 30.0);

        assert!(sim.particle_count() > 0, "the burst at 15378/60 s should have fired");
    }

    #[test]
    fn seek_to_an_off_grid_duration_runs_the_final_partial_window() {
        // duration 0.525 sits between grid points 31 (0.5167) and 32
        // (0.5333), so a trigger at 0.52 fires in step 32 -- a step a
        // floor-to-the-grid seek would never run.
        let track = EmitterTrack {
            duration: 0.525,
            preroll: 0.0,
            keyframes: vec![
                EmitterKeyframe { time: 0.0, x: 0.0, y: 0.0, vx: Some(0.0), vy: Some(0.0) },
                EmitterKeyframe { time: 0.525, x: 0.0, y: 0.0, vx: Some(0.0), vy: Some(0.0) },
            ],
            triggers: vec![EmitterTrigger { time: 0.52, kind: TriggerKind::Burst }],
        };
        let config_at = || {
            let mut config = base_config();
            config.emitter.spawn_burst_size = 2;
            config.emitter_track = Some(track.clone());
            config
        };

        let mut at_duration = Simulation::new(config_at(), 5);
        at_duration.seek(0.525);
        assert!(at_duration.particle_count() > 0, "seek to the duration should run the tail window");

        let mut past_duration = Simulation::new(config_at(), 5);
        past_duration.seek(10.0);
        assert!(past_duration.particle_count() > 0, "a capped seek should run the tail window too");

        let mut before = Simulation::new(config_at(), 5);
        before.seek(0.5);
        assert_eq!(before.particle_count(), 0, "a seek short of the trigger must not fire it");
    }

    #[test]
    fn a_twenty_four_fps_host_sees_a_burst_on_the_first_frame_at_or_after_it() {
        // 24 fps frames never land on the 1/60 grid. A burst authored at
        // 0.955 must show up on frame 23 (t = 0.9583), the first frame at
        // or after it -- not frame 24, which is what flooring gave.
        let config_at = || {
            let mut config = base_config();
            config.emitter.spawn_burst_size = 2;
            config.emitter_track = Some(EmitterTrack {
                duration: 1.0,
                preroll: 0.0,
                keyframes: vec![
                    EmitterKeyframe { time: 0.0, x: 0.0, y: 0.0, vx: Some(0.0), vy: Some(0.0) },
                    EmitterKeyframe { time: 1.0, x: 0.0, y: 0.0, vx: Some(0.0), vy: Some(0.0) },
                ],
                triggers: vec![EmitterTrigger { time: 0.955, kind: TriggerKind::Burst }],
            });
            config
        };

        let mut before = Simulation::new(config_at(), 5);
        before.seek(22.0 / 24.0);
        assert_eq!(before.particle_count(), 0, "frame 22 (t = 0.9167) is before the burst");

        let mut on_frame = Simulation::new(config_at(), 5);
        on_frame.seek(23.0 / 24.0);
        assert!(
            on_frame.particle_count() > 0,
            "frame 23 (t = 0.9583) is the first frame at or after 0.955 and must show the burst"
        );
    }

    #[test]
    fn a_trigger_at_a_duration_a_hair_past_a_grid_point_still_fires() {
        // The reviewer's case: duration 1.0000001 is within tolerance of
        // grid point 60, so every seek at or past it runs step 60 -- the
        // step the burst authored at the duration fires in. No host frame
        // time lands exactly on 1.0000001, so a trigger there could never
        // fire under a floor-and-special-case-the-duration contract.
        let duration = 1.0000001_f32;
        let config_at = || {
            let mut config = base_config();
            config.emitter.spawn_burst_size = 2;
            config.emitter_track = Some(EmitterTrack {
                duration,
                preroll: 0.0,
                keyframes: vec![
                    EmitterKeyframe { time: 0.0, x: 0.0, y: 0.0, vx: Some(0.0), vy: Some(0.0) },
                    EmitterKeyframe { time: duration, x: 0.0, y: 0.0, vx: Some(0.0), vy: Some(0.0) },
                ],
                triggers: vec![EmitterTrigger { time: duration, kind: TriggerKind::Burst }],
            });
            config
        };

        for time in [duration, 10.0, 1.0] {
            let mut sim = Simulation::new(config_at(), 5);
            sim.seek(time);
            assert!(sim.particle_count() > 0, "seek({time}) should have fired the burst");
        }
    }

    #[test]
    fn the_final_step_samples_the_emitter_at_the_duration_not_past_it() {
        // Step 32 ends at 0.5333, past the 0.525 duration. Sampling there
        // would extrapolate the keyframe ramp and spawn the burst at
        // x = 533 instead of the x = 525 the track actually reaches.
        let mut config = base_config();
        config.emitter.spawn_burst_size = 4;
        config.emitter_track = Some(EmitterTrack {
            duration: 0.525,
            preroll: 0.0,
            keyframes: vec![
                EmitterKeyframe { time: 0.0, x: 0.0, y: 0.0, vx: Some(0.0), vy: Some(0.0) },
                EmitterKeyframe { time: 1.0, x: 1000.0, y: 0.0, vx: Some(0.0), vy: Some(0.0) },
            ],
            triggers: vec![EmitterTrigger { time: 0.52, kind: TriggerKind::Burst }],
        });
        let mut sim = Simulation::new(config, 5);

        sim.seek(0.525);

        assert_eq!(sim.particle_count(), 4, "the burst should have fired");
        for p in sim.buffer() {
            // Zero speed, gravity and turbulence, so a particle never
            // leaves its spawn point; the only spread is the +/-2 px
            // spawn jitter.
            assert!((p.x - 525.0).abs() < 2.5, "burst spawned at x = {}, expected ~525", p.x);
        }
    }

    #[test]
    fn the_particle_instance_layout_is_eight_contiguous_f32s() {
        assert_eq!(std::mem::size_of::<ParticleInstance>(), 32);
        assert_eq!(std::mem::align_of::<ParticleInstance>(), 4);
    }

    #[test]
    fn the_buffer_pointer_is_stable_across_many_advances() {
        let mut config = base_config();
        config.emitter.spawn_rate_while_active = 10.0;
        let mut sim = Simulation::new(config, 42);
        sim.set_emitter(0.0, 0.0, 1.0, 0.0, true);

        let original = sim.buffer().as_ptr();
        assert!(!original.is_null(), "preallocated buffer must have a real address");

        for _ in 0..600 {
            sim.advance(1.0 / 60.0);
        }

        assert!(sim.particle_count() > 0, "test is vacuous with no particles");
        assert_eq!(sim.buffer().as_ptr(), original, "buffer reallocated mid-run");
    }

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
        sim.set_bounds(100.0, 100.0).unwrap();
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

    /// One particle from an emitter at (`x`, 50), moving along `angle`
    /// (degrees) at `speed` px per step, spawned by the first advance, after
    /// which the emitter is switched off. Bounds are 100x100.
    /// `margin` of `None` turns culling off, which gives each test a control
    /// run that proves the particle really spawned.
    fn runner_from(x: f32, angle: f32, speed: f32, margin: Option<f32>) -> Simulation {
        let mut config = base_config();
        config.emitter.spawn_rate_while_active = 1.0;
        config.emitter.emission_angle = angle;
        config.initial_speed_min = speed;
        config.initial_speed_max = speed;
        config.cull_margin = margin;
        let mut sim = Simulation::new(config, 7);
        sim.set_bounds(100.0, 100.0).unwrap();
        sim.set_emitter(x, 50.0, 0.0, 0.0, true);
        sim.advance(TICK);
        sim.set_emitter(x, 50.0, 0.0, 0.0, false);
        sim
    }

    fn advance_steps(sim: &mut Simulation, steps: usize) {
        for _ in 0..steps {
            sim.advance(TICK);
        }
    }

    /// The cull rule as an invariant: after a step, a particle outside the
    /// cull rectangle has not entered it and is not moving away from it.
    fn assert_survivors_obey_the_cull_rule(sim: &Simulation, margin: f32, w: f32, h: f32) {
        for p in sim.live.pool.iter() {
            if !within_margin(p.x, p.y, margin, w, h) {
                assert!(!p.entered, "an entered particle survived outside at ({}, {})", p.x, p.y);
                assert!(!moving_away(p, margin, w, h), "a particle moving away survived at ({}, {})", p.x, p.y);
            }
        }
    }

    #[test]
    fn a_particle_at_rest_outside_the_margin_is_kept() {
        // 50 px left of the frame with a margin of 0 and no speed: it is
        // neither inside nor moving away, so it stays.
        let sim = runner_from(-50.0, 0.0, 0.0, Some(0.0));
        assert_eq!(runner_from(-50.0, 0.0, 0.0, None).particle_count(), 1, "test is vacuous");
        assert_eq!(sim.particle_count(), 1);
        assert!(sim.live.pool.iter().all(|p| !p.entered));
    }

    #[test]
    fn an_entered_particle_is_culled_when_it_leaves_again() {
        // From x = -30 at +10 px per step, margin 0: outside on the way in
        // (moving toward the frame, so kept), inside from about step 4, past
        // x = 100 from about step 14.
        let mut sim = runner_from(-30.0, 0.0, 10.0, Some(0.0));
        assert_eq!(sim.particle_count(), 1, "culled on the way in");
        advance_steps(&mut sim, 7); // step 8, x about 50
        assert_eq!(sim.particle_count(), 1, "culled while crossing the frame");
        assert!(sim.live.pool.iter().all(|p| p.entered), "never marked as entered");
        advance_steps(&mut sim, 8); // step 16, x about 130
        assert_eq!(sim.particle_count(), 0, "kept after leaving");
    }

    #[test]
    fn a_particle_moving_away_from_an_outside_emitter_is_culled() {
        // 30 px left of the frame, moving left: it never enters and never
        // will, so it must not sit in the pool for its whole lifetime.
        assert_eq!(runner_from(-30.0, 180.0, 10.0, None).particle_count(), 1, "test is vacuous");
        assert_eq!(runner_from(-30.0, 180.0, 10.0, Some(0.0)).particle_count(), 0, "kept a particle moving away");
    }

    #[test]
    fn a_particle_in_the_margin_zone_is_culled_once_it_leaves_the_margin() {
        // 20 px left of the frame, inside a margin of 50, moving away.
        let mut sim = runner_from(-20.0, 180.0, 10.0, Some(50.0));
        assert_eq!(sim.particle_count(), 1, "culled while still inside the margin");
        assert!(sim.live.pool.iter().all(|p| p.entered), "not marked as entered");
        advance_steps(&mut sim, 7); // step 8, x about -100, past -50
        assert_eq!(sim.particle_count(), 0, "kept past the margin");
    }

    #[test]
    fn a_particle_that_leaves_the_frame_in_its_first_step_is_culled() {
        // 15 px per step (the schema's speed ceiling) from x = 99 against a
        // margin of 5: outside the rectangle after one step.
        assert_eq!(runner_from(99.0, 0.0, 15.0, None).particle_count(), 1, "test is vacuous");
        assert_eq!(runner_from(99.0, 0.0, 15.0, Some(5.0)).particle_count(), 0, "kept a particle that left");
    }

    #[test]
    fn a_particle_that_skips_a_tiny_frame_in_one_step_is_culled() {
        // A 4x4 frame and 15 px per step: from x = -5 the particle lands at
        // about 10, past the far edge, without a step ever ending inside.
        // It is outside and moving away, so it is culled all the same.
        let count = |margin: Option<f32>| {
            let mut config = base_config();
            config.emitter.spawn_rate_while_active = 1.0;
            config.initial_speed_min = 15.0;
            config.initial_speed_max = 15.0;
            config.cull_margin = margin;
            let mut sim = Simulation::new(config, 7);
            sim.set_bounds(4.0, 4.0).unwrap();
            sim.set_emitter(-5.0, 2.0, 0.0, 0.0, true);
            sim.advance(TICK);
            sim.particle_count()
        };
        assert_eq!(count(None), 1, "test is vacuous");
        assert_eq!(count(Some(0.0)), 0, "a particle that skipped the frame was kept");
    }

    #[test]
    fn switching_culling_on_mid_run_culls_particles_already_outside() {
        let mut config = base_config();
        config.emitter.spawn_rate_while_active = 1.0;
        config.initial_speed_min = 10.0;
        config.initial_speed_max = 10.0;
        let mut sim = Simulation::new(config.clone(), 7);
        sim.set_bounds(100.0, 100.0).unwrap();
        sim.set_emitter(50.0, 50.0, 0.0, 0.0, true);
        sim.advance(TICK);
        sim.set_emitter(50.0, 50.0, 0.0, 0.0, false);
        advance_steps(&mut sim, 29); // x about 350, long gone from the frame
        assert_eq!(sim.particle_count(), 1, "test is vacuous: culling was never off");

        config.cull_margin = Some(0.0);
        sim.set_config(config);
        assert_eq!(sim.particle_count(), 0, "a particle already outside survived");
        assert!(sim.buffer().is_empty(), "the buffer still shows it before any step");
    }

    #[test]
    fn a_smaller_cull_margin_culls_the_live_pool_at_once() {
        let mut config = base_config();
        config.emitter.spawn_rate_while_active = 1.0;
        config.initial_speed_min = 10.0;
        config.initial_speed_max = 10.0;
        config.cull_margin = Some(500.0);
        let mut sim = Simulation::new(config.clone(), 7);
        sim.set_bounds(100.0, 100.0).unwrap();
        sim.set_emitter(50.0, 50.0, 0.0, 0.0, true);
        sim.advance(TICK);
        sim.set_emitter(50.0, 50.0, 0.0, 0.0, false);
        advance_steps(&mut sim, 10); // x about 160: past the frame, inside the margin
        assert_eq!(sim.particle_count(), 1, "test is vacuous: the margin did not hold it");

        config.cull_margin = Some(5.0);
        sim.set_config(config);
        assert_eq!(sim.particle_count(), 0);
        assert!(sim.buffer().is_empty(), "the buffer still shows it before any step");
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
            sim.set_bounds(100.0, 100.0).unwrap();
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

    /// An infinite speed range makes `Rng::range` return NaN (inf + 0 * (inf - inf)),
    /// so the spawned particle's position is NaN after its first step. Set
    /// past `Simulation::new`, which would clamp the infinities to 15: the
    /// cull guard is the last line of defence, for whatever else yields NaN.
    fn nan_runner(margin: Option<f32>) -> Simulation {
        let mut config = base_config();
        config.emitter.spawn_rate_while_active = 1.0;
        config.cull_margin = margin;
        let mut sim = Simulation::new(config, 7);
        sim.config.initial_speed_min = f32::INFINITY;
        sim.config.initial_speed_max = f32::INFINITY;
        sim.set_bounds(100.0, 100.0).unwrap();
        sim.set_emitter(50.0, 50.0, 0.0, 0.0, true);
        sim.advance(TICK);
        sim
    }

    #[test]
    fn a_particle_with_a_non_finite_position_is_culled() {
        // Without a margin the NaN particle lives, so this cannot pass
        // because nothing spawned.
        assert_eq!(nan_runner(None).particle_count(), 1, "test is vacuous without the NaN particle");
        assert_eq!(nan_runner(Some(5.0)).particle_count(), 0, "a NaN position was kept");
    }

    #[test]
    fn nothing_is_culled_without_a_margin() {
        let mut sim = one_runner(None);
        sim.set_bounds(100.0, 100.0).unwrap();
        for _ in 0..20 {
            sim.advance(TICK);
        }
        assert_eq!(sim.particle_count(), 1);
    }

    #[test]
    fn invalid_bounds_are_rejected_and_leave_the_bounds_alone() {
        let mut sim = Simulation::new(base_config(), 1);
        assert!(sim.set_bounds(f32::NAN, 50.0).is_err());
        assert_eq!(sim.bounds(), None, "a rejected size set bounds");
        sim.set_bounds(100.0, 50.0).unwrap();
        assert_eq!(sim.bounds(), Some((100.0, 50.0)));
        for (w, h) in [(0.0, 50.0), (100.0, -1.0), (f32::NAN, 50.0), (100.0, f32::INFINITY)] {
            let error = sim.set_bounds(w, h).unwrap_err();
            assert!(error.contains("finite and positive"), "({w}, {h}): {error}");
            assert_eq!(sim.bounds(), Some((100.0, 50.0)), "({w}, {h}) changed the bounds");
        }
    }

    #[test]
    fn rejected_bounds_keep_the_baked_position_and_the_pool() {
        let mut sim = Simulation::new(track_config(), 5);
        sim.set_bounds(100.0, 100.0).unwrap();
        sim.seek(0.5);
        assert!(sim.baked.is_some());
        assert!(sim.set_bounds(0.0, 0.0).is_err());
        assert!(sim.baked.is_some(), "a rejected size cleared the cursor");

        // A runner outside a 100-wide frame would be culled if the
        // rejected size reached the pool in any form.
        let mut sim = one_runner(Some(0.0));
        sim.set_bounds(200.0, 100.0).unwrap();
        advance_steps(&mut sim, 5);
        assert!(sim.buffer()[0].x > 100.0, "test is vacuous: the runner is inside a 100-wide frame");
        for (w, h) in [(0.0, 100.0), (100.0, f32::NAN)] {
            assert!(sim.set_bounds(w, h).is_err());
            assert_eq!(sim.particle_count(), 1, "({w}, {h}) culled the pool");
        }
    }

    #[test]
    fn changing_the_bounds_clears_the_baked_position_but_repeating_them_does_not() {
        let mut sim = Simulation::new(track_config(), 5);
        sim.seek(0.5);
        assert!(sim.baked.is_some());
        sim.set_bounds(100.0, 100.0).unwrap();
        assert!(sim.baked.is_none(), "a new size left the cursor in place");

        sim.seek(0.5);
        assert!(sim.baked.is_some());
        sim.set_bounds(100.0, 100.0).unwrap();
        assert!(sim.baked.is_some(), "identical bounds cleared the cursor");
    }

    #[test]
    fn shrinking_the_bounds_culls_the_live_pool_without_a_step() {
        let mut sim = one_runner(Some(0.0));
        sim.set_bounds(200.0, 100.0).unwrap();
        advance_steps(&mut sim, 5);
        assert_eq!(sim.buffer().len(), 1, "test is vacuous: the runner left the frame");
        assert!(sim.buffer()[0].x > 100.0, "test is vacuous: the runner is inside the smaller frame");

        sim.set_bounds(100.0, 100.0).unwrap();
        assert_eq!(sim.particle_count(), 0, "a particle outside the new frame survived");
        assert!(sim.buffer().is_empty(), "the buffer still shows it");
    }

    #[test]
    fn recull_follows_the_entered_and_moving_away_rules() {
        // A runner that has not entered the frame is kept while it heads
        // toward it and culled once it heads away; one inside the new
        // frame is kept and marked entered.
        let mut sim = one_runner(Some(0.0));
        sim.set_bounds(10.0, 10.0).unwrap(); // the runner sits around (60, 50), right of it
        assert_eq!(sim.particle_count(), 0, "outside and moving away (right) should go");

        let mut sim = one_runner(Some(0.0));
        sim.live.pool.retain_mut(|p| {
            p.vx = -10.0; // now heading back toward a frame to its left
            true
        });
        sim.set_bounds(10.0, 100.0).unwrap();
        assert_eq!(sim.particle_count(), 1, "outside but moving toward the frame must stay");
        assert!(sim.live.pool.iter().all(|p| !p.entered));

        sim.set_bounds(200.0, 100.0).unwrap();
        assert!(sim.live.pool.iter().all(|p| p.entered), "inside the new frame, not marked entered");
    }

    #[test]
    fn growing_the_bounds_or_having_no_margin_culls_nothing() {
        let mut sim = one_runner(Some(0.0));
        sim.set_bounds(200.0, 100.0).unwrap();
        advance_steps(&mut sim, 3);
        sim.set_bounds(400.0, 400.0).unwrap();
        assert_eq!(sim.particle_count(), 1);

        let mut sim = one_runner(None);
        sim.set_bounds(200.0, 100.0).unwrap();
        advance_steps(&mut sim, 5);
        sim.set_bounds(100.0, 100.0).unwrap();
        assert_eq!(sim.particle_count(), 1, "no cullMargin, nothing to cull");
    }

    /// Every public `&mut self` method of `Simulation`, split by whether it
    /// changes state outside `seek` (and so must clear the baked position).
    const STATE_MUTATORS: [&str; 5] = ["set_config", "set_bounds", "set_emitter", "trigger_burst", "advance"];

    #[test]
    fn every_public_mutator_is_known_to_this_suite() {
        // Fails when a `pub fn ... (&mut self` is added to `Simulation`,
        // so its author has to add it to `STATE_MUTATORS` and to
        // `every_state_mutator_invalidates_forward_seek` (or confirm it is
        // `seek`, the only method allowed to keep the baked position).
        let source = include_str!("simulation.rs");
        let production = &source[..source.find("#[cfg(test)]").unwrap()];
        // Whitespace-collapsed, so a signature the formatter wrapped over
        // several lines is still one `pub fn ... (&mut self ...)` run.
        let flat = production.split_whitespace().collect::<Vec<_>>().join(" ");
        let mut found: Vec<&str> = Vec::new();
        for (start, _) in flat.match_indices("pub ") {
            let rest = &flat[start + 4..];
            let rest = rest.trim_start_matches("const ").trim_start_matches("unsafe ");
            let Some(rest) = rest.strip_prefix("fn ") else { continue };
            let end = rest.find(|c| c == '{' || c == ';').unwrap_or(rest.len());
            let (name, signature) = (&rest[..rest.find('(').unwrap()], &rest[..end]);
            if signature.contains("&mut self") || signature.contains("self: &mut") {
                found.push(name);
            }
        }
        found.retain(|name| *name != "seek");
        let mut expected = STATE_MUTATORS.to_vec();
        expected.sort_unstable();
        found.sort_unstable();
        assert_eq!(found, expected, "a public mutator is not covered by the forward-seek invalidation test");
    }

    /// Each of `STATE_MUTATORS`, applied in a way that changes the state.
    fn state_mutators() -> [(&'static str, fn(&mut Simulation)); 5] {
        [
            ("set_config", |sim| {
                let mut config = sim.config().clone();
                config.gravity_y += 3.0;
                sim.set_config(config);
            }),
            ("set_bounds", |sim| sim.set_bounds(100.0, 100.0).unwrap()),
            ("set_emitter", |sim| sim.set_emitter(5.0, 5.0, 1.0, 1.0, true)),
            ("trigger_burst", |sim| sim.trigger_burst()),
            ("advance", |sim| sim.advance(TICK)),
        ]
    }

    #[test]
    fn every_state_mutator_invalidates_forward_seek() {
        let apply = state_mutators();
        assert_eq!(apply.map(|(name, _)| name), STATE_MUTATORS, "keep this table in step with STATE_MUTATORS");

        let make = || {
            let mut config = culling_track_config();
            config.cull_margin = Some(0.0);
            Simulation::new(config, 5)
        };
        for (name, mutate) in apply {
            // Reference: the mutation, then a seek from a cleared cursor.
            let mut fresh = make();
            fresh.seek(0.5);
            fresh.leave_baked();
            mutate(&mut fresh);
            fresh.seek(0.8);

            // Under test: seek, mutate, then seek forward, which steps on
            // only if the mutator left the cursor in place.
            let mut stepped = make();
            stepped.seek(0.5);
            assert!(stepped.baked.is_some(), "{name}: vacuous, no cursor to invalidate");
            mutate(&mut stepped);
            assert!(stepped.baked.is_none(), "{name} left the baked position in place");
            stepped.seek(0.8);

            assert_eq!(xy(&fresh), xy(&stepped), "{name}: forward seek differs from a fresh seek");
        }
    }

    #[test]
    fn every_state_mutator_keeps_a_backward_seek_pure() {
        // The saved pre-roll is the other state that can outlive a mutator.
        // A backward seek resumes from it, so a mutator that changes what a
        // replay computes (and so the pre-roll) must drop it. Each mutator
        // is applied after a seek has saved it, and the backward seek that
        // follows must match a handle that never saved one.
        let make = || {
            let mut config = prerolled_config(0.5, 0.0, -30.0, 60.0);
            config.cull_margin = Some(0.0);
            config.initial_speed_min = 2.0;
            config.initial_speed_max = 2.0;
            Simulation::new(config, 5)
        };
        let mut untouched = make();
        untouched.seek(0.3);
        for (name, mutate) in state_mutators() {
            let mut fresh = make();
            mutate(&mut fresh);
            fresh.seek(0.3);

            let mut scrubbed = make();
            scrubbed.seek(0.8);
            assert!(scrubbed.preroll_saved, "{name}: vacuous, no saved pre-roll");
            mutate(&mut scrubbed);
            scrubbed.seek(0.3);

            assert_eq!(buffer_bits(&fresh), buffer_bits(&scrubbed), "{name}: backward seek differs from a fresh seek");
            if matches!(name, "set_config" | "set_bounds") {
                assert_ne!(buffer_bits(&fresh), buffer_bits(&untouched), "{name}: vacuous, the pre-roll did not change");
            }
        }
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
        culled.set_bounds(200.0, 200.0).unwrap();
        culled.seek(0.55);

        assert_eq!(open.particle_count(), 12, "test is vacuous without the burst");
        assert!(culled.particle_count() > 0, "everything was culled");
        assert!(culled.particle_count() < open.particle_count(), "nothing was culled");
        assert_survivors_obey_the_cull_rule(&culled, 0.0, 200.0, 200.0);
    }

    #[test]
    fn culling_is_deterministic_under_seek() {
        let make = || {
            let mut sim = Simulation::new(culling_track_config(), 5);
            sim.set_bounds(200.0, 200.0).unwrap();
            sim
        };
        let mut fresh = make();
        fresh.seek(0.55);

        // Backward: overshoot, then seek back, which replays from the start of the track.
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

    #[test]
    fn culling_with_an_off_screen_emitter_is_deterministic_under_seek() {
        // The burst comes from (30, -40), above the frame. Downward
        // particles enter it and upward ones are culled for moving away, so
        // the `entered` flag is part of the state each seek path has to
        // reproduce.
        let make = || {
            let mut config = culling_track_config();
            for keyframe in &mut config.emitter_track.as_mut().unwrap().keyframes {
                keyframe.y = -40.0;
            }
            let mut sim = Simulation::new(config, 5);
            sim.set_bounds(200.0, 200.0).unwrap();
            sim
        };
        let mut fresh = make();
        fresh.seek(0.9);

        let mut rewound = make();
        rewound.seek(1.0);
        rewound.seek(0.9);

        let mut stepped = make();
        stepped.seek(0.8);
        stepped.seek(0.9);

        assert!(fresh.live.pool.iter().any(|p| p.entered), "test is vacuous: nothing entered");
        assert!(fresh.particle_count() < 12, "test is vacuous: nothing was culled");
        assert_survivors_obey_the_cull_rule(&fresh, 0.0, 200.0, 200.0);
        assert_eq!(xy(&fresh), xy(&rewound));
        assert_eq!(xy(&fresh), xy(&stepped));
    }
}
