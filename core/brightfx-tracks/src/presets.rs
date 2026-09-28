//! The preset library's source of truth. `presets/*.brightfx.json` are this
//! module rendered to JSON by `examples/gen_presets.rs`; the test in
//! `tests/presets.rs` fails if they drift. Every preset is authored in a
//! 1920×1080 pixel frame and carries a `MAX_EMITTER_TRACK_DURATION` track
//! with one `StartContinuous` at 0, so `amount` and `window` in the
//! composition, not the track, decide when it shows.
//!
//! The core spawns within ±2 px of the emitter, so presets that cover an
//! area move the emitter: `sweep_track` for an edge, `scatter_sweep` for
//! the whole frame.
//!
//! Two numbers drive every preset and are worth stating once. The core
//! steps at `PLAYBACK_STEP` (1/60 s) and measures `lifetime` in those
//! steps, so a preset's on-screen population is
//! `spawnRateWhileActive * lifetime` and its lifetime ceiling of 120
//! steps is two seconds of travel. Speeds are px per step, so the
//! Remotion components' "px per 30 fps frame" halve on the way in.
//!
//! One consequence of that shapes both rainbow presets and is worth
//! stating once. `RainbowCycle` advances the hue by `rainbowSpeed * 0.5`
//! degrees *per spawn*, so the number of full hue cycles alive at any
//! moment is `population * rainbowSpeed * 0.5 / 360` -- at the
//! `rainbowSpeed` ceiling of 10 that is `population / 72`, whatever the
//! spawn rate or the lifetime. At the components' densities that is about
//! one cycle: the spectrum is laid out exactly once across the live
//! particles. If their positions are then a monotonic function of their
//! age, as in a rain falling from an edge, the frame reads as one
//! top-to-bottom colour ramp. The fix is to break the age-to-position
//! map, not to chase the hue -- see `sprinkle_rain`.

use brightfx_core::schema::{EmitterKeyframe, EmitterTrack, EmitterTrigger, TriggerKind};
use brightfx_core::{
    BlendMode, Category, ColorMode, EmissionPattern, EmitterConfig, ParticleFxConfig, ParticleShape, SizeCurve,
    MAX_EMITTER_TRACK_DURATION, PLAYBACK_STEP,
};

use crate::sweep_track;

pub const FRAME_W: f32 = 1920.0;
pub const FRAME_H: f32 = 1080.0;
/// The presets run for as long as the core will replay a track, so a
/// composition can window them anywhere without the track running out.
pub const TRACK_DURATION: f32 = MAX_EMITTER_TRACK_DURATION;
/// How far past the left and right edges `scatter_sweep` reaches.
const SCATTER_MARGIN: f32 = 60.0;

/// A wandering emitter path: every `leg` seconds it arrives at another
/// pseudo-random point of the band `y_min..y_max`, sweeping the chord in
/// between, so particles spawned along the way land scattered over the
/// whole area rather than on a fixed set of rows.
///
/// The two coordinates come from multiplicative sequences with different
/// moduli, and the moduli are coprime with any plausible spawn cadence.
/// That matters: a triangle sweep between two fixed x ends aliases against
/// the spawn budget -- a 0.5-per-step rate and a six-step crossing put
/// every particle on one of four columns -- and the effect collapses into
/// stripes.
///
/// `leg` is floored at `PLAYBACK_STEP`, since a leg shorter than one step
/// is sampled at a single point and lays its particles in a clump.
///
/// `y_min == y_max` is allowed and useful: it degenerates to a wandering
/// *horizontal* path at a fixed height, which is what a rain wants when
/// its edge must be visited out of order.
fn scatter_sweep(y_min: f32, y_max: f32, leg: f32) -> EmitterTrack {
    let leg = leg.max(PLAYBACK_STEP);
    let count = crate::sweep::leg_count(TRACK_DURATION, leg) + 1;
    let mut keyframes = Vec::with_capacity(count);
    for i in 0..count {
        let i = i as u32;
        // Off the left and right edges by a margin, so the chords cover the
        // frame's own edges instead of turning back at them.
        let fx = ((i * 3571) % 10007) as f32 / 10007.0;
        let fy = ((i * 7919) % 10009) as f32 / 10009.0;
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
        keyframes,
        triggers: vec![EmitterTrigger { time: 0.0, kind: TriggerKind::StartContinuous }],
    }
}

