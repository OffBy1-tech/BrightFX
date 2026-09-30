use crate::color::{hex_to_rgb, hsl_to_rgb, interpolate_hex, palette_index, sample_palette, Palette};
use crate::particle::{Particle, ParticlePool, MAX_PARTICLES};
use crate::rng::Rng;
use crate::schema::{
    ColorMode, SpinDirection, EmissionPattern, EmitterKeyframe, ParticleFxConfig, TriggerKind,
    MAX_EMITTER_TRACK_DURATION,
};
use std::sync::Arc;

/// Presets store lifetimes/speeds/gravity/drag/turbulence in 60fps
/// frame-equivalent units (matching Mouseflare's `customFxRenderer.ts`), so
/// `step` converts a wall-clock `dt` (seconds) into frame-equivalents (`fe`)
/// before applying it. `dt` is clamped so a stall can't teleport or
/// mass-expire particles.
const REFERENCE_HZ: f32 = 60.0;
const MAX_DELTA_SECONDS: f32 = 0.1;
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
const GRID_RATE: f64 = 60.0;

/// How far a baked time may sit from a grid point and still snap to it, in
/// steps. The relative term tracks the f32 ULP of `target` (an f32's ULP
/// grows with its magnitude, and `target * 60` scales that error by 60);
/// the absolute term covers times near zero.
const GRID_TOLERANCE_ABS: f64 = 1e-6;
const GRID_TOLERANCE_REL: f64 = 8e-6;

