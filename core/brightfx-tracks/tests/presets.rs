//! Guards the preset library four ways: the committed JSON matches the
//! generator (staleness), every preset loads unclamped and produces
//! particles (validity), every preset survives the fit to the 9:16
//! delivery frame, and each has a golden frame (look).
//!
//! A golden frame is the authored 1920×1080 frame drawn at the renderer's
//! minimum scale (0.25) and box-filtered down to 240×135, so it is a
//! thumbnail of the real effect rather than a differently-shaped one:
//! `fit_track` rescales an emitter track's positions but deliberately
//! leaves sizes and speeds in pixels, so a preset rendered into a small
//! fitted frame is not that preset's look. `flight-arc` makes the point --
//! its particles cross 2000 px whatever the frame is, so a fitted 320×180
//! golden is an empty frame. The fit is covered by its own test instead.
//!
//! Run with `BRIGHTFX_REGENERATE=1` to rewrite `core/fixtures/presets/*.rgba`
//! after an intentional change, then look at the frames before committing
//! (`magick -size 240x135 -depth 8 rgba:file.rgba out.png`).

use brightfx_core::abi::AbiSimulation;
use brightfx_core::MAX_EMITTER_TRACK_DURATION;
use brightfx_tracks::fit_track;
use brightfx_tracks::presets::{library, render_json, FRAME_H, FRAME_W};
use std::path::PathBuf;

/// The renderer's `MIN_SCALE`. Anything smaller is clamped back up to it.
const RENDER_SCALE: f32 = 0.25;
/// Box-filter factor from the rendered frame down to the stored golden.
const DOWNSAMPLE: usize = 2;
const RENDER_W: u32 = (FRAME_W * RENDER_SCALE) as u32;
const RENDER_H: u32 = (FRAME_H * RENDER_SCALE) as u32;
const GOLDEN_W: usize = RENDER_W as usize / DOWNSAMPLE;
const GOLDEN_H: usize = RENDER_H as usize / DOWNSAMPLE;
const GOLDEN_TIME: f32 = 3.0;
const CHANNEL_TOLERANCE: u8 = 4;
/// 0.5% of the frame.
const MAX_DIFFERING_PIXELS: usize = GOLDEN_W * GOLDEN_H / 200;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn presets_dir() -> PathBuf {
    repo_root().join("presets")
}

fn golden_dir() -> PathBuf {
    repo_root().join("core/fixtures/presets")
}

#[test]
fn committed_json_matches_the_generator() {
    for (name, config) in library() {
        let path = presets_dir().join(format!("{name}.brightfx.json"));
        let on_disk = std::fs::read_to_string(&path).unwrap_or_else(|_| {
            panic!(
                "{} missing -- run: cargo run -p brightfx-tracks --example gen_presets -- ../presets",
                path.display()
            )
        });
        assert_eq!(on_disk, render_json(&config), "{name} is stale -- rerun gen_presets");
    }
}

/// The staleness check above only proves every `library()` name is on disk
/// and current -- it says nothing about a stale file left behind for a
/// preset the library no longer names. Catch that direction too.
#[test]
fn presets_dir_has_no_files_the_library_no_longer_names() {
    let names: std::collections::BTreeSet<&str> = library().iter().map(|(name, _)| *name).collect();
    let on_disk: std::collections::BTreeSet<String> = std::fs::read_dir(presets_dir())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter_map(|file_name| file_name.strip_suffix(".brightfx.json").map(str::to_string))
        .collect();
    let stale: Vec<&String> = on_disk.iter().filter(|stem| !names.contains(stem.as_str())).collect();
    assert!(
        stale.is_empty(),
        "presets/ has files the library no longer names: {stale:?} -- rerun gen_presets, which removes them"
    );
}

/// Same check for the golden frames: a preset removed from `library()`
/// should take its `.rgba` fixture with it, not leave it to rot.
#[test]
fn golden_dir_has_no_files_the_library_no_longer_names() {
    let names: std::collections::BTreeSet<&str> = library().iter().map(|(name, _)| *name).collect();
    let on_disk: std::collections::BTreeSet<String> = std::fs::read_dir(golden_dir())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter_map(|file_name| file_name.strip_suffix(".rgba").map(str::to_string))
        .collect();
    let stale: Vec<&String> = on_disk.iter().filter(|stem| !names.contains(stem.as_str())).collect();
    assert!(
        stale.is_empty(),
        "core/fixtures/presets/ has golden frames the library no longer names: {stale:?}"
    );
}

