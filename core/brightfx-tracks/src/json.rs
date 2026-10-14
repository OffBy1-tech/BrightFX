//! String-in, string-out entry points for the wasm boundary. Envelopes
//! follow the core's `set_config` convention so a host parses one shape.
//!
//! Success envelopes are written with `serde_json::to_string` over derived
//! `Serialize` structs, not `serde_json::json!`. `json!` builds a `Value`,
//! which widens every `f32` field to `f64` and prints 17 significant
//! digits; serde_json's default float parser is not correctly rounded on
//! such inputs, so any fixture that round-trips through `Value` can drift
//! by an ULP between platforms. The string serializer prints the shortest
//! f32 form instead, which every parser reads back exactly.

use crate::{fit_track, generate_cue_tracks, CueInput, EffectJob};
use brightfx_core::abi::{err_envelope, parse_config};
use brightfx_core::ParticleFxConfig;
use serde::Serialize;

#[derive(Serialize)]
struct ConfigEnvelope<'a> {
    ok: bool,
    config: &'a ParticleFxConfig,
}

#[derive(Serialize)]
struct JobsEnvelope<'a> {
    ok: bool,
    jobs: &'a [EffectJob],
}

/// `{"ok":true,"config":{...}}` with the fitted config, or an error envelope.
///
/// The config goes through the core's `parse_config`, so a config
/// `set_config` would refuse (wrong `schemaVersion`, bad body) is refused
/// here with the same message rather than fitted and handed back.
///
/// A number that does not fit in an f32 is refused too, naming the field.
/// serde parses an out-of-range literal like `1e39` to infinity without
/// error and serializes it as `null`, so it would otherwise come back as
/// `null` behind `ok:true`, which `set_config` then rejects.
pub fn fit_track_json(config_json: &str, from_w: f32, from_h: f32, to_w: f32, to_h: f32) -> String {
    let config = match parse_config(config_json) {
        Ok(config) => config,
        Err(message) => return err_envelope(&message),
    };
    let fitted = match fit_track(config, (from_w, from_h), (to_w, to_h)) {
        Ok(fitted) => fitted,
        Err(message) => return err_envelope(&message),
    };
    let input: serde_json::Value = serde_json::from_str(config_json).expect("parse_config accepted it");
    let output = serde_json::to_value(&fitted).expect("config serializes");
    if let Some(path) = first_overflow(&input, &output, String::new()) {
        return err_envelope(&format!("{path} is out of range: it does not fit in an f32"));
    }
    serde_json::to_string(&ConfigEnvelope { ok: true, config: &fitted }).expect("envelope serializes")
}

/// The first place, as a dotted path, where `input` holds a number and
/// `output` a `null`: a non-finite f32, which JSON can only write as `null`.
/// `output` is walked; a field with no counterpart in `input` is skipped.
fn first_overflow(input: &serde_json::Value, output: &serde_json::Value, path: String) -> Option<String> {
    use serde_json::Value;
    match (input, output) {
        (Value::Number(_), Value::Null) => Some(path),
        (Value::Object(input), Value::Object(output)) => output.iter().find_map(|(key, out)| {
            let path = if path.is_empty() { key.clone() } else { format!("{path}.{key}") };
            first_overflow(input.get(key)?, out, path)
        }),
        (Value::Array(input), Value::Array(output)) => output
            .iter()
            .zip(input)
            .enumerate()
            .find_map(|(i, (out, inp))| first_overflow(inp, out, format!("{path}[{i}]"))),
        _ => None,
    }
}

