mod common;

use brightfx_core::*;
use common::LIBM_DRIFT_TOLERANCE;
use std::path::Path;

fn fire_config() -> ParticleFxConfig {
    ParticleFxConfig {
        schema_version: SCHEMA_VERSION,
        id: "golden-fire".into(),
        name: "Golden Fire".into(),
        category: Category::Elemental,
        description: "Regression fixture".into(),
        author: None,
        icon: "fire".into(),
        emitter: EmitterConfig {
            spawn_rate_while_active: 8.0,
            spawn_burst_size: 15,
            spawn_rate_idle: 0.5,
            emission_pattern: EmissionPattern::Fountain,
            emission_angle: 0.0,
            emission_spread: 40.0,
            velocity_inheritance: 0.3,
        },
        emitter_track: None,
        shape: ParticleShape::PlasmaOrb,
        blend_mode: BlendMode::Lighter,
        glow_bloom: true,
        glow_radius: 8.0,
        initial_speed_min: 1.0,
        initial_speed_max: 4.0,
        gravity_x: 0.0,
        gravity_y: -0.6,
        drag: 0.97,
        turbulence: 1.2,
        vortex_attraction: 0.0,
        rotation_speed_min: -1.0,
        rotation_speed_max: 1.0,
        spin_direction: SpinDirection::Fixed,
        lifetime_min: 40.0,
        lifetime_max: 80.0,
        cull_margin: None,
        start_size: 6.0,
        peak_size: 9.0,
        end_size: 0.0,
        size_curve: SizeCurve::GrowShrink,
        color_mode: ColorMode::GradientLifetime,
        primary_color: "#ff9900".into(),
        secondary_color: "#ff3300".into(),
        accent_color: "#330000".into(),
        color_stops: None,
        rainbow_speed: 0.0,
        start_alpha: 1.0,
        peak_alpha: 1.0,
        end_alpha: 0.0,
        sound_on_spawn: None,
    }
}

/// Runs 120 fixed ticks of a fixed config+seed and returns the final
/// buffer's particle count plus a rounded summary (avoids ULP-level float
/// noise breaking the comparison across machines) so the snapshot stays
/// meaningful without needing exact bit-for-bit float equality.
fn run_and_summarize() -> serde_json::Value {
    let mut sim = Simulation::new(fire_config(), 7);
    sim.set_emitter(0.0, 0.0, 0.0, 0.0, true);
    for _ in 0..120 {
        sim.advance(1.0 / 60.0);
    }
    // Widen to f64 *before* rounding: serde_json serializes an f32 via a
    // shortest-round-trip-as-f32 string, which reparses to a different f64
    // than a value that was f64 all along. Left as f32, otherwise-identical
    // particle values compare unequal after writing to the fixture file and
    // reading it back in a later test run -- rounding in f64 sidesteps it.
    let round = |v: f32| (((v as f64) * 1000.0).round()) / 1000.0;
    let summary: Vec<_> = sim
        .buffer()
        .iter()
        .map(|p| {
            serde_json::json!({
                "x": round(p.x), "y": round(p.y), "size": round(p.size),
                "rotation": round(p.rotation), "color": p.color.map(round),
            })
        })
        .collect();
    serde_json::json!({ "particleCount": sim.particle_count(), "particles": summary })
}

/// Numeric leaves are compared within `LIBM_DRIFT_TOLERANCE` rather than
/// bitwise (see `common` for why). Structure (object keys, array lengths,
/// non-numeric leaves) is still required to match exactly.
fn assert_values_close(actual: &serde_json::Value, expected: &serde_json::Value, path: &str) {
    use serde_json::Value;
    match (actual, expected) {
        (Value::Number(a), Value::Number(b)) => {
            let a = a.as_f64().expect("actual number is not representable as f64");
            let b = b.as_f64().expect("expected number is not representable as f64");
            let tolerance = LIBM_DRIFT_TOLERANCE as f64;
            assert!(
                (a - b).abs() < tolerance,
                "numeric mismatch at {path}: actual {a} vs expected {b} (tolerance {tolerance})"
            );
        }
        (Value::Array(a), Value::Array(b)) => {
            assert_eq!(
                a.len(),
                b.len(),
                "array length mismatch at {path}: actual {} vs expected {}",
                a.len(),
                b.len()
            );
            for (i, (av, bv)) in a.iter().zip(b.iter()).enumerate() {
                assert_values_close(av, bv, &format!("{path}[{i}]"));
            }
        }
        (Value::Object(a), Value::Object(b)) => {
            let mut a_keys: Vec<_> = a.keys().collect();
            let mut b_keys: Vec<_> = b.keys().collect();
            a_keys.sort();
            b_keys.sort();
            assert_eq!(
                a_keys, b_keys,
                "object keys mismatch at {path}: actual {a_keys:?} vs expected {b_keys:?}"
            );
            for key in a.keys() {
                assert_values_close(&a[key], &b[key], &format!("{path}.{key}"));
            }
        }
        (a, b) => {
            assert_eq!(a, b, "value mismatch at {path}: actual {a:?} vs expected {b:?}");
        }
    }
}

#[test]
fn golden_output_matches_committed_fixture() {
    let fixture_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/golden_fire.json");
    let actual = run_and_summarize();

    if std::env::var("BRIGHTFX_UPDATE_GOLDEN").is_ok() {
        std::fs::create_dir_all(fixture_path.parent().unwrap()).unwrap();
        std::fs::write(&fixture_path, serde_json::to_string_pretty(&actual).unwrap()).unwrap();
        return;
    }

    let expected_str = std::fs::read_to_string(&fixture_path).unwrap_or_else(|_| {
        panic!(
            "no golden fixture at {fixture_path:?} -- run with BRIGHTFX_UPDATE_GOLDEN=1 to create it"
        )
    });
    let expected: serde_json::Value = serde_json::from_str(&expected_str).unwrap();
    assert_values_close(&actual, &expected, "$");
}

#[test]
fn config_parses_from_a_raw_json_string_like_a_host_app_would_send() {
    let json = serde_json::to_string(&fire_config()).unwrap();
    let config: ParticleFxConfig = serde_json::from_str(&json).unwrap();
    let mut sim = Simulation::new(config, 1);
    sim.trigger_burst();
    assert_eq!(sim.particle_count(), 15);
}
