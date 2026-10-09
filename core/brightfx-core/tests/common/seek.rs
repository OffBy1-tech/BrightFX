//! Scaffolding for `seek_fixture.rs` and `seek_forward_fixture.rs`.

use super::{assert_within_libm_drift, fixtures_dir, floats, load_or_regenerate, LIBM_DRIFT_TOLERANCE};
use brightfx_core::abi::AbiSimulation;

/// A handle loaded with the config file `config_file` from the fixtures
/// directory, ready to seek.
pub fn load_sim_for(config_file: &str, seed: u64) -> AbiSimulation {
    let config = std::fs::read_to_string(fixtures_dir().join(config_file))
        .unwrap_or_else(|_| panic!("{config_file} missing"));
    let mut sim = AbiSimulation::new(seed);
    let envelope: serde_json::Value =
        serde_json::from_str(&sim.set_config(&config)).expect("envelope is not JSON");
    assert_eq!(envelope["ok"], true, "{config_file} rejected: {envelope}");
    sim
}

/// A handle loaded with the shared seek config, ready to seek.
pub fn load_sim(seed: u64) -> AbiSimulation {
    load_sim_for("ffi-seek.config.json", seed)
}

/// The seek protocol every harness must reproduce exactly: `seek_times`
/// in order on one handle loaded with `config_file`.
pub fn run_protocol_for(config_file: &str, seed: u64, seek_times: &[f32]) -> (u32, Vec<f32>) {
    let mut sim = load_sim_for(config_file, seed);
    for &time in seek_times {
        sim.seek(time);
    }
    (sim.particle_count(), sim.buffer_slice().to_vec())
}

/// `run_protocol_for` on the shared seek config.
pub fn run_protocol(seed: u64, seek_times: &[f32]) -> (u32, Vec<f32>) {
    run_protocol_for("ffi-seek.config.json", seed, seek_times)
}

/// Runs the protocol on `config_file` and checks it against `expected_file`
/// in the fixtures directory, or rewrites that file when regenerating.
pub fn check_fixture_for(config_file: &str, expected_file: &str, seed: u64, seek_times: &[f32]) {
    let (count, buffer) = run_protocol_for(config_file, seed, seek_times);
    let record = || {
        serde_json::json!({
            "seed": seed,
            "seekTimes": seek_times,
            "particleFloats": AbiSimulation::PARTICLE_FLOATS,
            "tolerance": LIBM_DRIFT_TOLERANCE,
            "particleCount": count,
            "buffer": buffer,
        })
    };
    let Some(expected) = load_or_regenerate(&fixtures_dir().join(expected_file), record) else { return };

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
    assert_within_libm_drift(&buffer, &floats(&expected["buffer"]));
}

/// `check_fixture_for` on the shared seek config.
pub fn check_fixture(expected_file: &str, seed: u64, seek_times: &[f32]) {
    check_fixture_for("ffi-seek.config.json", expected_file, seed, seek_times)
}
