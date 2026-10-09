use brightfx_core::schema::{EmitterKeyframe, EmitterTrack, EmitterTrigger, TriggerKind};
use brightfx_core::{MAX_EMITTER_TRACK_DURATION, PLAYBACK_STEP};

/// Shortest sweep period the core can actually play back.
///
/// `Simulation::seek` samples an `EmitterTrack` once per `PLAYBACK_STEP`.
/// A sweep's half-period is `period / 2`, so a period below
/// `2 * PLAYBACK_STEP` puts more than one direction reversal inside a
/// single sample: the sampler lands on the same phase every step and the
/// emitter never visibly leaves `a`. Flooring here keeps the slowest legal
/// sweep at exactly one sample per end.
pub const MIN_SWEEP_PERIOD: f32 = 2.0 * PLAYBACK_STEP;

/// How many whole `leg`-second steps fit into `duration`.
///
/// Counted arithmetically rather than by stepping a float accumulator
/// until it passes `duration`: `i as f32 * leg` can overshoot by an ULP
/// and silently drop the final keyframe.
///
/// The quotient is taken in f64 and the tolerance is *relative*, because
/// `leg` is an f32 and so carries its own rounding: 600 / 0.15f32 is
/// 3999.99984, not 4000, and 600 / 0.3f32 is 1999.99992. A fixed absolute
/// epsilon of 1e-4 is smaller than that error once the quotient runs to
/// thousands, which used to drop the last leg and end a 600 s track at
/// 599.7-599.85 s. A relative 1e-6 keeps scaling with the quotient.
pub(crate) fn leg_count(duration: f32, leg: f32) -> usize {
    let quotient = duration as f64 / leg as f64;
    if !quotient.is_finite() || quotient <= 0.0 {
        return 0;
    }
    let nearest = quotient.round();
    let legs = if (quotient - nearest).abs() <= quotient * 1e-6 { nearest } else { quotient.floor() };
    legs as usize
}

