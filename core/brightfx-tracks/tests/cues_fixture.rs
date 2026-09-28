//! The cue generator's cross-target fixture: Rust records the generator's
//! exact output bytes for a fixed input; the Node harness runs the same
//! input through the wasm build and requires the identical string. Bytes,
//! not parsed values: the output is meant to be identical on every target
//! (see the serializer note at the top of `json.rs`), and a parse-and-
//! compare would forgive exactly the float-formatting drift this fixture
//! exists to catch. Run with `BRIGHTFX_REGENERATE=1` to rewrite the
//! expected file after an intentional change.

use brightfx_tracks::json::generate_cue_tracks_json;
use std::path::PathBuf;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures")
}

fn generate() -> String {
    let input = std::fs::read_to_string(fixtures_dir().join("tracks-cues.input.json")).unwrap();
    let mut out = generate_cue_tracks_json(&input);
    out.push('\n');
    out
}

#[test]
fn the_fixture_matches_the_recorded_expectation() {
    let actual = generate();
    let path = fixtures_dir().join("tracks-cues.expected.json");
    if std::env::var("BRIGHTFX_REGENERATE").is_ok() {
        std::fs::write(&path, &actual).unwrap();
        eprintln!("regenerated {}", path.display());
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .expect("expected fixture missing -- run with BRIGHTFX_REGENERATE=1");
    assert_eq!(actual, expected, "generator output drifted from the recorded bytes");
}

#[test]
fn the_fixture_is_not_vacuous() {
    let envelope: serde_json::Value = serde_json::from_str(generate().trim_end()).unwrap();
    assert_eq!(envelope["ok"], true, "{envelope}");
    let roles: Vec<&str> = envelope["jobs"].as_array().unwrap().iter().map(|j| j["role"].as_str().unwrap()).collect();
    assert_eq!(roles, ["entrance", "entrance", "lineup", "hero"]);
}