fn grid_tolerance(target: f32) -> f64 {
    GRID_TOLERANCE_ABS + GRID_TOLERANCE_REL * target as f64
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
fn grid_step(target: f32) -> u32 {
    let steps = target as f64 * GRID_RATE;
    let nearest = steps.round();
    if (steps - nearest).abs() <= grid_tolerance(target) {
        nearest as u32
    } else {
        steps.ceil() as u32
    }
}

/// One of the track's triggers with the step it fires in resolved ahead
/// of time: `fire_step` is the grid step at or after its authored time,
/// so the trigger fires in the step that *ends* there -- exactly the
/// step `seek` runs to reach that time. `0` means it fires at the reset,
/// before any step runs.
#[derive(Debug, Clone, Copy)]
struct BakedTrigger {
    fire_step: u32,
    kind: TriggerKind,
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
    buffer: Vec<ParticleInstance>,
    /// Whole steps applied since the last reset in baked mode. `None`
    /// whenever the pool is not the product of a pure replay, so the next
    /// `seek` replays from zero. Only `seek` sets it; everything else
    /// clears it through `leave_baked`.
    baked: Option<u32>,
}

impl Simulation {
    pub fn new(config: ParticleFxConfig, seed: u64) -> Self {
        Self {
            palette: build_palette(&config),
            baked_track: build_baked_track(&config),
            config,
            seed,
            pool: ParticlePool::new(MAX_PARTICLES),
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
            // Preallocated so the pointer handed across the FFI boundary is
            // stable for the life of the simulation. `rebuild_buffer` only
            // ever clears and re-pushes, never exceeding MAX_PARTICLES, so
            // this Vec never reallocates.
            buffer: Vec::with_capacity(MAX_PARTICLES),
            baked: None,
        }
    }

    pub fn set_config(&mut self, config: ParticleFxConfig) {
        self.palette = build_palette(&config);
        self.baked_track = build_baked_track(&config);
        self.config = config;
        // Live particles keep their state (that is the boundary's contract),
        // but the track may have changed, so the next seek replays.
        self.leave_baked();
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
        self.emitter_x = x;
        self.emitter_y = y;
        self.emitter_vx = vx;
        self.emitter_vy = vy;
        self.emitter_speed = (vx * vx + vy * vy).sqrt();
        self.emitter_active = active;
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

    pub fn advance(&mut self, dt: f32) {
        self.leave_baked();
        self.step(dt);
    }

    pub fn buffer(&self) -> &[ParticleInstance] {
        &self.buffer
    }

    pub fn config(&self) -> &ParticleFxConfig {
        &self.config
    }

    pub fn particle_count(&self) -> usize {
        self.pool.len()
    }

    /// The loaded config's name. Lets hosts confirm which effect is active
    /// without round-tripping the whole config back across the boundary.
    pub fn config_name(&self) -> &str {
        &self.config.name
    }

    /// Drops the baked position, so the next `seek` replays from zero
    /// instead of stepping forward. The one place `baked` is cleared. By
    /// convention -- nothing enforces it -- every host-facing call that
    /// changes simulation state calls this: `set_emitter`, `trigger_burst`,
    /// and `advance` before their own writes, `set_config` after swapping
    /// the config, and `reset`. A new such call must do the same, or a
    /// forward seek would step on from a state the track did not produce.
    fn leave_baked(&mut self) {
        self.baked = None;
    }

    fn reset(&mut self) {
        self.leave_baked();
        self.pool.clear();
        self.rng = Rng::new(self.seed);
        self.color_rng = color_rng(self.seed);
        self.spawn_budget = 0.0;
        self.global_hue = 0.0;
        self.buffer.clear();
        self.emitter_x = 0.0;
        self.emitter_y = 0.0;
        self.emitter_vx = 0.0;
        self.emitter_vy = 0.0;
        self.emitter_speed = 0.0;
        self.emitter_active = false;
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
    /// If the last call was a seek to an earlier or equal grid step, only
    /// the steps in between are applied; otherwise the simulation resets
    /// and replays from t=0. Both paths run the same whole steps from a
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
        // number of synchronous step iterations.
        let target = time.max(0.0).min(track.duration_cap);
        let n = grid_step(target);

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
            // Grid times are computed from the step index, never
            // accumulated, so step k is the same on every path. The
            // sample is clamped to the duration so the final step of an
            // off-grid track does not extrapolate past the last keyframe
            // window.
            let t_next = ((k + 1) as f32 * PLAYBACK_STEP).min(track.duration_cap);

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
    }

    /// Resets and fires the track's `fire_step == 0` triggers -- those
    /// authored at (or within tolerance of) t=0. Baked playback requires
    /// an explicit `StartContinuous` (or a `Burst`) to spawn anything --
    /// unlike live mode, seek never implicitly emits from t=0.
    fn restart_track(&mut self, track: &BakedTrack) {
        self.reset();
        let (x0, y0, vx0, vy0) = sample_track(&track.keyframes, 0.0);
        self.place_emitter(x0, y0, vx0, vy0, false);
        self.fire_triggers(track, 0);
    }

    /// Applies the triggers that fire in `step`, in time order. Each
    /// trigger names exactly one step, so there is no bookkeeping of what
    /// has fired.
    fn fire_triggers(&mut self, track: &BakedTrack, step: u32) {
        for trig in &track.triggers {
            if trig.fire_step == step {
                match trig.kind {
                    TriggerKind::Burst => self.spawn_burst(),
                    TriggerKind::StartContinuous => self.emitter_active = true,
                    TriggerKind::StopContinuous => self.emitter_active = false,
                }
            }
        }
    }

    fn step(&mut self, dt: f32) {
        let seconds = dt.max(0.0).min(MAX_DELTA_SECONDS);
        let fe = seconds * REFERENCE_HZ;

        if self.emitter_active {
            self.spawn_budget += self.config.emitter.spawn_rate_while_active * fe;
            let count = (self.spawn_budget.floor() as i32).clamp(0, 40) as u32;
            self.spawn_budget -= count as f32;
            for _ in 0..count {
                self.spawn_particle(false);
            }
        } else if self.config.emitter.spawn_rate_idle > 0.0
            && self.rng.f32() < self.config.emitter.spawn_rate_idle * 0.3
        {
            self.spawn_particle(false);
        }

        let gravity_x = self.config.gravity_x * 0.1;
        let gravity_y = self.config.gravity_y * 0.1;
        let drag = self.config.drag;
        let turbulence = self.config.turbulence;
        let vortex = self.config.vortex_attraction;
        let emitter_x = self.emitter_x;
        let emitter_y = self.emitter_y;
        let size_curve = self.config.size_curve;

        self.pool.retain_mut(|p| {
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
            p.rotation += p.rotation_speed * fe;

            let progress = if p.max_life > 0.0 { p.life / p.max_life } else { 1.0 };
            p.size = evaluate_size_curve(size_curve, p, progress).max(0.2);
            p.alpha = evaluate_alpha_curve(p, progress).clamp(0.0, 1.0);

            true
        });

        self.rebuild_buffer();
    }

    fn spawn_particle(&mut self, is_burst: bool) {
        let emitter = self.config.emitter.clone();
        let dx = self.emitter_vx;
        let dy = self.emitter_vy;

        let pattern = if is_burst {
            EmissionPattern::RadialBurst
        } else {
            emitter.emission_pattern
        };

        let mut speed = self
            .rng
            .range(self.config.initial_speed_min, self.config.initial_speed_max);

        let angle = if is_burst {
            speed *= 1.4;
            self.rng.range(0.0, std::f32::consts::TAU)
        } else {
            match pattern {
                EmissionPattern::RadialBurst
                | EmissionPattern::VortexSpiral
                | EmissionPattern::Orbit => self.rng.range(0.0, std::f32::consts::TAU),
                EmissionPattern::Fountain => {
                    let base = (emitter.emission_angle - 90.0).to_radians();
                    let spread = emitter.emission_spread.to_radians();
                    base + (self.rng.f32() - 0.5) * spread
                }
                EmissionPattern::DirectionalCone => {
                    let base = emitter.emission_angle.to_radians();
                    let spread = emitter.emission_spread.to_radians();
                    base + (self.rng.f32() - 0.5) * spread
                }
                EmissionPattern::Trail => {
                    if (dx * dx + dy * dy).sqrt() > 0.1 {
                        let move_angle = dy.atan2(dx);
                        move_angle
                            + std::f32::consts::PI
                            + (self.rng.f32() - 0.5) * emitter.emission_spread.to_radians()
                    } else {
                        self.rng.range(0.0, std::f32::consts::TAU)
                    }
                }
            }
        };

        let vx = angle.cos() * speed + self.emitter_vx * emitter.velocity_inheritance * 0.15;
        let vy = angle.sin() * speed + self.emitter_vy * emitter.velocity_inheritance * 0.15;

        let max_life = self
            .rng
            .range(self.config.lifetime_min, self.config.lifetime_max);

        self.global_hue = (self.global_hue + self.config.rainbow_speed * 0.5) % 360.0;
        let color_rgb = if self.config.color_mode == ColorMode::RainbowCycle {
            hsl_to_rgb(self.global_hue, 0.9, 0.6)
        } else {
            hex_to_rgb(&self.config.primary_color)
        };
        // Drawn in every mode, so switching into random-palette later finds
        // a pick already on every live particle.
        let palette_pick = self.color_rng.f32();

        let rotation_speed = self
            .rng
            .range(self.config.rotation_speed_min, self.config.rotation_speed_max);
        // Drawn only in random mode, so a fixed-spin config keeps the
        // motion sequence it has always had.
        let rotation_speed = if self.config.spin_direction == SpinDirection::Random && self.rng.f32() < 0.5 {
            -rotation_speed
        } else {
            rotation_speed
        };

        let particle = Particle {
            x: self.emitter_x + (self.rng.f32() - 0.5) * 4.0,
            y: self.emitter_y + (self.rng.f32() - 0.5) * 4.0,
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
            hue: self.global_hue,
            life: 0.0,
            max_life,
            rotation: self.rng.range(0.0, std::f32::consts::TAU),
            rotation_speed,
            turbulence_seed: self.rng.range(0.0, 100.0),
        };

        self.pool.spawn(particle);
    }

    fn rebuild_buffer(&mut self) {
        self.buffer.clear();
        let color_mode = self.config.color_mode;
        let primary = &self.config.primary_color;
        let secondary = &self.config.secondary_color;
        let accent = &self.config.accent_color;
        let palette = &self.palette;
        let emitter_speed = self.emitter_speed;

        for p in self.pool.iter() {
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
/// clamped into [0, 1] here as well as in `clamp_to_bounds`, so a config
/// that skipped clamping still samples sanely.
fn build_palette(config: &ParticleFxConfig) -> Palette {
    let mut palette: Palette = config
        .color_stops
        .iter()
        .flatten()
        .map(|stop| (stop.offset.clamp(0.0, 1.0), hex_to_rgb(&stop.color)))
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

    let mut triggers = track.triggers.clone();
    triggers.sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap_or(std::cmp::Ordering::Equal));
    // `grid_step` is monotonic in its argument, so chronological order is
    // also fire-step order.
    let triggers = triggers
        .iter()
        .map(|trig| BakedTrigger { fire_step: grid_step(trig.time.max(0.0)), kind: trig.kind })
        .collect();

    // `clamp_to_bounds` enforces this range for well-behaved hosts, but
    // `Simulation` can be built directly from unvalidated JSON, so the
    // ceiling must hold regardless. `max`/`min` rather than `clamp`: a
    // NaN duration must collapse to 0.0 here, and `clamp` would
    // propagate it.
    #[allow(clippy::manual_clamp)]
    let duration_cap = track.duration.max(0.0).min(MAX_EMITTER_TRACK_DURATION);

    Some(Arc::new(BakedTrack { keyframes: track.keyframes.clone(), triggers, duration_cap }))
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
        config.lifetime_min = 2.0;
        config.lifetime_max = 2.0;
        let mut sim = Simulation::new(config, 1);

        sim.trigger_burst();
        assert_eq!(sim.particle_count(), 1);
        sim.advance(TICK); // life 0 -> 1, still alive
        assert_eq!(sim.particle_count(), 1);
        sim.advance(TICK); // life 1 -> 2, 2 >= max_life => removed
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
        // Bitwise, so -0.0 and +0.0 count as different, as they do to the
        // harnesses, and finite first, since identical NaNs share bits.
        // Read as the flat floats a host sees, so every field is covered.
        let bits = |sim: &Simulation| -> Vec<u32> {
            let buffer = sim.buffer();
            let len = std::mem::size_of_val(buffer) / std::mem::size_of::<f32>();
            // SAFETY: `ParticleInstance` is `#[repr(C)]` and all `f32` (its
            // stride is asserted at compile time), so this is `len`
            // initialized, aligned f32s.
            let floats = unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<f32>(), len) };
            assert!(floats.iter().all(|f| f.is_finite()), "buffer contains NaN or inf");
            floats.iter().map(|f| f.to_bits()).collect()
        };
        assert_eq!(bits(&fresh), bits(&rewound));
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
    }

    #[test]
    fn seek_with_no_track_is_a_no_op() {
        let mut sim = Simulation::new(base_config(), 1);
        sim.seek(1.0);
        assert_eq!(sim.particle_count(), 0);
    }

    #[test]
    fn seek_start_continuous_then_stop_continuous_actually_toggles_spawning() {
        let mut config = base_config();
        config.emitter.spawn_rate_while_active = 10.0;
        config.emitter_track = Some(EmitterTrack {
            duration: 2.0,
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
}