/// A triangle-wave emitter path: the emitter travels from `a` to `b` and
/// back once per `period` seconds, for `duration` seconds, with a single
/// `StartContinuous` at time 0. The core spawns within ±2 px of the
/// emitter, so a preset that must cover an edge or the whole frame moves
/// the emitter across it. Keyframe velocities are pinned to zero so the
/// sweep does not leak into the particles through `velocityInheritance`
/// or the trail pattern's direction.
///
/// `period` is floored at [`MIN_SWEEP_PERIOD`] and `duration` is sanitized
/// into `0..=MAX_EMITTER_TRACK_DURATION` (a non-finite duration becomes 0),
/// so no input can ask for a keyframe list the core would never play.
pub fn sweep_track(a: (f32, f32), b: (f32, f32), period: f32, duration: f32) -> EmitterTrack {
    let half = period.max(MIN_SWEEP_PERIOD) / 2.0;
    let duration = if duration.is_finite() {
        duration.clamp(0.0, MAX_EMITTER_TRACK_DURATION)
    } else {
        0.0
    };

    let count = leg_count(duration, half) + 1;

    let mut keyframes = Vec::with_capacity(count);
    for i in 0..count {
        // `.min(duration)` because a whole leg can land a fraction past the
        // end (600 s at a 0.3 s leg ends at 600.00006): a keyframe beyond
        // the track's own duration is one the core would never reach.
        let time = (i as f32 * half).min(duration);
        let (x, y) = if i.is_multiple_of(2) { a } else { b };
        keyframes.push(EmitterKeyframe { time, x, y, vx: Some(0.0), vy: Some(0.0) });
    }
    EmitterTrack {
        duration,
        preroll: 0.0,
        keyframes,
        triggers: vec![EmitterTrigger { time: 0.0, kind: TriggerKind::StartContinuous }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use brightfx_core::{ParticleFxConfig, Simulation};

    #[test]
    fn alternates_between_the_two_ends_every_half_period() {
        let track = sweep_track((0.0, -20.0), (1920.0, -20.0), 1.0, 2.0);
        let times: Vec<f32> = track.keyframes.iter().map(|k| k.time).collect();
        assert_eq!(times, vec![0.0, 0.5, 1.0, 1.5, 2.0]);
        let xs: Vec<f32> = track.keyframes.iter().map(|k| k.x).collect();
        assert_eq!(xs, vec![0.0, 1920.0, 0.0, 1920.0, 0.0]);
        assert!(track.keyframes.iter().all(|k| k.y == -20.0));
    }

    #[test]
    fn velocities_are_pinned_to_zero() {
        let track = sweep_track((0.0, 0.0), (10.0, 10.0), 1.0, 1.0);
        assert!(track.keyframes.iter().all(|k| k.vx == Some(0.0) && k.vy == Some(0.0)));
    }

    #[test]
    fn starts_continuous_emission_at_zero_and_nothing_else() {
        let track = sweep_track((0.0, 0.0), (10.0, 10.0), 1.0, 600.0);
        assert_eq!(track.duration, 600.0);
        assert_eq!(
            track.triggers,
            vec![EmitterTrigger { time: 0.0, kind: TriggerKind::StartContinuous }]
        );
    }

    #[test]
    fn the_final_keyframe_survives_a_half_period_that_does_not_divide_exactly() {
        // 0.9 / 0.3 is 2.9999998 in f32; without the tolerance the last
        // keyframe disappears and the sweep stops one end short.
        let track = sweep_track((0.0, 0.0), (10.0, 0.0), 0.6, 0.9);
        assert_eq!(track.keyframes.len(), 4);
        let last = track.keyframes.last().unwrap().time;
        assert!((last - 0.9).abs() <= 1e-6, "last keyframe at {last}, wanted 0.9");
    }

    #[test]
    fn the_full_cap_ends_on_its_duration_for_a_leg_that_is_not_exact_in_f32() {
        // 600 / 0.3f32 is 1999.99992, and the old absolute 1e-4 tolerance
        // was too small to see that as 2000: the sweep dropped its last
        // leg and stopped at 599.7 s.
        let track = sweep_track((0.0, 0.0), (10.0, 0.0), 0.6, MAX_EMITTER_TRACK_DURATION);
        assert_eq!(track.keyframes.len(), 2001);
        assert_eq!(track.keyframes.last().unwrap().time, MAX_EMITTER_TRACK_DURATION);
    }

    #[test]
    fn a_non_finite_duration_produces_one_keyframe_and_a_zero_duration() {
        for bad in [f32::NAN, f32::INFINITY] {
            let track = sweep_track((0.0, 0.0), (10.0, 0.0), 1.0, bad);
            assert_eq!(track.keyframes.len(), 1, "duration {bad}");
            assert_eq!(track.keyframes[0].time, 0.0);
            assert_eq!(track.duration, 0.0);
        }
    }

    #[test]
    fn a_huge_duration_is_clamped_to_the_cores_ceiling() {
        let track = sweep_track((0.0, 0.0), (10.0, 0.0), 1.0, 1e9);
        assert_eq!(track.duration, MAX_EMITTER_TRACK_DURATION);
        // 600 s at a 0.5 s half-period: 1200 intervals, 1201 keyframes.
        assert_eq!(track.keyframes.len(), 1201);
        assert_eq!(track.keyframes.last().unwrap().time, 600.0);
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn a_sub_step_period_still_moves_the_emitter_across_the_sweep() {
        // The defect this guards: a half-period below PLAYBACK_STEP aliases
        // so the emitter is sampled at the `a` end on every grid point and
        // the whole sweep collapses to a point. Assert positions, not
        // keyframe counts -- the count was right while the motion was not.
        let mut config = ParticleFxConfig::default();
        config.emitter_track = Some(sweep_track((0.0, 0.0), (1000.0, 0.0), 0.0, 1.0));
        config.emitter.spawn_rate_while_active = 20.0;
        // Particles spawn within +-2 px of the emitter; freeze them there.
        config.initial_speed_min = 0.0;
        config.initial_speed_max = 0.0;
        config.gravity_x = 0.0;
        config.gravity_y = 0.0;
        config.turbulence = 0.0;
        config.lifetime_min = 120.0;
        config.lifetime_max = 120.0;

        let mut sim = Simulation::new(config, 7);
        let mut near_a = false;
        let mut near_b = false;
        for step in 1..=12u32 {
            sim.seek(step as f32 * PLAYBACK_STEP);
            for p in sim.buffer() {
                if p.x < 5.0 {
                    near_a = true;
                }
                if p.x > 995.0 {
                    near_b = true;
                }
            }
        }
        assert!(near_a && near_b, "emitter never left one end (a: {near_a}, b: {near_b})");
    }
}
