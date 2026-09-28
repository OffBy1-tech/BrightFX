//! Generates and verifies the cross-target forward-seek fixture.
//!
//! `ffi-seek` seeks backward, which replays from zero. This one seeks a
//! strictly increasing sequence on a single handle, which after the
//! incremental-seek change steps forward from the previous position. Every
//! harness replays the sequence, compares to the recorded buffer, and then
//! seeks a fresh handle straight to the last time and requires a
//! bit-identical buffer. Run with `BRIGHTFX_REGENERATE=1` to rewrite
//! `ffi-seek-forward.expected.json` after an intentional simulation change.

mod common;

use brightfx_core::abi::AbiSimulation;
use common::LIBM_DRIFT_TOLERANCE;
use std::path::PathBuf;

const SEED: u64 = 42;
/// Strictly increasing and all on the 1/60 s grid, so every seek is a
/// forward step and the last one is a grid point a fresh seek lands on.
const SEEK_TIMES: [f32; 4] = [0.5, 1.0, 1.25, 1.75];

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures")
}

fn load_sim() -> AbiSimulation {
    let config = std::fs::read_to_string(fixtures_dir().join("ffi-seek.config.json"))
        .expect("seek fixture config missing");
    let mut sim = AbiSimulation::new(SEED);
    let envelope: serde_json::Value =
        serde_json::from_str(&sim.set_config(&config)).expect("envelope is not JSON");
    assert_eq!(envelope["ok"], true, "seek fixture config rejected: {envelope}");
    sim
}

fn run_protocol() -> (u32, Vec<f32>) {
    let mut sim = load_sim();
    for time in SEEK_TIMES {
        sim.seek(time);
    }
    (sim.particle_count(), sim.buffer_slice().to_vec())
}

#[test]
fn the_fixture_matches_the_recorded_expectation() {
    let (count, buffer) = run_protocol();
    let path = fixtures_dir().join("ffi-seek-forward.expected.json");

    if std::env::var("BRIGHTFX_REGENERATE").is_ok() {
        let json = serde_json::json!({
            "seed": SEED,
            "seekTimes": SEEK_TIMES,
            "particleFloats": AbiSimulation::PARTICLE_FLOATS,
            "tolerance": LIBM_DRIFT_TOLERANCE,
            "particleCount": count,
            "buffer": buffer,
        });
        std::fs::write(&path, serde_json::to_string_pretty(&json).unwrap()).unwrap();
        eprintln!("regenerated {}", path.display());
        return;
    }

    let expected: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&path)
            .expect("expected fixture missing -- run with BRIGHTFX_REGENERATE=1 to create it"),
    )
    .unwrap();
    assert_eq!(expected["seed"].as_u64().unwrap(), SEED, "fixture seed drifted");
    let seek_times: Vec<f32> = expected["seekTimes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_f64().unwrap() as f32)
        .collect();
    assert_eq!(seek_times, SEEK_TIMES, "seek times drifted");
    assert_eq!(expected["particleFloats"].as_u64().unwrap() as u32, AbiSimulation::PARTICLE_FLOATS);
    assert_eq!(expected["tolerance"].as_f64().unwrap() as f32, LIBM_DRIFT_TOLERANCE);
    assert_eq!(expected["particleCount"].as_u64().unwrap() as u32, count);
    let expected_buffer: Vec<f32> = expected["buffer"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_f64().unwrap() as f32)
        .collect();
    assert_eq!(expected_buffer.len(), buffer.len(), "buffer length changed");
    for (index, (actual, wanted)) in buffer.iter().zip(&expected_buffer).enumerate() {
        assert!(
            (actual - wanted).abs() <= LIBM_DRIFT_TOLERANCE,
            "float {index} drifted: got {actual}, expected {wanted}"
        );
    }
}

#[test]
fn the_protocol_actually_produces_particles() {
    let (count, buffer) = run_protocol();
    assert!(count > 0, "forward-seek fixture is vacuous -- no particles");
    assert!(buffer.iter().all(|v| v.is_finite()), "fixture contains NaN or inf");
}

#[test]
fn forward_seeking_is_bit_identical_to_a_fresh_seek() {
    let (count, buffer) = run_protocol();
    let mut fresh = load_sim();
    fresh.seek(SEEK_TIMES[SEEK_TIMES.len() - 1]);
    assert_eq!(fresh.particle_count(), count);
    assert_eq!(fresh.buffer_slice(), &buffer[..]);
}