/// `icon` is one short word a picker can show; it is per preset, so the
/// library does not present seven identical tiles.
fn base(id: &str, name: &str, description: &str, icon: &str, track: EmitterTrack) -> ParticleFxConfig {
    ParticleFxConfig {
        id: id.into(),
        name: name.into(),
        category: Category::Custom,
        description: description.into(),
        author: Some("Off By 1".into()),
        icon: icon.into(),
        emitter_track: Some(track),
        ..ParticleFxConfig::default()
    }
}

fn emitter(rate: f32, burst: u32, pattern: EmissionPattern, angle: f32, spread: f32) -> EmitterConfig {
    EmitterConfig {
        spawn_rate_while_active: rate,
        spawn_burst_size: burst,
        spawn_rate_idle: 0.0,
        emission_pattern: pattern,
        emission_angle: angle,
        emission_spread: spread,
        velocity_inheritance: 0.0,
    }
}

/// Rainbow diamonds tumbling down the whole frame, about 90 pieces
/// of confetti. A 0.15 s scatter leg spreads each dozen along a chord of
/// the frame.
///
/// `rainbowSpeed` sits at its ceiling because the hue a particle is drawn
/// with is its spawn hue plus `progress * 120`, and that second term runs
/// against the cycle: at 10 the palette still only sweeps ~330° over a
/// particle's life, and anything lower leaves a visible hole in the
/// spectrum (7.3 loses every blue and cyan). The banding the module doc
/// warns about does not bite here: the emitter scatters over the whole
/// frame, so a piece's height is where it was born, not how old it is.
///
/// Pieces are born inside the frame, so they cannot simply wink out at
/// the end of a flat two-second life. `endAlpha` is 0 so the exit is a
/// fade, and `startAlpha` 0.6 softens the entrance the same way. Both are
/// a workaround: the real cure is a life long enough to cross the frame,
/// which is the "Raise lifetimeMax for baked playback" issue (#24). The
/// spawn rate carries the cost of the fade -- 0.95 per step holds ~114
/// pieces, of which about 90 are above half alpha, which is the
/// component's count.
pub fn confetti() -> ParticleFxConfig {
    let mut c = base(
        "confetti",
        "Confetti",
        "Rainbow diamonds drifting down across the whole frame",
        "confetti",
        scatter_sweep(-40.0, FRAME_H, 0.15),
    );
    c.emitter = emitter(0.95, 30, EmissionPattern::DirectionalCone, 90.0, 40.0);
    c.shape = ParticleShape::Diamond;
    c.blend_mode = BlendMode::SourceOver;
    c.glow_bloom = false;
    c.initial_speed_min = 1.0;
    c.initial_speed_max = 2.5;
    c.gravity_y = 0.06;
    c.drag = 1.0;
    c.turbulence = 1.0;
    c.rotation_speed_min = 0.04;
    c.rotation_speed_max = 0.09;
    c.lifetime_min = 120.0;
    c.lifetime_max = 120.0;
    c.start_size = 9.0;
    c.peak_size = 9.0;
    c.end_size = 9.0;
    c.size_curve = SizeCurve::Constant;
    c.color_mode = ColorMode::RainbowCycle;
    c.rainbow_speed = 10.0;
    c.start_alpha = 0.6;
    c.peak_alpha = 1.0;
    c.end_alpha = 0.0;
    c
}

