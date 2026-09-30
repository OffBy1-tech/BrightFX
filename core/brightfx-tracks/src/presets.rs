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
//! `spawnRateWhileActive * lifetime` and its lifetime ceiling of 300
//! steps is five seconds of travel. Speeds are px per step, so the
//! Remotion components' "px per 30 fps frame" halve on the way in.
//!
//! A particle dies when its life runs out, not when it leaves the frame,
//! so that population includes the ones already off screen. A rain sized
//! to reach the bottom of the 1080×1920 delivery frame spends half its
//! life below a 1080-high one, which is why the rains' pools run to
//! ~270-380 to keep ~115-170 in a 1920×1080 frame. `MAX_PARTICLES` is
//! 500; `tests/presets.rs` holds every preset to 400 at steady state.
//!
//! The same test holds every preset to dying out of sight, in both
//! frames. The rains do it with margin in speed and life, not a fade:
//! the core's alpha curve runs from peak to `endAlpha` over the last 80%
//! of life, so a fade to 0 would dim every drop well before the bottom
//! of a 1920 px frame. A rain also takes its fall time, 2-4 s, to fill
//! the frame from the top.
//!
//! The multicolour presets deal colours with `RandomPalette`, not
//! `RainbowCycle`. `RainbowCycle` advances the hue *per spawn*, so hue
//! follows spawn order, and spawn order follows the emitter along its
//! path: confetti streaked along its scatter chords, and a rain falling
//! from an edge read as one top-to-bottom colour ramp. Dealing each
//! particle a colour from the seeded RNG breaks that link wherever the
//! particle lands.

use brightfx_core::schema::{ColorStop, EmitterKeyframe, EmitterTrack, EmitterTrigger, TriggerKind};
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

/// The rainbow the two multicolour presets deal from: eight hues 45°
/// apart at the saturation and lightness `RainbowCycle` draws with
/// (hsl(h, 90%, 60%)), so they keep the look that mode gave them.
const RAINBOW: [&str; 8] = ["#F53D3D", "#F5C73D", "#99F53D", "#3DF56B", "#3DF5F5", "#3D6BF5", "#993DF5", "#F53DC7"];

