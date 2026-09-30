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
pub fn fit_track_json(config_json: &str, from_w: f32, from_h: f32, to_w: f32, to_h: f32) -> String {
    let config = match parse_config(config_json) {
        Ok(config) => config,
        Err(message) => return err_envelope(&message),
    };
    match fit_track(config, (from_w, from_h), (to_w, to_h)) {
        Ok(fitted) => serde_json::to_string(&ConfigEnvelope { ok: true, config: &fitted }).expect("envelope serializes"),
        Err(message) => err_envelope(&message),
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
        config["schemaVersion"] = serde_json::json!(4);
        config["somethingNew"] = serde_json::json!(true);
        let v = parse(&fit_track_json(&config.to_string(), 1920.0, 1080.0, 1080.0, 1920.0));
        assert_eq!(v["ok"], false);
        assert!(v["error"].as_str().unwrap().contains("unsupported schemaVersion 4"), "{v}");
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