#[test]
fn every_preset_loads_unclamped_and_emits() {
    for (name, _) in library() {
        let json = std::fs::read_to_string(presets_dir().join(format!("{name}.brightfx.json"))).unwrap();
        let mut sim = AbiSimulation::new(7);
        let envelope: serde_json::Value = serde_json::from_str(&sim.set_config(&json)).unwrap();
        assert_eq!(envelope["ok"], true, "{name}: {envelope}");
        assert_eq!(envelope["clamped"].as_array().unwrap().len(), 0, "{name} needs clamping: {envelope}");
        sim.seek(GOLDEN_TIME);
        assert!(sim.particle_count() > 0, "{name} produces no particles at t={GOLDEN_TIME}");
        let config: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            config["emitterTrack"]["duration"], MAX_EMITTER_TRACK_DURATION,
            "{name}: track must run the full cap"
        );
        // One trigger, at 0, and nothing after it: a second trigger (a
        // stop, a burst) would let the track decide when the preset shows,
        // which is the composition's job.
        let triggers = config["emitterTrack"]["triggers"].as_array().unwrap();
        assert_eq!(triggers.len(), 1, "{name}: expected exactly one trigger, got {triggers:?}");
        assert_eq!(triggers[0]["kind"], "startContinuous", "{name}: must start continuous");
        assert_eq!(triggers[0]["time"], 0.0, "{name}: must start at 0");
    }
}

#[test]
fn every_preset_survives_the_fit_to_the_delivery_frame() {
    // What the composition does for a 9:16 render. The fit must not push
    // any preset out of bounds, and the fitted track must still emit.
    for (name, config) in library() {
        let fitted = fit_track(config, (FRAME_W, FRAME_H), (FRAME_H, FRAME_W))
            .unwrap_or_else(|e| panic!("{name}: fit_track rejected the delivery frame: {e}"));
        let mut sim = AbiSimulation::new(7);
        let envelope: serde_json::Value =
            serde_json::from_str(&sim.set_config(&serde_json::to_string(&fitted).unwrap())).unwrap();
        assert_eq!(envelope["ok"], true, "{name}: {envelope}");
        assert_eq!(envelope["clamped"].as_array().unwrap().len(), 0, "{name} fitted needs clamping: {envelope}");
        sim.seek(GOLDEN_TIME);
        assert!(sim.particle_count() > 0, "{name} fitted produces no particles at t={GOLDEN_TIME}");
    }
}

/// The authored frame at `RENDER_SCALE`, box-filtered down to the golden
/// size. Averaging keeps a sub-pixel wobble in one particle from tripping
/// the per-pixel tolerance while a real change still moves whole blocks.
fn render_golden(config: &brightfx_core::ParticleFxConfig) -> Vec<u8> {
    let mut sim = AbiSimulation::new(7);
    let envelope: serde_json::Value =
        serde_json::from_str(&sim.set_config(&serde_json::to_string(config).unwrap())).unwrap();
    assert_eq!(envelope["ok"], true);
    let envelope: serde_json::Value = serde_json::from_str(&sim.set_viewport(RENDER_W, RENDER_H, RENDER_SCALE)).unwrap();
    assert_eq!(envelope["ok"], true);
    assert_eq!(envelope["clamped"].as_array().unwrap().len(), 0, "the golden viewport must not be clamped");
    sim.seek(GOLDEN_TIME);
    sim.render();
    let frame = sim.frame_slice();

    let mut out = Vec::with_capacity(GOLDEN_W * GOLDEN_H * 4);
    let stride = RENDER_W as usize * 4;
    for y in 0..GOLDEN_H {
        for x in 0..GOLDEN_W {
            for channel in 0..4 {
                let mut sum = 0u32;
                for dy in 0..DOWNSAMPLE {
                    for dx in 0..DOWNSAMPLE {
                        sum += frame[(y * DOWNSAMPLE + dy) * stride + (x * DOWNSAMPLE + dx) * 4 + channel] as u32;
                    }
                }
                out.push((sum / (DOWNSAMPLE * DOWNSAMPLE) as u32) as u8);
            }
        }
    }
    out
}

#[test]
fn golden_frames_match() {
    std::fs::create_dir_all(golden_dir()).unwrap();
    let regenerate = std::env::var("BRIGHTFX_REGENERATE").is_ok();
    for (name, config) in library() {
        let frame = render_golden(&config);
        let path = golden_dir().join(format!("{name}.rgba"));
        let painted = frame.chunks(4).filter(|px| px[3] != 0).count();
        assert!(painted > 50, "{name}: golden frame is vacuous ({painted} painted pixels)");
        if regenerate {
            std::fs::write(&path, &frame).unwrap();
            eprintln!("regenerated {}", path.display());
            continue;
        }
        let expected = std::fs::read(&path)
            .unwrap_or_else(|_| panic!("{} missing -- run with BRIGHTFX_REGENERATE=1", path.display()));
        assert_eq!(expected.len(), frame.len(), "{name}: frame size changed");
        let differing = frame
            .chunks(4)
            .zip(expected.chunks(4))
            .filter(|(a, b)| a.iter().zip(b.iter()).any(|(x, y)| x.abs_diff(*y) > CHANNEL_TOLERANCE))
            .count();
        assert!(differing <= MAX_DIFFERING_PIXELS, "{name}: {differing} pixels drifted");
    }
}
