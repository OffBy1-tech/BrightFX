//! Generates and verifies the cross-target seek fixture.
//!
//! The smoke fixture drives the emitter by hand through `set_emitter` and
//! `advance`. This one drives it from the config's `emitterTrack` through
//! `seek`, the one entry point the smoke fixture never crosses the boundary
//! with, and the only cross-target coverage of baked timeline playback. Run
//! with `BRIGHTFX_REGENERATE=1` to rewrite `ffi-seek.expected.json` after an
//! intentional simulation change.

mod common;

use common::bits;
use common::seek::{check_fixture, load_sim, run_protocol};

const SEED: u64 = 42;
/// The second seek lands earlier than the first, so this fixture covers
/// the reset-and-replay path: the recorded state must be that of
/// `seek(1.25)` alone, which `seeking_again_starts_over` pins in-process.
/// `ffi-seek-forward` is the one that covers the incremental path, with
/// strictly increasing times. Both must stay that way -- between them they
/// are the only cross-target coverage of the two paths through `seek`.
const SEEK_TIMES: [f32; 2] = [1.75, 1.25];

#[test]
fn the_fixture_matches_the_recorded_expectation() {
    check_fixture("ffi-seek.expected.json", SEED, &SEEK_TIMES);
}

#[test]
fn the_protocol_actually_produces_particles() {
    let (count, buffer) = run_protocol(SEED, &SEEK_TIMES);
    assert!(count > 0, "seek fixture protocol is vacuous -- no particles");
    assert!(buffer.iter().all(|v| v.is_finite()), "fixture contains NaN or inf");
}

#[test]
fn seeking_again_starts_over() {
    // Same binary, same inputs: the second seek must be bit-identical to a
    // first seek to the same time, or the fixture would be recording the
    // history of calls rather than a point on the track.
    let (count, buffer) = run_protocol(SEED, &SEEK_TIMES);
    let mut fresh = load_sim(SEED);
    fresh.seek(SEEK_TIMES[SEEK_TIMES.len() - 1]);
    assert_eq!(fresh.particle_count(), count);
    assert_eq!(bits(fresh.buffer_slice()), bits(&buffer), "not bit-identical");
}
