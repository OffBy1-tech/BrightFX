//! Helpers every crate's fixture tests share. They live in one crate, a
//! dev-dependency of each, because a copy per test tree can drift: the rule
//! that only `BRIGHTFX_REGENERATE=1` regenerates holds across the workspace
//! only while every suite asks the same function.

use std::path::PathBuf;

/// `core/fixtures`, where the cross-target fixtures live. Resolved from
/// this crate's manifest, which sits beside the others under `core/`.
pub fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures")
}

/// Whether this run rewrites fixtures instead of verifying them. Only the
/// exact value `1` counts, so an inherited `BRIGHTFX_REGENERATE=0` or an
/// empty value cannot quietly turn a verify run into a re-baseline.
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
