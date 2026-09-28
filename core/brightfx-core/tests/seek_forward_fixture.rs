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

use common::bits;
use common::seek::{check_fixture, load_sim, run_protocol};

const SEED: u64 = 42;
/// Strictly increasing and all on the 1/60 s grid, so every seek is a
/// forward step and the last one is a grid point a fresh seek lands on.
const SEEK_TIMES: [f32; 4] = [0.5, 1.0, 1.25, 1.75];

#[test]
fn the_fixture_matches_the_recorded_expectation() {
    check_fixture("ffi-seek-forward.expected.json", SEED, &SEEK_TIMES);
}

#[test]
fn the_protocol_actually_produces_particles() {
    let (count, buffer) = run_protocol(SEED, &SEEK_TIMES);
    assert!(count > 0, "forward-seek fixture is vacuous -- no particles");
    assert!(buffer.iter().all(|v| v.is_finite()), "fixture contains NaN or inf");
}

#[test]
fn forward_seeking_is_bit_identical_to_a_fresh_seek() {
    let (count, buffer) = run_protocol(SEED, &SEEK_TIMES);
    let mut fresh = load_sim(SEED);
    fresh.seek(SEEK_TIMES[SEEK_TIMES.len() - 1]);
    assert_eq!(fresh.particle_count(), count);
    assert_eq!(bits(fresh.buffer_slice()), bits(&buffer), "not bit-identical");
}