/// Soft yellow glows over the top 85% of the frame: `Fireflies`, 22 dots
/// of 5–14 px drifting and twinkling, in the band the component uses --
/// everything above `0.85 * height`, so the bottom strip stays clear for
/// captions. The twinkle is the alpha curve over a short life, so the
/// population turns over instead of pulsing in lockstep.
pub fn fireflies() -> ParticleFxConfig {
    let mut c = base(
        "fireflies",
        "Fireflies",
        "Soft yellow glows twinkling over the top 85% of the frame",
        "firefly",
        scatter_sweep(0.0, FRAME_H * 0.85, 0.08),
    );
    c.emitter = emitter(0.2, 12, EmissionPattern::RadialBurst, 0.0, 360.0);
    c.shape = ParticleShape::GlowDisc;
    c.blend_mode = BlendMode::Lighter;
    c.glow_bloom = true;
    c.glow_radius = 18.0;
    c.initial_speed_min = 0.1;
    c.initial_speed_max = 0.4;
    c.gravity_y = 0.0;
    c.drag = 0.98;
    c.turbulence = 2.0;
    c.lifetime_min = 90.0;
    c.lifetime_max = 120.0;
    c.start_size = 2.0;
    c.peak_size = 7.0;
    c.end_size = 2.0;
    c.size_curve = SizeCurve::GrowShrink;
    c.color_mode = ColorMode::Single;
    c.primary_color = "#FFE680".into();
    c.start_alpha = 0.0;
    c.peak_alpha = 1.0;
    c.end_alpha = 0.0;
    c
}

/// White-to-gold stars drifting up and fading. The effect it replaces
/// exists in two palettes; both rise 750–1100 px over 3–7 s.
///
/// A particle lives 2 s at most, so a full-height rise would be a frantic
/// one-second crossing. These take the other half of the trade: a
/// `DirectionalCone` straight up (270°) at 4.5–6.5 px/step with `drag`
/// just under 1 lifts a star 200–500 px, about 330 px typically, over a
/// 60–90 step life -- a rise that plainly reads as upward without the
/// star shooting off. The 60° spread and the turbulence keep them
/// drifting sideways as they climb rather than rising in a column, and
/// `gravity_y` is 0 so the drag alone decides the slow-down.
pub fn sparkles() -> ParticleFxConfig {
    let mut c = base(
        "sparkles",
        "Sparkles",
        "White-to-gold stars drifting up and fading; bursts for entrances",
        "sparkle",
        scatter_sweep(0.0, FRAME_H, 0.08),
    );
    c.emitter = emitter(0.4, 40, EmissionPattern::DirectionalCone, 270.0, 60.0);
    c.shape = ParticleShape::SparkleStar;
    c.blend_mode = BlendMode::Lighter;
    c.glow_bloom = true;
    c.glow_radius = 22.0;
    c.initial_speed_min = 4.5;
    c.initial_speed_max = 6.5;
    c.gravity_y = 0.0;
    c.drag = 0.995;
    c.turbulence = 1.0;
    c.rotation_speed_min = 0.02;
    c.rotation_speed_max = 0.08;
    c.lifetime_min = 60.0;
    c.lifetime_max = 90.0;
    c.start_size = 12.0;
    c.peak_size = 14.0;
    c.end_size = 0.0;
    c.size_curve = SizeCurve::PopFade;
    c.color_mode = ColorMode::GradientLifetime;
    c.primary_color = "#FFFFFF".into();
    c.secondary_color = "#FFE9A8".into();
    c.accent_color = "#FFD966".into();
    c.start_alpha = 0.0;
    c.peak_alpha = 1.0;
    c.end_alpha = 0.0;
    c
}

