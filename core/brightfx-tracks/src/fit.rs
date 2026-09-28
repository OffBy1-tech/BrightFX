use brightfx_core::ParticleFxConfig;

/// Rescales a config's emitter-track positions from the frame it was
/// authored in to the frame it will play in: `x` and `vx` by the width
/// ratio, `y` and `vy` by the height ratio. Sizes, speeds, lifetimes, and
/// every other field pass through untouched: a preset's look is tuned in
/// pixels, and the composition applies its own per-aspect edits.
///
/// Every dimension must be a positive finite number, as the core's
/// `set_viewport` requires. A NaN or infinite ratio would serialize the
/// keyframes as `null`, and a zero target would fold the track onto the
/// origin, both behind an `ok:true`.
pub fn fit_track(
    mut config: ParticleFxConfig,
    from: (f32, f32),
    to: (f32, f32),
) -> Result<ParticleFxConfig, String> {
    for (name, value) in [("from.width", from.0), ("from.height", from.1), ("to.width", to.0), ("to.height", to.1)] {
        if !value.is_finite() || value <= 0.0 {
            return Err(format!("{name} must be a positive finite number, got {value}"));
        }
    }
    let sx = to.0 / from.0;
    let sy = to.1 / from.1;
    if let Some(track) = config.emitter_track.as_mut() {
        for k in &mut track.keyframes {
            k.x *= sx;
            k.y *= sy;
            k.vx = k.vx.map(|v| v * sx);
            k.vy = k.vy.map(|v| v * sy);
        }
    }
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use brightfx_core::schema::{EmitterKeyframe, EmitterTrack};

    #[allow(clippy::field_reassign_with_default)]
    fn authored() -> ParticleFxConfig {
        let mut config = ParticleFxConfig::default();
        config.emitter_track = Some(EmitterTrack {
            duration: 10.0,
            keyframes: vec![
                EmitterKeyframe { time: 0.0, x: 0.0, y: 0.0, vx: None, vy: None },
                EmitterKeyframe { time: 5.0, x: 1920.0, y: 540.0, vx: Some(96.0), vy: Some(-27.0) },
            ],
            triggers: vec![],
        });
        config
    }

    fn keyframes(config: &ParticleFxConfig) -> &[EmitterKeyframe] {
        &config.emitter_track.as_ref().unwrap().keyframes
    }

    #[test]
    fn positions_and_velocities_scale_by_axis() {
        let fitted = fit_track(authored(), (1920.0, 1080.0), (1080.0, 1920.0)).unwrap();
        let k = &keyframes(&fitted)[1];
        assert!((k.x - 1080.0).abs() < 1e-3);
        assert!((k.y - 960.0).abs() < 1e-3);
        assert!((k.vx.unwrap() - 54.0).abs() < 1e-3);
        assert!((k.vy.unwrap() - (-48.0)).abs() < 1e-3);
        assert_eq!(keyframes(&fitted)[0].vx, None, "absent velocities stay absent");
    }

    #[test]
    fn everything_else_passes_through() {
        let fitted = fit_track(authored(), (1920.0, 1080.0), (1080.0, 1920.0)).unwrap();
        let mut expected = authored();
        expected.emitter_track = fitted.emitter_track.clone();
        assert_eq!(fitted, expected);
    }

    #[test]
    fn round_trip_returns_to_the_authored_frame() {
        let there = fit_track(authored(), (1920.0, 1080.0), (1080.0, 1920.0)).unwrap();
        let back = fit_track(there, (1080.0, 1920.0), (1920.0, 1080.0)).unwrap();
        for (a, b) in keyframes(&back).iter().zip(keyframes(&authored())) {
            assert!((a.x - b.x).abs() < 1e-3 && (a.y - b.y).abs() < 1e-3);
        }
    }

    #[test]
    fn a_config_without_a_track_is_unchanged() {
        let config = ParticleFxConfig::default();
        assert_eq!(fit_track(config.clone(), (1920.0, 1080.0), (1080.0, 1920.0)).unwrap(), config);
    }

    #[test]
    fn a_non_positive_or_non_finite_dimension_is_rejected() {
        // A NaN ratio would serialize every keyframe as null and a zero
        // target would collapse the track to the origin, both with ok:true;
        // the core's set_viewport rejects a zero dimension the same way.
        for (from, to, name) in [
            ((0.0, 1080.0), (1080.0, 1920.0), "from.width"),
            ((1920.0, -1.0), (1080.0, 1920.0), "from.height"),
            ((1920.0, 1080.0), (f32::NAN, 1920.0), "to.width"),
            ((1920.0, 1080.0), (1080.0, f32::INFINITY), "to.height"),
            ((1920.0, 1080.0), (0.0, 0.0), "to.width"),
        ] {
            let err = fit_track(authored(), from, to).unwrap_err();
            assert!(err.contains(name), "expected {name} in: {err}");
        }
    }
}
