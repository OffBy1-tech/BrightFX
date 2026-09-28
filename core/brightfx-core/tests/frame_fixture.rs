//! Generates and verifies the cross-target golden frame.
//!
//! The Node, Swift, and C# harnesses replay this same protocol, render, and
//! compare against `ffi-frame.expected.rgba` with the tolerances recorded in
//! `ffi-frame.expected.json`. Run with `BRIGHTFX_REGENERATE=1` to rewrite
//! both after an intentional rendering change, and review the diff.

#![cfg(feature = "render")]

use brightfx_core::abi::AbiSimulation;
use std::path::PathBuf;

const SEED: u64 = 42;
const FRAMES: usize = 120;
const DT: f32 = 0.015625;
const BURST_FRAME: usize = 60;
const WIDTH: u32 = 200;
const HEIGHT: u32 = 120;
const SCALE: f32 = 1.0;
/// Native and wasm libm can differ by an ulp in `sin`/`cos`, which moves an
/// anti-aliased edge by a fraction of a pixel. Allow that, not more.
const CHANNEL_TOLERANCE: u8 = 4;
/// 0.5% of the frame.
const MAX_DIFFERING_PIXELS: usize = 120;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures")
}

/// The emitter's state on every frame, recorded in the fixture for the same
/// reason as in `ffi_fixture.rs`: the harnesses replay numbers, not a
/// formula. This one used to need `Math.fround` gymnastics in JavaScript to
/// match f32 evaluation order.
fn emitter_frames() -> Vec<[f32; 4]> {
    (0..FRAMES)
        .map(|frame| {
            let i = frame as f32;
            [20.0 + i * 1.2, 90.0 - i * 0.5, 1.2, -0.5]
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

/// The frame protocol every harness must reproduce exactly.
fn run_protocol() -> Vec<u8> {
    let config = std::fs::read_to_string(fixtures_dir().join("ffi-frame.config.json"))
        .expect("frame fixture config missing");

    let mut sim = AbiSimulation::new(SEED);
    let envelope: serde_json::Value =
        serde_json::from_str(&sim.set_config(&config)).expect("envelope is not JSON");
    assert_eq!(envelope["ok"], true, "frame config rejected: {envelope}");
    let envelope: serde_json::Value =
        serde_json::from_str(&sim.set_viewport(WIDTH, HEIGHT, SCALE)).unwrap();
    assert_eq!(envelope["ok"], true, "viewport rejected: {envelope}");

    for (frame, [x, y, vx, vy]) in emitter_frames().into_iter().enumerate() {
        sim.set_emitter(x, y, vx, vy, true);
        if frame == BURST_FRAME {
            sim.trigger_burst();
        }
        sim.advance(DT);
    }
    sim.render();
    sim.frame_slice().to_vec()
}

fn nonzero_pixels(frame: &[u8]) -> usize {
    frame.chunks(4).filter(|px| px[3] != 0).count()
}

#[test]
fn the_frame_matches_the_recorded_expectation() {
    let frame = run_protocol();
    let json_path = fixtures_dir().join("ffi-frame.expected.json");
    let rgba_path = fixtures_dir().join("ffi-frame.expected.rgba");

    if std::env::var("BRIGHTFX_REGENERATE").as_deref() == Ok("1") {
        let json = serde_json::json!({
            "seed": SEED,
            "frames": FRAMES,
            "dt": DT,
            "burstFrame": BURST_FRAME,
            "emitterFrames": emitter_frames_json(&emitter_frames()),
            "width": WIDTH,
            "height": HEIGHT,
            "scale": SCALE,
            "channelTolerance": CHANNEL_TOLERANCE,
            "maxDifferingPixels": MAX_DIFFERING_PIXELS,
            "nonzeroPixels": nonzero_pixels(&frame),
            "rgbaFile": "ffi-frame.expected.rgba",
        });
        std::fs::write(&json_path, serde_json::to_string_pretty(&json).unwrap()).unwrap();
        std::fs::write(&rgba_path, &frame).unwrap();
        eprintln!("regenerated {} and {}", json_path.display(), rgba_path.display());
        return;
    }

    let expected: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&json_path)
            .expect("expected frame missing -- run with BRIGHTFX_REGENERATE=1 to create it"),
    )
    .unwrap();
    assert_eq!(expected["seed"].as_u64().unwrap(), SEED, "fixture seed drifted");
    assert_eq!(expected["frames"].as_u64().unwrap() as usize, FRAMES, "frame count drifted");
    assert_eq!(expected["dt"].as_f64().unwrap() as f32, DT, "dt drifted");
    assert_eq!(expected["burstFrame"].as_u64().unwrap() as usize, BURST_FRAME, "burst frame drifted");
    assert_eq!(
        emitter_frames_from_json(&expected["emitterFrames"]),
        emitter_frames(),
        "emitter frames drifted"
    );
    assert_eq!(expected["width"].as_u64().unwrap() as u32, WIDTH, "width drifted");
    assert_eq!(expected["height"].as_u64().unwrap() as u32, HEIGHT, "height drifted");
    assert_eq!(expected["scale"].as_f64().unwrap() as f32, SCALE, "scale drifted");
    assert_eq!(expected["channelTolerance"].as_u64().unwrap() as u8, CHANNEL_TOLERANCE);
    assert_eq!(expected["maxDifferingPixels"].as_u64().unwrap() as usize, MAX_DIFFERING_PIXELS);
    // An exact count is platform-fragile for the same libm reason the
    // per-pixel tolerance above exists.
    let actual_nonzero = nonzero_pixels(&frame);
    let expected_nonzero = expected["nonzeroPixels"].as_u64().unwrap() as usize;
    let nonzero_diff = actual_nonzero.abs_diff(expected_nonzero);
    assert!(
        nonzero_diff <= MAX_DIFFERING_PIXELS,
        "painted pixel count drifted: got {actual_nonzero}, expected {expected_nonzero}"
    );

    let expected_frame = std::fs::read(&rgba_path).expect("expected rgba missing");
    assert_eq!(expected_frame.len(), frame.len(), "frame length changed");

    let differing = frame
        .chunks(4)
        .zip(expected_frame.chunks(4))
        .filter(|(a, b)| a.iter().zip(b.iter()).any(|(x, y)| x.abs_diff(*y) > CHANNEL_TOLERANCE))
        .count();
    assert!(
        differing <= MAX_DIFFERING_PIXELS,
        "{differing} pixels drifted beyond {CHANNEL_TOLERANCE} per channel (allowed {MAX_DIFFERING_PIXELS})"
    );
}

#[test]
fn the_protocol_actually_paints_something() {
    let frame = run_protocol();
    let painted = nonzero_pixels(&frame);
    assert!(painted > 500, "frame protocol is vacuous -- only {painted} pixels painted");
}