/// Rainbow capsules falling from above the top edge: `SprinkleRain`, 70
/// pieces of 8×20 px crossing the frame. The fall is faster than the
/// component's 2.2–4.0 s because a particle only lives 2 s; slower and the
/// rain would stop in mid-air.
///
/// The emitter does not sweep the top edge. It wanders a *band* 450 px
/// deep above it, which is what mixes the colours. Per the module doc, at
/// this density the rainbow lays its spectrum out exactly once across the
/// live particles, so as long as a piece's height is a clean function of
/// its age the frame is one vertical colour ramp; the earlier left-to-
/// right edge sweep gave exactly that (a cyan top grading to a pink
/// bottom). Spawning anywhere in a 450 px band decouples the two: the
/// piece at a given row may have fallen 450 px more than its neighbour,
/// which is ~40 steps -- about 170° of hue -- so every row carries several
/// hues at once. The wander also visits x out of order, which keeps the
/// residual within-sweep ordering from lining up into streaks.
///
/// The rest follows from that band. `gravityY` is only 0.05 so the fall
/// stays near constant speed and the band does not pile the rain at the
/// top; 10–13 px/step is then fast enough that even the deepest-spawned
/// piece clears the bottom edge inside its 2 s, so nothing winks out
/// mid-air.
///
/// The spawn rate is the one place this preset knowingly overshoots its
/// component. Mixing scales with population -- the module doc's cycle
/// count is `population / 72` -- and at the component's 70 pieces the
/// spectrum still reads as a warm top over a cool bottom. 1.15 per step
/// is a pool of ~138, about 100 of them inside the frame, and that is
/// where the rows stop sorting by hue at both t = 3 s and t = 6 s. A
/// longer `lifetimeMax` (issue #24) would buy the same mixing from a
/// deeper band instead. Turbulence is the component's sway.
pub fn sprinkle_rain() -> ParticleFxConfig {
    let mut c = base(
        "sprinkle-rain",
        "Sprinkle Rain",
        "Rainbow sprinkles falling fast from the top edge",
        "sprinkle",
        scatter_sweep(-450.0, -20.0, 0.1),
    );
    c.emitter = emitter(1.15, 30, EmissionPattern::DirectionalCone, 90.0, 20.0);
    c.shape = ParticleShape::ShardCrystal;
    c.blend_mode = BlendMode::SourceOver;
    c.glow_bloom = false;
    c.initial_speed_min = 10.0;
    c.initial_speed_max = 13.0;
    c.gravity_y = 0.05;
    c.drag = 1.0;
    c.turbulence = 2.2;
    c.rotation_speed_min = 0.04;
    c.rotation_speed_max = 0.1;
    c.lifetime_min = 120.0;
    c.lifetime_max = 120.0;
    c.start_size = 8.0;
    c.peak_size = 8.0;
    c.end_size = 8.0;
    c.size_curve = SizeCurve::Constant;
    c.color_mode = ColorMode::RainbowCycle;
    c.rainbow_speed = 10.0;
    c.start_alpha = 0.95;
    c.peak_alpha = 0.95;
    c.end_alpha = 0.95;
    c
}

/// Pink frosting drops raining from the top edge: `FrostingRain`, 80
/// rounded pieces. Rounder, larger, and denser than the sprinkles, which
/// is what separates the two rains on screen.
///
/// The component's palette is six colours (two pinks, white, yellow,
/// purple, green); this keeps the pink-and-white majority and drops the
/// rest, because a full spectrum here would be `sprinkle-rain` again. The
/// gradient runs pink → white → pink rather than ending on white, so the
/// fall does not read as one long fade.
pub fn frosting_rain() -> ParticleFxConfig {
    let mut c = base(
        "frosting-rain",
        "Frosting Rain",
        "Pink frosting drops raining from the top edge",
        "frosting",
        sweep_track((0.0, -20.0), (FRAME_W, -20.0), 0.5, TRACK_DURATION),
    );
    c.emitter = emitter(0.7, 30, EmissionPattern::DirectionalCone, 90.0, 15.0);
    c.shape = ParticleShape::Circle;
    c.blend_mode = BlendMode::SourceOver;
    c.glow_bloom = false;
    c.initial_speed_min = 8.0;
    c.initial_speed_max = 10.5;
    c.gravity_y = 0.4;
    c.drag = 1.0;
    c.turbulence = 2.2;
    c.lifetime_min = 120.0;
    c.lifetime_max = 120.0;
    c.start_size = 8.0;
    c.peak_size = 9.0;
    c.end_size = 8.0;
    c.size_curve = SizeCurve::GrowShrink;
    c.color_mode = ColorMode::GradientLifetime;
    c.primary_color = "#FF5D8F".into();
    c.secondary_color = "#FFFFFF".into();
    c.accent_color = "#FF8FC8".into();
    c.start_alpha = 0.95;
    c.peak_alpha = 0.95;
    c.end_alpha = 0.95;
    c
}

