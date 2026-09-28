//! Forward seeking steps from where the last seek left off. These tests pin
//! the property that makes that safe: a forward seek on one handle is
//! bit-identical to a from-zero seek on a fresh handle, and anything that
//! is not a seek forces the next seek to replay.

mod common;

use brightfx_core::schema::{EmitterKeyframe, EmitterTrigger, TriggerKind};
use brightfx_core::{ParticleFxConfig, Simulation};
use common::{fixtures_dir, instance_bits};

const SEED: u64 = 42;

fn track_config() -> ParticleFxConfig {
    let path = fixtures_dir().join("ffi-seek.config.json");
    serde_json::from_str(&std::fs::read_to_string(path).expect("seek fixture config missing")).unwrap()
}

fn fresh_at(time: f32) -> Simulation {
    let mut sim = Simulation::new(track_config(), SEED);
    sim.seek(time);
    sim
}

fn assert_same(a: &Simulation, b: &Simulation, label: &str) {
    assert_eq!(a.particle_count(), b.particle_count(), "particle count differs: {label}");
    // Bit patterns, not `PartialEq`: -0.0 == +0.0 would pass as equal.
    assert_eq!(instance_bits(a.buffer()), instance_bits(b.buffer()), "buffer differs: {label}");
}

#[test]
fn seeking_forward_matches_seeking_from_zero() {
    let mut forward = Simulation::new(track_config(), SEED);
    for time in [0.0, 0.25, 0.5, 1.0, 1.5, 1.75, 2.0] {
        forward.seek(time);
        assert_same(&forward, &fresh_at(time), &format!("at {time}"));
    }
    assert!(forward.particle_count() > 0, "test is vacuous");
}

#[test]
fn seeking_backward_replays_from_zero() {
    let mut sim = Simulation::new(track_config(), SEED);
    sim.seek(1.75);
    sim.seek(1.25);
    assert_same(&sim, &fresh_at(1.25), "after 1.75 then 1.25");
}

#[test]
fn an_off_grid_time_lands_on_the_grid_point_at_or_above_it() {
    // 60.4 steps runs *through* step 61, so nothing authored at or
    // before it is still pending when the host renders that time.
    let mut sim = Simulation::new(track_config(), SEED);
    sim.seek(1.0 + 0.4 / 60.0);
    assert_same(&sim, &fresh_at(61.0 / 60.0), "1.0 + 0.4 steps");
}

#[test]
fn a_thirty_fps_frame_late_in_a_long_track_still_runs_its_own_window() {
    // A 300 s track with a burst authored on grid point 15378 — frame 7689
    // at 30 fps. The f32 quotient sits far enough below the grid point
    // that flooring the quotient alone drops a step and the burst never
    // fires. (`grid_step`'s unit tests sweep this; here it is end to end,
    // through a config a host could actually load.)
    let mut config = track_config();
    {
        let track = config.emitter_track.as_mut().expect("seek fixture has a track");
        track.duration = 300.0;
        track.keyframes = vec![
            EmitterKeyframe { time: 0.0, x: 100.0, y: 100.0, vx: Some(0.0), vy: Some(0.0) },
            EmitterKeyframe { time: 300.0, x: 100.0, y: 100.0, vx: Some(0.0), vy: Some(0.0) },
        ];
        track.triggers = vec![EmitterTrigger { time: 15378.0 / 60.0, kind: TriggerKind::Burst }];
    }

    let mut sim = Simulation::new(config, SEED);
    sim.seek(7689.0 / 30.0);

    assert!(sim.particle_count() > 0, "the burst at 15378/60 s should have fired");
}

#[test]
fn set_emitter_invalidates_the_baked_position() {
    // A same-step seek applies zero steps, so without invalidation the
    // host's emitter position and active flag would survive into a seek
    // that is supposed to re-sample both from the track. The stale state
    // is invisible in the buffer until something steps the simulation,
    // so this steps it -- the reviewer's repro was 143 particles at
    // x~504 instead of 133 at x~81.
    let mut sim = Simulation::new(track_config(), SEED);
    sim.seek(1.0);
    sim.set_emitter(500.0, 500.0, 0.0, 0.0, true);
    sim.seek(1.0);

    let mut fresh = fresh_at(1.0);
    assert_same(&sim, &fresh, "after set_emitter");

    sim.advance(1.0 / 60.0);
    fresh.advance(1.0 / 60.0);
    assert!(fresh.particle_count() > 0, "test is vacuous");
    assert_same(&sim, &fresh, "one step after set_emitter between seeks");
}

#[test]
fn set_config_invalidates_the_baked_position() {
    let mut sim = Simulation::new(track_config(), SEED);
    sim.seek(1.0);
    let mut heavier = track_config();
    heavier.gravity_y = 3.0;
    sim.set_config(heavier.clone());
    sim.seek(1.5);
    let mut fresh = Simulation::new(heavier, SEED);
    fresh.seek(1.5);
    assert_same(&sim, &fresh, "after set_config");
}

#[test]
fn advance_invalidates_the_baked_position() {
    let mut sim = Simulation::new(track_config(), SEED);
    sim.seek(1.0);
    sim.advance(1.0 / 60.0);
    sim.seek(1.5);
    assert_same(&sim, &fresh_at(1.5), "after advance");
}

#[test]
fn trigger_burst_invalidates_the_baked_position() {
    let mut sim = Simulation::new(track_config(), SEED);
    sim.seek(1.0);
    sim.trigger_burst();
    sim.seek(1.5);
    assert_same(&sim, &fresh_at(1.5), "after trigger_burst");
}

#[test]
fn a_config_without_a_track_still_ignores_seek() {
    let mut config = track_config();
    config.emitter_track = None;
    let mut sim = Simulation::new(config, SEED);
    sim.seek(1.0);
    assert_eq!(sim.particle_count(), 0);
}
