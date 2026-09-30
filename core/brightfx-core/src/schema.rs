use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The config schema version this build writes. Version 2 added the
/// `random-palette` colour mode and the `capsule` shape; nothing was
/// renamed or removed, so a version 1 config is a valid version 2 body.
pub const SCHEMA_VERSION: u32 = 2;

/// The oldest config schema version this build still reads. `AbiSimulation`
/// accepts `MIN_SCHEMA_VERSION..=SCHEMA_VERSION`, migrates an older config
/// forward on load, and rejects anything else -- so a build that predates
/// a version reports "unsupported schemaVersion" rather than misreading it.
pub const MIN_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ParticleShape {
    Circle,
    SparkleStar,
    GlowDisc,
    Ring,
    ShardCrystal,
    PlasmaOrb,
    SmokePuff,
    LightningBolt,
    Bubble,
    Heart,
    SakuraPetal,
    Diamond,
    Rune,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum BlendMode {
    SourceOver,
    Lighter,
    Screen,
    ColorDodge,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum EmissionPattern {
    Trail,
    RadialBurst,
    VortexSpiral,
    Fountain,
    Orbit,
    DirectionalCone,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ColorMode {
    Single,
    GradientLifetime,
    RainbowCycle,
    SpeedResponsive,
    MultiPalette,
    /// Each particle is dealt one `color_stops` entry, uniformly, and keeps
    /// it for life. Offsets do not weight the pick; they set the order the
    /// stops are dealt from, which is what a live stop edit follows.
    RandomPalette,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SizeCurve {
    LinearShrink,
    GrowShrink,
    Constant,
    PopFade,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Category {
    Elemental,
    Cyber,
    Cosmic,
    Nature,
    Magic,
    Custom,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ColorStop {
    pub offset: f32,
    pub color: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum TriggerKind {
    Burst,
    StartContinuous,
    StopContinuous,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EmitterKeyframe {
    pub time: f32,
    pub x: f32,
    pub y: f32,
    pub vx: Option<f32>,
    pub vy: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EmitterTrigger {
    pub time: f32,
    pub kind: TriggerKind,
}

/// Upper bound for `EmitterTrack.duration`, enforced by `clamp_to_bounds`
/// and used as a hard ceiling by `Simulation::seek` even for configs that
/// skip clamping -- keeps a corrupt or malicious `duration` from driving an
/// unbounded number of step iterations.
///
/// Public so track generators (`brightfx-tracks`) can refuse to bake
/// triggers the simulation will never reach instead of discovering the
/// ceiling only when `seek` silently stops short of them.
pub const MAX_EMITTER_TRACK_DURATION: f32 = 600.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EmitterTrack {
    pub duration: f32,
    pub keyframes: Vec<EmitterKeyframe>,
    pub triggers: Vec<EmitterTrigger>,
}

/// Replaces Mouseflare's cursor-specific spawn fields (`spawnRateOnMove`,
/// `spawnBurstOnClick`, `spawnRateIdle`) with the same shape, driven by
/// whatever position/velocity the host feeds `Simulation::set_emitter`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EmitterConfig {
    pub spawn_rate_while_active: f32,
    pub spawn_burst_size: u32,
    pub spawn_rate_idle: f32,
    pub emission_pattern: EmissionPattern,
    pub emission_angle: f32,
    pub emission_spread: f32,
    pub velocity_inheritance: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ParticleFxConfig {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub category: Category,
    pub description: String,
    pub author: Option<String>,
    pub icon: String,

    pub emitter: EmitterConfig,
    /// Optional keyframed authoring data. When present, `Simulation::seek`
    /// can drive the whole effect standalone for preview/export.
    pub emitter_track: Option<EmitterTrack>,

    pub shape: ParticleShape,
    pub blend_mode: BlendMode,
    pub glow_bloom: bool,
    pub glow_radius: f32,

    pub initial_speed_min: f32,
    pub initial_speed_max: f32,
    pub gravity_x: f32,
    pub gravity_y: f32,
    pub drag: f32,
    pub turbulence: f32,
    pub vortex_attraction: f32,
    pub rotation_speed_min: f32,
    pub rotation_speed_max: f32,

    pub lifetime_min: f32,
    pub lifetime_max: f32,
    pub start_size: f32,
    pub peak_size: f32,
    pub end_size: f32,
    pub size_curve: SizeCurve,

    pub color_mode: ColorMode,
    pub primary_color: String,
    pub secondary_color: String,
    pub accent_color: String,
    pub color_stops: Option<Vec<ColorStop>>,
    pub rainbow_speed: f32,
    pub start_alpha: f32,
    pub peak_alpha: f32,
    pub end_alpha: f32,

    pub sound_on_spawn: Option<bool>,
}

impl ParticleFxConfig {
    /// Clamps every numeric field with a documented valid range (per
    /// Mouseflare's original `fxEditor.ts` comments) into that range,
    /// tolerating out-of-range values from hand-edited JSON instead of
    /// rejecting the whole config. Returns the field names that were
    /// changed, so a host UI can surface a warning.
    pub fn clamp_to_bounds(&mut self) -> Vec<&'static str> {
        let mut changed = Vec::new();
        let mut clamp = |value: &mut f32, min: f32, max: f32, name: &'static str| {
            let clamped = value.clamp(min, max);
            if clamped != *value {
                *value = clamped;
                changed.push(name);
            }
        };

        clamp(&mut self.emitter.spawn_rate_while_active, 0.0, 20.0, "emitter.spawnRateWhileActive");
        clamp(&mut self.emitter.spawn_rate_idle, 0.0, 5.0, "emitter.spawnRateIdle");
        clamp(&mut self.emitter.emission_angle, 0.0, 360.0, "emitter.emissionAngle");
        clamp(&mut self.emitter.emission_spread, 0.0, 360.0, "emitter.emissionSpread");
        clamp(&mut self.emitter.velocity_inheritance, 0.0, 1.0, "emitter.velocityInheritance");
        clamp(&mut self.glow_radius, 0.0, 30.0, "glowRadius");
        clamp(&mut self.initial_speed_min, 0.0, 15.0, "initialSpeedMin");
        clamp(&mut self.initial_speed_max, 0.0, 15.0, "initialSpeedMax");
        clamp(&mut self.gravity_x, -3.0, 3.0, "gravityX");
        clamp(&mut self.gravity_y, -3.0, 3.0, "gravityY");
        clamp(&mut self.drag, 0.85, 1.0, "drag");
        clamp(&mut self.turbulence, 0.0, 5.0, "turbulence");
        clamp(&mut self.vortex_attraction, -3.0, 3.0, "vortexAttraction");
        clamp(&mut self.rotation_speed_min, -10.0, 10.0, "rotationSpeedMin");
        clamp(&mut self.rotation_speed_max, -10.0, 10.0, "rotationSpeedMax");
        clamp(&mut self.lifetime_min, 10.0, 300.0, "lifetimeMin");
        clamp(&mut self.lifetime_max, 10.0, 300.0, "lifetimeMax");
        clamp(&mut self.start_size, 1.0, 40.0, "startSize");
        clamp(&mut self.peak_size, 1.0, 40.0, "peakSize");
        clamp(&mut self.end_size, 0.0, 40.0, "endSize");
        clamp(&mut self.rainbow_speed, 0.0, 10.0, "rainbowSpeed");
        clamp(&mut self.start_alpha, 0.0, 1.0, "startAlpha");
        clamp(&mut self.peak_alpha, 0.0, 1.0, "peakAlpha");
        clamp(&mut self.end_alpha, 0.0, 1.0, "endAlpha");

        if let Some(track) = self.emitter_track.as_mut() {
            clamp(&mut track.duration, 0.0, MAX_EMITTER_TRACK_DURATION, "emitterTrack.duration");
        }

        // Reported once for the whole list: field names are static strings,
        // so per-index names would have to be leaked.
        let mut stops_changed = false;
        for stop in self.color_stops.iter_mut().flatten() {
            let offset = stop.offset.clamp(0.0, 1.0);
            if offset != stop.offset {
                stop.offset = offset;
                stops_changed = true;
            }
        }
        if stops_changed {
            changed.push("colorStops");
        }

        let burst = self.emitter.spawn_burst_size.min(60);
        if burst != self.emitter.spawn_burst_size {
            self.emitter.spawn_burst_size = burst;
            changed.push("emitter.spawnBurstSize");
        }

        // Ordered pairs, after each side is in range: an inverted pair is
        // repaired by raising max to min, the way Mouseflare's macOS
        // renderer reads `max(min, max)` at spawn time. Only max is
        // reported -- min was the value the author left within range.
        let mut order = |min: f32, max: &mut f32, max_name: &'static str| {
            if *max < min {
                *max = min;
                changed.push(max_name);
            }
        };
        order(self.initial_speed_min, &mut self.initial_speed_max, "initialSpeedMax");
        order(self.rotation_speed_min, &mut self.rotation_speed_max, "rotationSpeedMax");
        order(self.lifetime_min, &mut self.lifetime_max, "lifetimeMax");

        changed
    }
}

impl Default for ParticleFxConfig {
    /// A neutral, in-bounds starting effect — what an authoring tool opens
    /// on "new effect", and what a simulation runs before any config is
    /// loaded across the FFI boundary.
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            id: "untitled".into(),
            name: "Untitled".into(),
            category: Category::Custom,
            description: String::new(),
            author: None,
            icon: String::new(),
            emitter: EmitterConfig {
                spawn_rate_while_active: 8.0,
                spawn_burst_size: 12,
                spawn_rate_idle: 0.0,
                emission_pattern: EmissionPattern::Trail,
                emission_angle: 0.0,
                emission_spread: 30.0,
                velocity_inheritance: 0.5,
            },
            emitter_track: None,
            shape: ParticleShape::Circle,
            blend_mode: BlendMode::Lighter,
            glow_bloom: true,
            glow_radius: 8.0,
            initial_speed_min: 1.0,
            initial_speed_max: 3.0,
            gravity_x: 0.0,
            gravity_y: 0.5,
            drag: 0.98,
            turbulence: 0.0,
            vortex_attraction: 0.0,
            rotation_speed_min: 0.0,
            rotation_speed_max: 1.0,
            lifetime_min: 30.0,
            lifetime_max: 60.0,
            start_size: 4.0,
            peak_size: 6.0,
            end_size: 0.0,
            size_curve: SizeCurve::GrowShrink,
            color_mode: ColorMode::GradientLifetime,
            primary_color: "#ffffff".into(),
            secondary_color: "#8899ff".into(),
            accent_color: "#ffffff".into(),
            color_stops: None,
            rainbow_speed: 0.0,
            start_alpha: 1.0,
            peak_alpha: 1.0,
            end_alpha: 0.0,
            sound_on_spawn: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn particle_shape_serializes_kebab_case() {
        assert_eq!(
            serde_json::to_string(&ParticleShape::SparkleStar).unwrap(),
            "\"sparkle-star\""
        );
        let back: ParticleShape = serde_json::from_str("\"glow-disc\"").unwrap();
        assert_eq!(back, ParticleShape::GlowDisc);
    }

    #[test]
    fn color_stop_round_trips() {
        let stop = ColorStop {
            offset: 0.5,
            color: "#ff0000".to_string(),
        };
        let json = serde_json::to_string(&stop).unwrap();
        let back: ColorStop = serde_json::from_str(&json).unwrap();
        assert_eq!(stop, back);
    }

    #[test]
    fn trigger_kind_serializes_camel_case() {
        assert_eq!(
            serde_json::to_string(&TriggerKind::StartContinuous).unwrap(),
            "\"startContinuous\""
        );
    }

    fn example_config() -> ParticleFxConfig {
        ParticleFxConfig {
            schema_version: SCHEMA_VERSION,
            id: "test-fx".into(),
            name: "Test FX".into(),
            category: Category::Elemental,
            description: "A test effect".into(),
            author: Some("Off By 1".into()),
            icon: "fire".into(),
            emitter: EmitterConfig {
                spawn_rate_while_active: 10.0,
                spawn_burst_size: 20,
                spawn_rate_idle: 0.0,
                emission_pattern: EmissionPattern::Trail,
                emission_angle: 0.0,
                emission_spread: 30.0,
                velocity_inheritance: 0.5,
            },
            emitter_track: None,
            shape: ParticleShape::Circle,
            blend_mode: BlendMode::Lighter,
            glow_bloom: true,
            glow_radius: 10.0,
            initial_speed_min: 1.0,
            initial_speed_max: 3.0,
            gravity_x: 0.0,
            gravity_y: 1.0,
            drag: 0.98,
            turbulence: 0.5,
            vortex_attraction: 0.0,
            rotation_speed_min: 0.0,
            rotation_speed_max: 2.0,
            lifetime_min: 30.0,
            lifetime_max: 60.0,
            start_size: 4.0,
            peak_size: 6.0,
            end_size: 0.0,
            size_curve: SizeCurve::GrowShrink,
            color_mode: ColorMode::GradientLifetime,
            primary_color: "#ff6600".into(),
            secondary_color: "#ffcc00".into(),
            accent_color: "#ffffff".into(),
            color_stops: None,
            rainbow_speed: 0.0,
            start_alpha: 1.0,
            peak_alpha: 1.0,
            end_alpha: 0.0,
            sound_on_spawn: None,
        }
    }

    #[test]
    fn particle_fx_config_round_trips_through_json() {
        let config = example_config();
        let json = serde_json::to_string(&config).unwrap();
        let back: ParticleFxConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config, back);
    }

    #[test]
    fn particle_fx_config_uses_camel_case_field_names() {
        let config = example_config();
        let json = serde_json::to_value(&config).unwrap();
        assert!(json.get("schemaVersion").is_some());
        assert!(json.get("primaryColor").is_some());
        assert!(json.get("emitter").is_some());
    }

    #[test]
    fn json_schema_generation_includes_expected_properties() {
        let schema = schemars::schema_for!(ParticleFxConfig);
        let value = serde_json::to_value(&schema).unwrap();
        let properties = value.get("properties").unwrap();
        assert!(properties.get("primaryColor").is_some());
        assert!(properties.get("emitter").is_some());
        assert!(properties.get("schemaVersion").is_some());
    }

    #[test]
    fn out_of_range_values_get_clamped_and_reported() {
        let mut config = example_config();
        config.gravity_x = 999.0;
        config.drag = 5.0;
        config.emitter.spawn_burst_size = 9999;

        let changed = config.clamp_to_bounds();

        assert_eq!(config.gravity_x, 3.0);
        assert_eq!(config.drag, 1.0);
        assert_eq!(config.emitter.spawn_burst_size, 60);
        assert!(changed.contains(&"gravityX"));
        assert!(changed.contains(&"drag"));
        assert!(changed.contains(&"emitter.spawnBurstSize"));
    }

    #[test]
    fn rotation_speed_and_size_fields_get_clamped_and_reported() {
        let mut config = example_config();
        config.rotation_speed_min = -999.0;
        config.rotation_speed_max = 999.0;
        config.peak_size = 99999.0;
        config.end_size = -5.0;

        let changed = config.clamp_to_bounds();

        assert_eq!(config.rotation_speed_min, -10.0);
        assert_eq!(config.rotation_speed_max, 10.0);
        assert_eq!(config.peak_size, 40.0);
        assert_eq!(config.end_size, 0.0);
        assert!(changed.contains(&"rotationSpeedMin"));
        assert!(changed.contains(&"rotationSpeedMax"));
        assert!(changed.contains(&"peakSize"));
        assert!(changed.contains(&"endSize"));
    }

    #[test]
    fn an_inverted_lifetime_pair_raises_max_to_min_and_reports_max() {
        let mut config = example_config();
        config.lifetime_min = 100.0;
        config.lifetime_max = 10.0;

        let changed = config.clamp_to_bounds();

        assert_eq!(config.lifetime_min, 100.0);
        assert_eq!(config.lifetime_max, 100.0);
        assert_eq!(changed, vec!["lifetimeMax"]);
    }

    #[test]
    fn inverted_speed_and_rotation_pairs_are_repaired_too() {
        let mut config = example_config();
        config.initial_speed_min = 9.0;
        config.initial_speed_max = 2.0;
        config.rotation_speed_min = 5.0;
        config.rotation_speed_max = -5.0;

        let changed = config.clamp_to_bounds();

        assert_eq!(config.initial_speed_max, 9.0);
        assert_eq!(config.rotation_speed_max, 5.0);
        assert!(changed.contains(&"initialSpeedMax"));
        assert!(changed.contains(&"rotationSpeedMax"));
        assert!(!changed.contains(&"initialSpeedMin"));
        assert!(!changed.contains(&"rotationSpeedMin"));
    }

    #[test]
    fn a_five_second_lifetime_is_in_bounds_for_baked_playback() {
        // A baked preset's particle is the on-screen object and has to
        // cross the frame, which takes up to 4.5 s; 300 steps is 5 s.
        let mut config = example_config();
        config.lifetime_min = 300.0;
        config.lifetime_max = 300.0;

        let changed = config.clamp_to_bounds();

        assert!(changed.is_empty(), "{changed:?}");
        assert_eq!(config.lifetime_max, 300.0);
    }

    #[test]
    fn a_pair_that_only_inverts_after_range_clamping_is_still_repaired() {
        // min clamps down to 300 which is above an in-range max of 60
        let mut config = example_config();
        config.lifetime_min = 500.0;
        config.lifetime_max = 60.0;

        let changed = config.clamp_to_bounds();

        assert_eq!(config.lifetime_min, 300.0);
        assert_eq!(config.lifetime_max, 300.0);
        assert!(changed.contains(&"lifetimeMin"));
        assert!(changed.contains(&"lifetimeMax"));
    }

    #[test]
    fn color_stop_offsets_outside_the_unit_range_get_clamped_and_reported() {
        let mut config = example_config();
        config.color_stops = Some(vec![
            ColorStop { offset: -0.5, color: "#ff0000".into() },
            ColorStop { offset: 0.5, color: "#00ff00".into() },
            ColorStop { offset: 7.0, color: "#0000ff".into() },
        ]);

        let changed = config.clamp_to_bounds();

        let offsets: Vec<f32> = config.color_stops.unwrap().iter().map(|s| s.offset).collect();
        assert_eq!(offsets, vec![0.0, 0.5, 1.0]);
        assert_eq!(changed, vec!["colorStops"]);
    }

    #[test]
    fn in_range_color_stops_are_not_reported() {
        let mut config = example_config();
        config.color_stops = Some(vec![
            ColorStop { offset: 0.0, color: "#ff0000".into() },
            ColorStop { offset: 1.0, color: "#0000ff".into() },
        ]);
        let changed = config.clamp_to_bounds();
        assert!(changed.is_empty());
    }

    #[test]
    fn in_range_values_are_left_alone_and_not_reported() {
        let mut config = example_config();
        config.gravity_y = 1.0; // already in range from example_config()
        let changed = config.clamp_to_bounds();
        assert_eq!(config.gravity_y, 1.0);
        assert!(changed.is_empty());
    }

    #[test]
    fn an_oversized_emitter_track_duration_gets_clamped_and_reported() {
        let mut config = example_config();
        config.emitter_track = Some(EmitterTrack {
            duration: 1_000_000.0,
            keyframes: vec![],
            triggers: vec![],
        });

        let changed = config.clamp_to_bounds();

        assert_eq!(config.emitter_track.unwrap().duration, MAX_EMITTER_TRACK_DURATION);
        assert!(changed.contains(&"emitterTrack.duration"));
    }

    #[test]
    fn a_config_with_no_emitter_track_is_unaffected_by_the_duration_clamp() {
        let mut config = example_config();
        assert!(config.emitter_track.is_none());
        let changed = config.clamp_to_bounds();
        assert!(changed.is_empty());
    }

    #[test]
    fn the_default_config_declares_the_current_schema_version() {
        assert_eq!(ParticleFxConfig::default().schema_version, SCHEMA_VERSION);
    }

    #[test]
    fn the_default_config_is_already_within_bounds() {
        let mut config = ParticleFxConfig::default();
        let changed = config.clamp_to_bounds();
        assert!(changed.is_empty(), "default config needed clamping: {changed:?}");
    }

    #[test]
    fn the_default_config_round_trips_through_json() {
        let config = ParticleFxConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let back: ParticleFxConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config, back);
    }
}
