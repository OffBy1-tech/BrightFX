use brightfx_core::schema::{EmitterKeyframe, EmitterTrack, EmitterTrigger, TriggerKind};
use brightfx_core::{MAX_EMITTER_TRACK_DURATION, MAX_PREROLL, PLAYBACK_STEP};

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
/// `StartContinuous` at the start of the timeline. The core spawns within
/// ±2 px of the emitter, so the path is the area the effect covers.
/// Keyframe velocities are pinned to zero so the sweep does not leak into
/// the particles through `velocityInheritance` or the trail pattern's
/// direction; the host decides whether a moving emitter should throw.
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
        // serializes as `-0.0` and would change the bytes of any track
        // generated with a zero pre-roll (the tests, and any future caller).
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

#[cfg(test)]
mod tests {
    use super::*;
    use brightfx_core::{ParticleFxConfig, Simulation};

    #[test]
    fn alternates_between_the_two_ends_every_half_period() {
        let track = sweep_track((0.0, -20.0), (1920.0, -20.0), 1.0, 2.0, 0.0);
        let times: Vec<f32> = track.keyframes.iter().map(|k| k.time).collect();
        assert_eq!(times, vec![0.0, 0.5, 1.0, 1.5, 2.0]);
        let xs: Vec<f32> = track.keyframes.iter().map(|k| k.x).collect();
        assert_eq!(xs, vec![0.0, 1920.0, 0.0, 1920.0, 0.0]);
        assert!(track.keyframes.iter().all(|k| k.y == -20.0));
    }

    #[test]
    fn velocities_are_pinned_to_zero() {
        let track = sweep_track((0.0, 0.0), (10.0, 10.0), 1.0, 1.0, 0.0);
        assert!(track.keyframes.iter().all(|k| k.vx == Some(0.0) && k.vy == Some(0.0)));
    }

    #[test]
    fn starts_continuous_emission_at_the_track_start_and_nothing_else() {
        let track = sweep_track((0.0, 0.0), (10.0, 10.0), 1.0, 600.0, 0.0);
        assert_eq!(track.duration, 600.0);
        assert_eq!(
            track.triggers,
            vec![EmitterTrigger { time: 0.0, kind: TriggerKind::StartContinuous }]
        );
    }

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
        assert!(track.triggers[0].time.is_sign_positive(), "a zero pre-roll must write 0.0, not -0.0");
    }

    #[test]
    fn a_bad_preroll_is_clamped_like_the_core_clamps_it() {
        assert_eq!(sweep_track((0.0, 0.0), (10.0, 0.0), 1.0, 2.0, -3.0).preroll, 0.0);
        assert_eq!(sweep_track((0.0, 0.0), (10.0, 0.0), 1.0, 2.0, f32::NAN).preroll, 0.0);
        let capped = sweep_track((0.0, 0.0), (10.0, 0.0), 1.0, 2.0, 1e9);
        assert_eq!(capped.preroll, MAX_PREROLL);
        assert!(capped.keyframes[0].time <= -MAX_PREROLL);
    }

    #[test]
    fn the_final_keyframe_survives_a_half_period_that_does_not_divide_exactly() {
        // 0.9 / 0.3 is 2.9999998 in f32; without the tolerance the last
        // keyframe disappears and the sweep stops one end short.
        let track = sweep_track((0.0, 0.0), (10.0, 0.0), 0.6, 0.9, 0.0);
        assert_eq!(track.keyframes.len(), 4);
        let last = track.keyframes.last().unwrap().time;
        assert!((last - 0.9).abs() <= 1e-6, "last keyframe at {last}, wanted 0.9");
    }

    #[test]
    fn the_full_cap_ends_on_its_duration_for_a_leg_that_is_not_exact_in_f32() {
        // 600 / 0.3f32 is 1999.99992, and the old absolute 1e-4 tolerance
        // was too small to see that as 2000: the sweep dropped its last
        // leg and stopped at 599.7 s.
        let track = sweep_track((0.0, 0.0), (10.0, 0.0), 0.6, MAX_EMITTER_TRACK_DURATION, 0.0);
        assert_eq!(track.keyframes.len(), 2001);
        assert_eq!(track.keyframes.last().unwrap().time, MAX_EMITTER_TRACK_DURATION);
    }

    #[test]
    fn a_non_finite_duration_produces_one_keyframe_and_a_zero_duration() {
        for bad in [f32::NAN, f32::INFINITY] {
            let track = sweep_track((0.0, 0.0), (10.0, 0.0), 1.0, bad, 0.0);
            assert_eq!(track.keyframes.len(), 1, "duration {bad}");
            assert_eq!(track.keyframes[0].time, 0.0);
            assert_eq!(track.duration, 0.0);
        }
    }

    #[test]
    fn a_huge_duration_is_clamped_to_the_cores_ceiling() {
        let track = sweep_track((0.0, 0.0), (10.0, 0.0), 1.0, 1e9, 0.0);
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
        config.emitter_track = Some(sweep_track((0.0, 0.0), (1000.0, 0.0), 0.0, 1.0, 0.0));
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
