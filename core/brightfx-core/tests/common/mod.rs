//! Shared by the fixture tests. Cargo builds each file in `tests/` as its own
//! crate, so each one pulls this in with `mod common;` -- and each crate uses
//! only part of it, hence the allow.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// Tolerance for comparing simulation output produced by different builds
/// of the same code: `golden.rs` compares this machine's libm against the
/// one that recorded the fixture, and the cross-target fixtures compare
/// native libm against wasm's. The values come from ~120 chained ticks of
/// `sin`/`cos`/`powf`/`atan2`, which are not bit-identical across libms,
/// and a 1-ulp divergence amplified over that many integration steps can
/// straddle a rounding boundary. Same cause everywhere, so one value: the
/// fixture tests record it in their JSON and the harnesses read it back.
pub const LIBM_DRIFT_TOLERANCE: f32 = 2e-3;

/// The two seek fixtures share a config, a protocol, and a file format;
/// only the seek times and the file they record to differ.
pub mod seek;

/// `core/fixtures`, where the cross-target fixtures live.
pub fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures")
}

/// Whether this run rewrites fixtures instead of verifying them. Only the
/// exact value `1` counts, so an inherited `BRIGHTFX_REGENERATE=0` or an
/// empty value cannot quietly turn a verify run into a re-baseline.
pub fn regenerating() -> bool {
    std::env::var("BRIGHTFX_REGENERATE").as_deref() == Ok("1")
}

/// The recorded expectation at `path`, or `None` after rewriting it from
/// `record` when `regenerating()` -- the caller then has nothing to verify.
pub fn load_or_regenerate(path: &Path, record: impl FnOnce() -> serde_json::Value) -> Option<serde_json::Value> {
    if regenerating() {
        std::fs::write(path, serde_json::to_string_pretty(&record()).unwrap()).unwrap();
        eprintln!("regenerated {}", path.display());
        return None;
    }
    let text = std::fs::read_to_string(path)
        .expect("expected fixture missing -- run with BRIGHTFX_REGENERATE=1 to create it");
    Some(serde_json::from_str(&text).unwrap())
}

/// A JSON array of numbers as `f32`s.
pub fn floats(array: &serde_json::Value) -> Vec<f32> {
    array.as_array().unwrap().iter().map(|v| v.as_f64().unwrap() as f32).collect()
}

/// Every float of `actual` within `LIBM_DRIFT_TOLERANCE` of `expected`.
pub fn assert_within_libm_drift(actual: &[f32], expected: &[f32]) {
    assert_eq!(expected.len(), actual.len(), "buffer length changed");
    for (index, (got, wanted)) in actual.iter().zip(expected).enumerate() {
        assert!(
            (got - wanted).abs() <= LIBM_DRIFT_TOLERANCE,
            "float {index} drifted: got {got}, expected {wanted}"
        );
    }
}