/// Pale bubbles rising from the bottom edge: `GriddleBubbles`, ten large
/// bubbles climbing the lower half and fading out. The palette rides the
/// lifetime gradient (warm → pink → pale blue) so the bubbles on screen,
/// all at different ages, show the component's four colours at once.
pub fn bubbles() -> ParticleFxConfig {
    let mut c = base(
        "bubbles",
        "Bubbles",
        "Pale bubbles rising from the bottom edge and fading out",
        "bubble",
        sweep_track((0.0, FRAME_H + 20.0), (FRAME_W, FRAME_H + 20.0), 0.6, TRACK_DURATION),
    );
    c.emitter = emitter(0.12, 20, EmissionPattern::DirectionalCone, 270.0, 30.0);
    c.shape = ParticleShape::Bubble;
    c.blend_mode = BlendMode::SourceOver;
    c.glow_bloom = false;
    c.initial_speed_min = 3.5;
    c.initial_speed_max = 5.0;
    c.gravity_y = -0.1;
    c.drag = 0.995;
    c.turbulence = 1.2;
    c.lifetime_min = 110.0;
    c.lifetime_max = 120.0;
    c.start_size = 10.0;
    c.peak_size = 20.0;
    c.end_size = 17.0;
    c.size_curve = SizeCurve::GrowShrink;
    c.color_mode = ColorMode::GradientLifetime;
    c.primary_color = "#FFE28A".into();
    c.secondary_color = "#FFD1E0".into();
    c.accent_color = "#CDE9FF".into();
    c.start_alpha = 0.9;
    c.peak_alpha = 0.9;
    c.end_alpha = 0.0;
    c
}

/// Sprite-mode preset: the motion of the flying donuts, pancakes, notes,
/// and gears. The composition supplies the glyph and scales `size` up
/// (the core caps size at 40; the glyphs are 60–120 px).
///
/// `FlyingDonuts` crosses the frame in 2.8–4.5 s; a particle lives 2 s, so
/// this crosses in 2 s. Even then 15 px/step — the speed cap — only covers
/// 1800 px of the 2100 px the object has to travel, so a rightward
/// `gravity_x` carries it the rest of the way. See the issue "Raise
/// lifetimeMax for baked playback".
pub fn flight_arc() -> ParticleFxConfig {
    let mut c = base(
        "flight-arc",
        "Flight Arc",
        "Objects thrown in from the left, arcing right across the frame",
        "arc",
        sweep_track((-100.0, FRAME_H * 0.43), (-100.0, FRAME_H * 0.6), 3.0, TRACK_DURATION),
    );
    c.emitter = emitter(0.06, 4, EmissionPattern::DirectionalCone, 318.0, 12.0);
    c.shape = ParticleShape::Circle;
    c.blend_mode = BlendMode::SourceOver;
    c.glow_bloom = false;
    c.initial_speed_min = 14.0;
    c.initial_speed_max = 15.0;
    c.gravity_x = 1.0;
    c.gravity_y = 1.7;
    c.drag = 1.0;
    c.rotation_speed_min = 0.02;
    c.rotation_speed_max = 0.05;
    c.lifetime_min = 120.0;
    c.lifetime_max = 120.0;
    c.start_size = 34.0;
    c.peak_size = 40.0;
    c.end_size = 36.0;
    c.size_curve = SizeCurve::GrowShrink;
    c.color_mode = ColorMode::Single;
    c.primary_color = "#FFFFFF".into();
    c.start_alpha = 1.0;
    c.peak_alpha = 1.0;
    c.end_alpha = 1.0;
    c
}

/// Every preset, by file stem, in the order they are documented.
pub fn library() -> Vec<(&'static str, ParticleFxConfig)> {
    vec![
        ("confetti", confetti()),
        ("fireflies", fireflies()),
        ("sparkles", sparkles()),
        ("sprinkle-rain", sprinkle_rain()),
        ("frosting-rain", frosting_rain()),
        ("bubbles", bubbles()),
        ("flight-arc", flight_arc()),
    ]
}

