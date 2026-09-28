//! Scaffolding for `seek_fixture.rs` and `seek_forward_fixture.rs`.

use super::LIBM_DRIFT_TOLERANCE;
use brightfx_core::abi::AbiSimulation;
use std::path::PathBuf;

pub fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures")
}

/// A handle loaded with the shared seek config, ready to seek.
pub fn load_sim(seed: u64) -> AbiSimulation {
    let config = std::fs::read_to_string(fixtures_dir().join("ffi-seek.config.json"))
        .expect("seek fixture config missing");
    let mut sim = AbiSimulation::new(seed);
    let envelope: serde_json::Value =
        serde_json::from_str(&sim.set_config(&config)).expect("envelope is not JSON");
    assert_eq!(envelope["ok"], true, "seek fixture config rejected: {envelope}");
    sim
}

/// The seek protocol every harness must reproduce exactly: `seek_times`
/// in order on one handle.
pub fn run_protocol(seed: u64, seek_times: &[f32]) -> (u32, Vec<f32>) {
    let mut sim = load_sim(seed);
    for &time in seek_times {
        sim.seek(time);
    }
    (sim.particle_count(), sim.buffer_slice().to_vec())
}

/// Runs the protocol and checks it against `expected_file` in the fixtures
/// directory, or rewrites that file when `BRIGHTFX_REGENERATE` is set.
pub fn check_fixture(expected_file: &str, seed: u64, seek_times: &[f32]) {
    let (count, buffer) = run_protocol(seed, seek_times);
    let path = fixtures_dir().join(expected_file);

    if std::env::var("BRIGHTFX_REGENERATE").is_ok() {
        let json = serde_json::json!({
            "seed": seed,
            "seekTimes": seek_times,
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

    assert_eq!(expected["seed"].as_u64().unwrap(), seed, "fixture seed drifted");
    assert_eq!(floats(&expected["seekTimes"]), seek_times, "seek times drifted");
    assert_eq!(
        expected["particleFloats"].as_u64().unwrap() as u32,
        AbiSimulation::PARTICLE_FLOATS,
        "stride drifted"
    );
    assert_eq!(
        expected["tolerance"].as_f64().unwrap() as f32,
        LIBM_DRIFT_TOLERANCE,
        "tolerance drifted"
    );
    assert_eq!(expected["particleCount"].as_u64().unwrap() as u32, count);

    let expected_buffer = floats(&expected["buffer"]);
    assert_eq!(expected_buffer.len(), buffer.len(), "buffer length changed");
    for (index, (actual, wanted)) in buffer.iter().zip(&expected_buffer).enumerate() {
        assert!(
            (actual - wanted).abs() <= LIBM_DRIFT_TOLERANCE,
            "float {index} drifted: got {actual}, expected {wanted}"
        );
    }
}

fn floats(array: &serde_json::Value) -> Vec<f32> {
    array.as_array().unwrap().iter().map(|v| v.as_f64().unwrap() as f32).collect()
}
