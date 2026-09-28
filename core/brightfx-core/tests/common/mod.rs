//! Shared by the fixture tests. Cargo builds each file in `tests/` as its own
//! crate, so each one pulls this in with `mod common;` and uses only part of
//! it. Every item here is used by some test crate but not all of them, so
//! each opts out of `dead_code` on its own -- a new helper has to do the
//! same deliberately rather than hide under a module-wide allow.

use brightfx_core::ParticleInstance;
use std::path::{Path, PathBuf};

/// Tolerance for comparing simulation output produced by different builds
/// of the same code: `golden.rs` compares this machine's libm against the
/// one that recorded the fixture, and the cross-target fixtures compare
/// native libm against wasm's. The values come from ~120 chained ticks of
/// `sin`/`cos`/`powf`/`atan2`, which are not bit-identical across libms,
/// and a 1-ulp divergence amplified over that many integration steps can
/// straddle a rounding boundary. Same cause everywhere, so one value: the
/// fixture tests record it in their JSON and the harnesses read it back.
#[allow(dead_code)]
pub const LIBM_DRIFT_TOLERANCE: f32 = 2e-3;

/// The two seek fixtures share a config, a protocol, and a file format;
/// only the seek times and the file they record to differ.
#[allow(dead_code)]
pub mod seek;

/// `core/fixtures`, where the cross-target fixtures live.
#[allow(dead_code)]
pub fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures")
}

/// Whether this run rewrites fixtures instead of verifying them. Only the
/// exact value `1` counts, so an inherited `BRIGHTFX_REGENERATE=0` or an
/// empty value cannot quietly turn a verify run into a re-baseline.
#[allow(dead_code)]
pub fn regenerating() -> bool {
    let value = match std::env::var("BRIGHTFX_REGENERATE") {
        Ok(value) if value == "1" => return true,
        Ok(value) => format!("{value:?}"),
        Err(std::env::VarError::NotUnicode(value)) => format!("{value:?} (not UTF-8)"),
        Err(std::env::VarError::NotPresent) => return false,
    };
    // Once per test binary: several fixtures may ask.
    static WARNED: std::sync::Once = std::sync::Once::new();
    WARNED.call_once(|| {
        eprintln!("BRIGHTFX_REGENERATE={value} is ignored and the fixtures are verified; only =1 regenerates")
    });
    false
}

/// The recorded expectation at `path`, or `None` after rewriting it from
/// `record` when `regenerating()` -- the caller then has nothing to verify.
#[allow(dead_code)]
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

/// Every float's bit pattern. "Bit-identical" compares these rather than
/// the floats: f32 `==` holds for -0.0 against +0.0, and the harnesses
/// compare bitwise, so the Rust side has to as well. Asserts every float is
/// finite first, since two identical NaNs have identical bits.
#[allow(dead_code)]
pub fn bits(floats: &[f32]) -> Vec<u32> {
    assert!(floats.iter().all(|f| f.is_finite()), "buffer contains NaN or inf");
    floats.iter().map(|f| f.to_bits()).collect()
}

/// `bits` of a `Simulation` buffer, read as the flat floats a host sees.
/// Not hand-flattened field by field, so a field added to
/// `ParticleInstance` is compared without this helper learning about it.
#[allow(dead_code)]
pub fn instance_bits(buffer: &[ParticleInstance]) -> Vec<u32> {
    let floats = std::mem::size_of_val(buffer) / std::mem::size_of::<f32>();
    // SAFETY: `ParticleInstance` is `#[repr(C)]` and made only of `f32`s
    // (its 32-byte stride is asserted at compile time in `simulation.rs`,
    // and `AbiSimulation::buffer_slice` hands hosts the same view), so the
    // slice is exactly `floats` initialized, aligned `f32`s.
    bits(unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<f32>(), floats) })
}

/// A JSON array of numbers as `f32`s.
#[allow(dead_code)]
pub fn floats(array: &serde_json::Value) -> Vec<f32> {
    array.as_array().unwrap().iter().map(|v| v.as_f64().unwrap() as f32).collect()
}

/// Every float of `actual` within `LIBM_DRIFT_TOLERANCE` of `expected`.
#[allow(dead_code)]
pub fn assert_within_libm_drift(actual: &[f32], expected: &[f32]) {
    assert_eq!(expected.len(), actual.len(), "buffer length changed");
    for (index, (got, wanted)) in actual.iter().zip(expected).enumerate() {
        assert!(
            (got - wanted).abs() <= LIBM_DRIFT_TOLERANCE,
            "float {index} drifted: got {got}, expected {wanted}"
        );
    }
}