/// The exact bytes `gen_presets` writes for a preset: compact JSON plus a
/// trailing newline. Compact because the files are generated, not edited,
/// and the scatter presets carry thousands of keyframes.
pub fn render_json(config: &ParticleFxConfig) -> String {
    let mut json = serde_json::to_string(config).expect("preset serializes");
    json.push('\n');
    json
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_preset_is_inside_the_cores_bounds() {
        for (name, config) in library() {
            let mut clamped = config.clone();
            let changed = clamped.clamp_to_bounds();
            assert!(changed.is_empty(), "{name} is out of bounds: {changed:?}");
        }
    }

    #[test]
    fn the_scatter_sweep_covers_its_band_and_reaches_both_edges() {
        let track = scatter_sweep(0.0, FRAME_H * 0.85, 0.12);
        assert_eq!(track.duration, TRACK_DURATION);
        assert!(track.keyframes.iter().all(|k| k.y >= 0.0 && k.y <= FRAME_H * 0.85));
        assert!(track.keyframes.iter().all(|k| k.x >= -SCATTER_MARGIN && k.x <= FRAME_W + SCATTER_MARGIN));
        // Both edges and both ends of the band are visited inside one
        // particle lifetime (2 s), not merely somewhere in 600 s.
        let early = &track.keyframes[..(2.0 / 0.12) as usize];
        assert!(early.iter().any(|k| k.x < FRAME_W * 0.15));
        assert!(early.iter().any(|k| k.x > FRAME_W * 0.85));
        assert!(early.iter().any(|k| k.y < FRAME_H * 0.85 * 0.15));
        assert!(early.iter().any(|k| k.y > FRAME_H * 0.85 * 0.85));
        let last = track.keyframes.last().unwrap().time;
        assert!(last <= TRACK_DURATION && last > TRACK_DURATION - 0.12, "last keyframe at {last}");
    }

    #[test]
    fn a_sub_step_leg_is_floored_rather_than_collapsing_to_a_point() {
        let track = scatter_sweep(0.0, 100.0, 0.0);
        let dt = track.keyframes[1].time - track.keyframes[0].time;
        assert!((dt - PLAYBACK_STEP).abs() < 1e-6, "leg of {dt}s");
    }

    #[test]
    fn successive_scatter_legs_do_not_repeat_a_position_within_a_lifetime() {
        // The defect this guards: a path whose period divides the spawn
        // cadence puts every particle on a handful of columns.
        let track = scatter_sweep(0.0, FRAME_H, 0.12);
        let window = (2.0 / 0.12) as usize;
        for w in 1..window {
            assert!(
                track.keyframes[..window]
                    .iter()
                    .zip(track.keyframes[w..window + w].iter())
                    .all(|(a, b)| (a.x - b.x).abs() > 1.0 || (a.y - b.y).abs() > 1.0),
                "the path repeats every {w} legs"
            );
        }
    }

    #[test]
    fn every_presets_last_keyframe_lands_on_the_track_duration() {
        // A preset whose keyframes stop early leaves its emitter parked at
        // the last one for the rest of the track, so a composition
        // windowing the tail gets a stationary emitter instead of the
        // effect. The f32 quotient used to end tracks at 599.7-599.85 s.
        for (name, config) in library() {
            let track = config.emitter_track.expect("every preset carries a track");
            let last = track.keyframes.last().expect("every track has keyframes").time;
            assert!(
                (last - TRACK_DURATION).abs() < 1e-3,
                "{name}: last keyframe at {last}, not {TRACK_DURATION}"
            );
        }
    }

    #[test]
    fn every_preset_has_its_own_icon() {
        let mut icons: Vec<String> = library().into_iter().map(|(_, c)| c.icon).collect();
        icons.sort();
        let count = icons.len();
        icons.dedup();
        assert_eq!(icons.len(), count, "two presets share an icon: {icons:?}");
    }

    #[test]
    fn every_preset_is_named_after_its_file_stem() {
        for (name, config) in library() {
            assert_eq!(config.id, name);
        }
    }
}
