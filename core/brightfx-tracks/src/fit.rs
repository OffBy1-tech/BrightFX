use brightfx_core::ParticleFxConfig;

/// The core spawns a particle within +/-2 px of the emitter.
const SPAWN_JITTER: f32 = 2.0;

/// How far turbulence swings a particle sideways, per unit of `turbulence`.
/// The core adds `0.15 * turbulence` to a velocity each step along a sine
/// of 0.1 rad per step in x and 0.08 in y, so the swing's amplitude is
/// `0.15 / 0.1 / 0.1` = 15 px per unit in x and `0.15 / 0.08 / 0.08` = 23.4
/// in y. This is the larger, rounded up.
const SWAY_PER_TURBULENCE: f32 = 24.0;

/// The `cullMargin` that holds an emitter's excursion `excursion` px outside
/// the frame: the excursion, plus the spawn jitter, plus the sway of a
/// particle with `turbulence`. A particle born near the edge of that margin
/// and swung out past it is culled for heading away although the sway would
/// bring it back, so a margin that stops at the emitter loses drops that
/// would have been on the frame. Measured on `sprinkle-rain` (turbulence
/// 0.8, emitter 60 px outside the frame): 4 px past the emitter lost
/// on-frame drops, 10 px did not.
pub fn emitter_cull_margin(excursion: f32, turbulence: f32) -> f32 {
    excursion + SPAWN_JITTER + SWAY_PER_TURBULENCE * turbulence
}

/// Rescales a config's emitter-track positions from the frame it was
/// authored in to the frame it will play in: `x` and `vx` by the width
/// ratio, `y` and `vy` by the height ratio. Sizes, speeds, lifetimes, and
/// every other field pass through untouched: a preset's look is tuned in
/// pixels, and the composition applies its own per-aspect edits.
///
/// The one exception is `cullMargin`, which is in pixels but has to keep
/// covering the emitter. A preset whose emitter sweeps outside its frame
/// sets a margin that holds that sweep, so a drop born out there is not
/// culled for heading away when its turbulence would bring it back. Fitting
/// stretches the sweep with the frame and not the margin, so a margin that
/// held the sweep at 1920 px does not at 3840. When the fitted track reaches
/// farther outside the target frame than the margin does, the margin is
/// raised to `emitter_cull_margin` for that distance. It is never lowered,
/// never set on a config that has none, and untouched when the emitter stays
/// inside the frame.
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
    if let (Some(margin), Some(track)) = (config.cull_margin, config.emitter_track.as_ref()) {
        // Keyframes are the extremes: the emitter moves in straight lines
        // between them.
        let excursion = track
            .keyframes
            .iter()
            .map(|k| (-k.x).max(k.x - to.0).max(-k.y).max(k.y - to.1))
            .fold(0.0_f32, f32::max);
        if excursion > 0.0 {
            config.cull_margin = Some(margin.max(emitter_cull_margin(excursion, config.turbulence)));
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

    /// `authored()` with a cull margin and the track's two keyframes moved to
    /// (`left`, `top`) and (`right`, 540).
    fn swept(margin: Option<f32>, left: f32, right: f32, top: f32) -> ParticleFxConfig {
        let mut config = authored();
        config.cull_margin = margin;
        let track = config.emitter_track.as_mut().unwrap();
        track.keyframes[0].x = left;
        track.keyframes[0].y = top;
        track.keyframes[1].x = right;
        config
    }

    #[test]
    fn a_cull_margin_grows_to_cover_the_emitter_s_fitted_excursion() {
        // The emitter sweeps 60 px past each side of a 1920 px frame. Fitted
        // to 4K it sweeps 120 px past each side, outside a margin of 24 that
        // was enough before, and a drop born out there and heading outward
        // would be culled although it could drift back on.
        let fitted = fit_track(swept(Some(24.0), -60.0, 1980.0, 0.0), (1920.0, 1080.0), (3840.0, 2160.0)).unwrap();
        assert_eq!(fitted.cull_margin, Some(122.0), "120 px excursion plus 2 px of spawn jitter");
    }

    #[test]
    fn the_margin_also_covers_the_turbulent_sway() {
        // A drop does not only start at the emitter: turbulence swings it
        // sideways by about 24 px per unit, and one that swings out past the
        // margin and back is lost. Measured on `sprinkle-rain` (turbulence
        // 0.8): 4 px past the emitter lost on-frame drops, 10 did not.
        let mut config = swept(Some(24.0), -60.0, 1980.0, 0.0);
        config.turbulence = 0.8;
        let fitted = fit_track(config, (1920.0, 1080.0), (3840.0, 2160.0)).unwrap();
        let margin = fitted.cull_margin.unwrap();
        assert!((margin - (120.0 + 2.0 + 24.0 * 0.8)).abs() < 1e-3, "margin was {margin}");
    }

    #[test]
    fn the_vertical_excursion_counts_too() {
        // y = -20 in a 1080 px frame is -35.55... in a 1920 px one.
        let fitted = fit_track(swept(Some(24.0), 0.0, 1920.0, -20.0), (1920.0, 1080.0), (1080.0, 1920.0)).unwrap();
        let margin = fitted.cull_margin.unwrap();
        assert!((margin - (20.0 * 1920.0 / 1080.0 + 2.0)).abs() < 1e-3, "margin was {margin}");
    }

    #[test]
    fn a_cull_margin_is_never_lowered_or_invented() {
        let wide = fit_track(swept(Some(500.0), -60.0, 1980.0, 0.0), (1920.0, 1080.0), (3840.0, 2160.0)).unwrap();
        assert_eq!(wide.cull_margin, Some(500.0), "a margin that already covers the path stays");
        let none = fit_track(swept(None, -60.0, 1980.0, 0.0), (1920.0, 1080.0), (3840.0, 2160.0)).unwrap();
        assert_eq!(none.cull_margin, None, "culling stays off when the preset did not ask for it");
        // An emitter inside the frame needs no more than the author chose,
        // including a margin of 0.
        let inside = fit_track(swept(Some(0.0), 0.0, 1920.0, 0.0), (1920.0, 1080.0), (3840.0, 2160.0)).unwrap();
        assert_eq!(inside.cull_margin, Some(0.0));
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
