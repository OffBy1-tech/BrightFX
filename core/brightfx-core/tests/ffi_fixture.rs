//! Generates and verifies the cross-target FFI fixture.
//!
//! The Node, Swift, and C# harnesses replay the exact same driving protocol
//! and compare against `ffi-smoke.expected.json`. This test is the source of
//! that file: run with `BRIGHTFX_REGENERATE=1` to rewrite it after an
//! intentional simulation change.

mod common;

use brightfx_core::abi::AbiSimulation;
use common::{assert_within_libm_drift, fixtures_dir, floats, load_or_regenerate, LIBM_DRIFT_TOLERANCE};

const SEED: u64 = 42;
const FRAMES: usize = 120;
/// 1/64 — exactly representable, so no host's float parsing can differ by an
/// ulp and turn a rounding artifact into a false marshaling failure.
const DT: f32 = 0.015625;
const BURST_FRAME: usize = 60;

/// The emitter's state on every frame: `x, y, vx, vy`. Recorded in the
/// fixture so the harnesses replay these numbers rather than re-derive them.
/// The formula is arithmetic that has nothing to do with the boundary, and
/// a slip in any one of three languages would surface as float drift instead
/// of a protocol mismatch.
fn emitter_frames() -> Vec<[f32; 4]> {
    (0..FRAMES)
        .map(|frame| {
            let i = frame as f32;
            [i * 1.5, i * -0.5, 1.5, -0.5]
        })
        .collect()
}

fn emitter_frames_json(frames: &[[f32; 4]]) -> serde_json::Value {
    frames
        .iter()
        .map(|f| serde_json::json!({ "x": f[0], "y": f[1], "vx": f[2], "vy": f[3] }))
        .collect()
}

fn emitter_frames_from_json(value: &serde_json::Value) -> Vec<[f32; 4]> {
    value
        .as_array()
        .expect("emitterFrames is not an array")
        .iter()
        .map(|f| ["x", "y", "vx", "vy"].map(|k| f[k].as_f64().unwrap() as f32))
        .collect()
}

/// The driving protocol every harness must reproduce exactly.
fn run_protocol() -> (u32, Vec<f32>) {
    let config = std::fs::read_to_string(fixtures_dir().join("ffi-smoke.config.json"))
        .expect("fixture config missing");

    let mut sim = AbiSimulation::new(SEED);
    let envelope: serde_json::Value =
        serde_json::from_str(&sim.set_config(&config)).expect("envelope is not JSON");
    assert_eq!(envelope["ok"], true, "fixture config rejected: {envelope}");

    for (frame, [x, y, vx, vy]) in emitter_frames().into_iter().enumerate() {
        sim.set_emitter(x, y, vx, vy, true);
        if frame == BURST_FRAME {
            sim.trigger_burst();
        }
        sim.advance(DT);
    }

    let count = sim.particle_count();
    (count, sim.buffer_slice().to_vec())
}

#[test]
fn the_fixture_matches_the_recorded_expectation() {
    let (count, buffer) = run_protocol();
    let record = || {
        serde_json::json!({
            "seed": SEED,
            "frames": FRAMES,
            "dt": DT,
            "burstFrame": BURST_FRAME,
            "emitterFrames": emitter_frames_json(&emitter_frames()),
            "particleFloats": AbiSimulation::PARTICLE_FLOATS,
            "tolerance": LIBM_DRIFT_TOLERANCE,
            "particleCount": count,
            "buffer": buffer,
        })
    };
    let Some(expected) = load_or_regenerate(&fixtures_dir().join("ffi-smoke.expected.json"), record) else { return };

    // Assert the protocol constants too, not just the output. If someone
    // changes `dt` or the frame count, this reports *that* in one line
    // instead of 3592 confusing float diffs -- and the three harnesses
    // replaying this protocol read these same fields.
    assert_eq!(expected["seed"].as_u64().unwrap(), SEED, "fixture seed drifted");
    assert_eq!(expected["frames"].as_u64().unwrap() as usize, FRAMES, "frame count drifted");
    assert_eq!(expected["dt"].as_f64().unwrap() as f32, DT, "dt drifted");
    assert_eq!(
        expected["burstFrame"].as_u64().unwrap() as usize,
        BURST_FRAME,
        "burst frame drifted"
    );
    assert_eq!(
        emitter_frames_from_json(&expected["emitterFrames"]),
        emitter_frames(),
        "emitter frames drifted"
    );
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

#[test]
fn the_protocol_actually_produces_particles() {
    let (count, buffer) = run_protocol();
    assert!(count > 0, "fixture protocol is vacuous -- no particles");
    assert!(buffer.iter().all(|v| v.is_finite()), "fixture contains NaN or inf");
}
