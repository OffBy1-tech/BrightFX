//! Shared by the fixture tests. Cargo builds each file in `tests/` as its own
//! crate, so each one pulls this in with `mod common;`.

/// Tolerance for comparing simulation output produced by different builds
/// of the same code: `golden.rs` compares this machine's libm against the
/// one that recorded the fixture, and the cross-target fixtures compare
/// native libm against wasm's. The values come from ~120 chained ticks of
/// `sin`/`cos`/`powf`/`atan2`, which are not bit-identical across libms,
/// and a 1-ulp divergence amplified over that many integration steps can
/// straddle a rounding boundary. Same cause everywhere, so one value: the
/// fixture tests record it in their JSON and the harnesses read it back.
pub const LIBM_DRIFT_TOLERANCE: f32 = 2e-3;