/// `{"ok":true,"jobs":[...]}` with the generated jobs, or an error envelope:
/// `invalid cue input: ...` when the document does not parse, `cue input
/// rejected: ...` when it parses but the generator refuses it (see
/// `generate_cue_tracks`), so a host can tell the two apart.
pub fn generate_cue_tracks_json(input_json: &str) -> String {
    let input: CueInput = match serde_json::from_str(input_json) {
        Ok(input) => input,
        Err(error) => return err_envelope(&format!("invalid cue input: {error}")),
    };
    match generate_cue_tracks(&input) {
        Ok(jobs) => serde_json::to_string(&JobsEnvelope { ok: true, jobs: &jobs }).expect("envelope serializes"),
        Err(message) => err_envelope(&format!("cue input rejected: {message}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bad_json_returns_an_error_envelope_not_a_panic() {
        let v: serde_json::Value = serde_json::from_str(&generate_cue_tracks_json("{ nope")).unwrap();
        assert_eq!(v["ok"], false);
        assert!(v["error"].as_str().unwrap().starts_with("invalid cue input"));
        let v: serde_json::Value = serde_json::from_str(&fit_track_json("[]", 1.0, 1.0, 1.0, 1.0)).unwrap();
        assert_eq!(v["ok"], false);
    }

    #[test]
    fn a_number_too_large_for_f32_is_refused_by_name() {
        let mut config: serde_json::Value = serde_json::from_str(include_str!("../../../presets/confetti.brightfx.json")).unwrap();
        config["emitterTrack"]["preroll"] = serde_json::from_str("1e39").unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&fit_track_json(&config.to_string(), 1920.0, 1080.0, 1080.0, 1920.0)).unwrap();
        assert_eq!(v["ok"], false, "{v}");
        assert_eq!(v["error"], "emitterTrack.preroll is out of range: it does not fit in an f32");

        config["emitterTrack"]["preroll"] = 5.0.into();
        config["emitterTrack"]["keyframes"][3]["time"] = serde_json::from_str("-1e39").unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&fit_track_json(&config.to_string(), 1920.0, 1080.0, 1080.0, 1920.0)).unwrap();
        assert_eq!(v["error"], "emitterTrack.keyframes[3].time is out of range: it does not fit in an f32");
    }

    #[test]
    fn every_preset_fits_without_a_false_overflow() {
        for (name, config) in crate::presets::library() {
            let json = serde_json::to_string(&config).unwrap();
            let v: serde_json::Value = serde_json::from_str(&fit_track_json(&json, 1920.0, 1080.0, 1080.0, 1920.0)).unwrap();
            assert_eq!(v["ok"], true, "{name}: {v}");
        }
    }

    #[test]
    fn fit_round_trips_through_json() {
        let config = serde_json::to_string(&ParticleFxConfig::default()).unwrap();
        let v: serde_json::Value = serde_json::from_str(&fit_track_json(&config, 1920.0, 1080.0, 1080.0, 1920.0)).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["config"]["schemaVersion"], brightfx_core::SCHEMA_VERSION);
    }

    fn parse(json: &str) -> serde_json::Value {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn error_envelopes_are_the_core_envelope_byte_for_byte() {
        let ours = fit_track_json("{ nope", 1.0, 1.0, 1.0, 1.0);
        let core = brightfx_core::abi::err_envelope("invalid JSON: key must be a string at line 1 column 3");
        assert_eq!(ours, core);
    }

    #[test]
    fn fit_track_json_applies_the_core_schema_gate() {
        let mut config = serde_json::to_value(ParticleFxConfig::default()).unwrap();
        config["schemaVersion"] = serde_json::json!(6);
        config["somethingNew"] = serde_json::json!(true);
        let v = parse(&fit_track_json(&config.to_string(), 1920.0, 1080.0, 1080.0, 1920.0));
        assert_eq!(v["ok"], false);
        assert!(v["error"].as_str().unwrap().contains("unsupported schemaVersion 6"), "{v}");
    }

    #[test]
    fn fit_track_json_rejects_bad_dimensions_with_an_envelope() {
        let config = serde_json::to_string(&ParticleFxConfig::default()).unwrap();
        let v = parse(&fit_track_json(&config, 1920.0, 1080.0, f32::NAN, 1080.0));
        assert_eq!(v["ok"], false);
        assert!(v["error"].as_str().unwrap().contains("to.width"), "{v}");
    }

    #[test]
    fn out_of_range_float_literals_are_rejected_not_written_back_as_null() {
        let input = r#"{"cues":[{"t":[1e40,2],"who":"a"}],"layout":{"cards":{"a":{"x":1,"y":2}},"lineup":{"x":0,"y":0}},"duration":153}"#;
        let out = generate_cue_tracks_json(input);
        let v = parse(&out);
        assert_eq!(v["ok"], false, "{out}");
        assert!(!out.contains("null"), "{out}");
        assert!(v["error"].as_str().unwrap().starts_with("cue input rejected"), "{out}");
    }

    #[test]
    fn parse_errors_and_validation_errors_carry_different_prefixes() {
        // A host (and the Node harness) can tell a malformed document from
        // a well-formed one the generator refused.
        let malformed = parse(&generate_cue_tracks_json("{ nope"));
        assert!(malformed["error"].as_str().unwrap().starts_with("invalid cue input: "), "{malformed}");
        let input = r#"{"cues":[{"t":[40,35],"who":"a"}],"layout":{"cards":{"a":{"x":1,"y":2}},"lineup":{"x":0,"y":0}},"duration":153}"#;
        let rejected = parse(&generate_cue_tracks_json(input));
        assert!(rejected["error"].as_str().unwrap().starts_with("cue input rejected: cue 0"), "{rejected}");
    }
}
