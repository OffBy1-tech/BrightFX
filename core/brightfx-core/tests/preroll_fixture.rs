//! Generates and verifies the cross-target pre-roll fixture.
//!
//! `ffi-seek` and `ffi-seek-forward` start their track at t=0. This one
//! starts 1 s earlier (`emitterTrack.preroll`), with its StartContinuous
//! and a burst at negative times, and records the buffer at `seek(0)`:
//! the pool the pre-roll built. Every harness reproduces it, then seeks
//! the same handle on to 0.5 s and requires that to be bit-identical to a
//! fresh seek, which is the forward path leaving the pre-roll. Run with
//! `BRIGHTFX_REGENERATE=1` to rewrite `ffi-preroll.expected.json` after an
//! intentional simulation change.

mod common;

use common::bits;
use common::seek::{check_fixture_for, load_sim_for, run_protocol_for};

const CONFIG: &str = "ffi-preroll.config.json";
const EXPECTED: &str = "ffi-preroll.expected.json";
const SEED: u64 = 42;
/// Only 0: the point of the fixture is the state the pre-roll leaves at
/// t=0. The forward step out of the pre-roll is checked in-process below
/// and in each harness, against a fresh seek rather than a recording.
const SEEK_TIMES: [f32; 1] = [0.0];
const FORWARD_TIME: f32 = 0.5;

#[test]
fn the_fixture_matches_the_recorded_expectation() {
    check_fixture_for(CONFIG, EXPECTED, SEED, &SEEK_TIMES);
}

#[test]
fn seeking_to_zero_is_not_vacuous() {
    let (count, buffer) = run_protocol_for(CONFIG, SEED, &SEEK_TIMES);
    assert!(count > 0, "pre-roll fixture is vacuous -- seek(0) left no particles");
    assert!(buffer.iter().all(|v| v.is_finite()), "fixture contains NaN or inf");
}

#[test]
fn seeking_on_out_of_the_preroll_is_bit_identical_to_a_fresh_seek() {
    let mut forward = load_sim_for(CONFIG, SEED);
    for &time in &SEEK_TIMES {
        forward.seek(time);
    }
    forward.seek(FORWARD_TIME);
    let mut fresh = load_sim_for(CONFIG, SEED);
    fresh.seek(FORWARD_TIME);
    assert_eq!(fresh.particle_count(), forward.particle_count());
    assert_eq!(bits(fresh.buffer_slice()), bits(forward.buffer_slice()), "not bit-identical");
}