/// `colors` as `colorStops` for `RandomPalette`, which deals them out
/// uniformly and ignores the offsets; they are spread evenly anyway so
/// the stops still read sensibly in an editor that shows them.
fn palette(colors: &[&str]) -> Option<Vec<ColorStop>> {
    let last = (colors.len().max(2) - 1) as f32;
    Some(
        colors
            .iter()
            .enumerate()
            .map(|(i, c)| ColorStop { offset: i as f32 / last, color: (*c).into() })
            .collect(),
    )
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

/// Rainbow diamonds tumbling down the whole frame, about 200 chunky
/// pieces of confetti. A 0.15 s scatter leg spreads each dozen along a
/// chord of the frame, and each piece is dealt one of the eight
/// `RAINBOW` colours, so neighbours along a chord differ.
///
/// Density, size, and alpha are set by how the preset reads in a real
/// full-frame composition, not in a thumbnail: dropped into a busy,
/// brightly lit music-video frame (#6), 0.95 per step at size 9 with a
/// 0.6 `startAlpha` barely registered. 1.8 per step holds ~216 pieces,
/// and size 16 -- `size` is roughly the diamond's half-extent -- matches
/// a 14×20 rounded rectangle. Both were matched against a hand-rolled
/// layer at 1920×1080 and 1080×1920.
///
/// Pieces are born inside the frame, so they cannot simply wink out at
/// the end of their life. `endAlpha` is 0 so the exit is a fade. The
/// entrance is not faded: `startAlpha` 1 is what the reference layer
/// showed, and a softer one left a visible share of the pieces
/// semi-transparent. Pieces therefore appear in place at full strength.
/// A longer life alone does not cure that -- a piece born mid-frame
/// still has to appear somewhere. Spawning above the frame and falling
/// through it would, and no longer costs a colour ramp, but crossing a
/// 1920 px frame inside the 5 s ceiling takes ~6.5 px/step: that is a
/// rain, not confetti drifting at 1–2.5. The drift is kept.
pub fn confetti() -> ParticleFxConfig {
    let mut c = base(
        "confetti",
        "Confetti",
        "Rainbow diamonds drifting down across the whole frame",
        "confetti",
        scatter_sweep(-40.0, FRAME_H, 0.15),
    );
    c.emitter = emitter(1.8, 30, EmissionPattern::DirectionalCone, 90.0, 40.0);
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
    c.start_size = 16.0;
    c.peak_size = 16.0;
    c.end_size = 16.0;
    c.size_curve = SizeCurve::Constant;
    c.color_mode = ColorMode::RandomPalette;
    c.color_stops = palette(&RAINBOW);
    c.start_alpha = 1.0;
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
/// These were tuned when a particle lived 2 s at most, where a
/// full-height rise would have been a frantic one-second crossing, and
/// took the other half of the trade. The 5 s ceiling now allows the
/// component's slow full-height rise; this preset has not been retuned
/// for it. As it stands: a
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

/// Rainbow capsules falling from just above the top edge: `SprinkleRain`,
/// 70 pieces of 8×20 px crossing the frame, each dealt one of the eight
/// `RAINBOW` colours. The emitter wanders the edge out of order (a
/// `scatter_sweep` at one height) rather than sweeping it, so spawn
/// order does not line up across the frame either.
///
/// The pieces are the component's capsules: `size` 10 is a capsule's
/// half-length, so 20 px long and 20 / 2.3 = 8.7 px across. They spin at
/// 0.035–0.1 rad/step, the component's 120–340°/s.
///
/// The fall is near the component's 2.2 s: 7.5–9 px/step with `gravityY`
/// only 0.05, so the speed stays near constant and the rain does not
/// crowd the top (density goes as 1 / speed). Turbulence 0.8 is the
/// sway; it also gives each piece a fixed drift of up to ±1.5 px/step
/// (see `frosting_rain`), so the slowest mean fall is 6 px/step, which
/// with the gravity clears a 1920 px frame inside the 300-step life.
/// Measured over 30 s at 9:16: none of 1620 deaths inside the frame.
///
/// The spawn rate overshoots the component's 70: 0.9 per step keeps
/// ~115 in a 1920×1080 frame, the density this preset has shipped with,
/// in a pool of ~270.
pub fn sprinkle_rain() -> ParticleFxConfig {
    let mut c = base(
        "sprinkle-rain",
        "Sprinkle Rain",
        "Rainbow sprinkles falling fast from the top edge",
        "sprinkle",
        scatter_sweep(-20.0, -20.0, 0.1),
    );
    c.emitter = emitter(0.9, 30, EmissionPattern::DirectionalCone, 90.0, 20.0);
    c.shape = ParticleShape::Capsule;
    c.blend_mode = BlendMode::SourceOver;
    c.glow_bloom = false;
    c.initial_speed_min = 7.5;
    c.initial_speed_max = 9.0;
    c.gravity_y = 0.05;
    c.drag = 1.0;
    c.turbulence = 0.8;
    c.rotation_speed_min = 0.035;
    c.rotation_speed_max = 0.1;
    c.lifetime_min = 300.0;
    c.lifetime_max = 300.0;
    c.start_size = 10.0;
    c.peak_size = 10.0;
    c.end_size = 10.0;
    c.size_curve = SizeCurve::Constant;
    c.color_mode = ColorMode::RandomPalette;
    c.color_stops = palette(&RAINBOW);
    c.start_alpha = 0.95;
    c.peak_alpha = 0.95;
    c.end_alpha = 0.95;
    c
}

/// Pink frosting drops raining from the top edge: `FrostingRain`, 80
/// capsules of 9×20 px. The same shape and spin as the sprinkles; the
/// soft palette and the slower fall are what separate the two rains.
/// `size` 10–11 is the capsule's half-length, so 20–22 px long and
/// 8.7–9.6 px across.
///
/// Denser rather than bigger is what reads as frosting at full-frame
/// scale (#6): ~170 drops in a 1920×1080 frame, over twice the
/// component's count. Its first round, as 8–9 px dots, 35–45 of them all
/// but vanished against a pink background plate, and 15–16 px dots read
/// as large white bubbles; at the component's own capsule size, the
/// count is what carries it.
///
/// The fall is the component's: a 1080 px frame in about 2.2 s at a near
/// constant 7.5–8.5 px/step. Two things keep it near constant.
/// `gravityY` is only 0.05, since density goes as 1 / speed and an
/// accelerating rain crowds the top. And turbulence is 0.4: its per-step
/// push integrates to a fixed per-drop drift of up to ±0.15 * turbulence
/// / 0.08 px/step, which at the sprinkles' old 2.2 was ±4 -- enough at
/// this speed to stall some drops and let them die short of the bottom.
/// At 0.4 the slowest mean fall is 7.5 - 0.75 = 6.75 px/step, which
/// clears a 1920 px frame in ~260 steps, inside the 270–280-step life.
/// Measured over 30 s at 9:16: none of ~2,430 deaths inside the frame.
/// 1.35 per step holds ~170 in a 1920×1080 frame, in a pool of ~376.
///
/// The palette is the component's six colours -- two pinks, white, and
/// yellow, purple, and mint accents -- dealt one per drop. Soft accents
/// rather than `sprinkle-rain`'s full-strength spectrum are what keep
/// the two rains apart. The earlier pink → white → pink lifetime
/// gradient dropped the accents and read as mostly white (#6).
pub fn frosting_rain() -> ParticleFxConfig {
    let mut c = base(
        "frosting-rain",
        "Frosting Rain",
        "Pink frosting drops raining from the top edge",
        "frosting",
        sweep_track((0.0, -20.0), (FRAME_W, -20.0), 0.5, TRACK_DURATION),
    );
    c.emitter = emitter(1.35, 30, EmissionPattern::DirectionalCone, 90.0, 15.0);
    c.shape = ParticleShape::Capsule;
    c.blend_mode = BlendMode::SourceOver;
    c.glow_bloom = false;
    c.initial_speed_min = 7.5;
    c.initial_speed_max = 8.5;
    c.gravity_y = 0.05;
    c.drag = 1.0;
    c.turbulence = 0.4;
    c.rotation_speed_min = 0.035;
    c.rotation_speed_max = 0.1;
    c.lifetime_min = 270.0;
    c.lifetime_max = 280.0;
    c.start_size = 10.0;
    c.peak_size = 11.0;
    c.end_size = 10.0;
    c.size_curve = SizeCurve::GrowShrink;
    c.color_mode = ColorMode::RandomPalette;
    c.color_stops = palette(&["#FF5D8F", "#FF8FC8", "#FFFFFF", "#FFE066", "#B98CFF", "#8FE3B0"]);
    c.primary_color = "#FF5D8F".into();
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
/// `FlyingDonuts` crosses 2200 px (the frame plus 140 px each side) in
/// 2.8–4.5 s, at constant speed, arcing 260–410 px up and back down. This
/// is that motion as a projectile: no `gravityX`, so the horizontal speed
/// is constant, and a launch at 330° ± 2° and 11–12 px/step puts it at
/// 9.3–10.6 px/step across -- a 3.5–3.9 s crossing. The rise is
/// `vy² / 2g`: `gravityY` 0.6 is 0.06 px/step² after the core's 0.1
/// scale, and the vertical launch speeds of 5.2–6.4 px/step give
/// 220–340 px. That is lower than the component's top end on purpose: the
/// throws start 464–648 px down a 1080 px frame, and a 60–120 px glyph at
/// a higher apex clips the top edge. Measured over 2 min, the highest
/// centre is y = 144 at 1920×1080 and y = 529 fitted to 1080×1920. The
/// 300-step life outlasts the slowest crossing, so nothing vanishes on
/// screen.
pub fn flight_arc() -> ParticleFxConfig {
    let mut c = base(
        "flight-arc",
        "Flight Arc",
        "Objects thrown in from the left, arcing right across the frame",
        "arc",
        sweep_track((-100.0, FRAME_H * 0.43), (-100.0, FRAME_H * 0.6), 3.0, TRACK_DURATION),
    );
    c.emitter = emitter(0.035, 4, EmissionPattern::DirectionalCone, 330.0, 4.0);
    c.shape = ParticleShape::Circle;
    c.blend_mode = BlendMode::SourceOver;
    c.glow_bloom = false;
    c.initial_speed_min = 11.0;
    c.initial_speed_max = 12.0;
    c.gravity_x = 0.0;
    c.gravity_y = 0.6;
    c.drag = 1.0;
    c.rotation_speed_min = 0.02;
    c.rotation_speed_max = 0.05;
    c.lifetime_min = 300.0;
    c.lifetime_max = 300.0;
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
