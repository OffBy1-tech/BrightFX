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
use brightfx_core::abi::{err_envelope, parse_config_value, parse_json};
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
/// The config goes through the core's parse steps, so a config
/// `set_config` would refuse (wrong `schemaVersion`, bad body) is refused
/// here with the same message rather than fitted and handed back.
///
/// A number too large for an f32 (`1e39`, or `-Number.MAX_VALUE` from a JS
/// host) reads as an infinity, which JSON can only write back as `null`.
/// `set_config` accepts one anywhere, and outside the keyframes it means
/// what the nearest finite f32 means: a preroll or duration clamps to its
/// cap, a trigger fires at the start or never, any other field clamps to
/// its range. So there it is written back as that f32. A keyframe is
/// refused by name instead: its numbers are what the fit scales, and an
/// infinite one has no place on the path. A finite keyframe that the
/// scale itself pushes past f32 is refused as the fit's overflow.
pub fn fit_track_json(config_json: &str, from_w: f32, from_h: f32, to_w: f32, to_h: f32) -> String {
    let mut value = match parse_json(config_json) {
        Ok(value) => value,
        Err(message) => return err_envelope(&message),
    };
    if let Err(path) = saturate_overflows(&mut value, String::new()) {
        return err_envelope(&format!("{path} is out of range: it does not fit in an f32"));
    }
    let config = match parse_config_value(value) {
        Ok(config) => config,
        Err(message) => return err_envelope(&message),
    };
    let fitted = match fit_track(config, (from_w, from_h), (to_w, to_h)) {
        Ok(fitted) => fitted,
        Err(message) => return err_envelope(&message),
    };
    if let Some(path) = first_non_finite(&fitted) {
        return err_envelope(&format!(
            "{path} overflows an f32 when fitted from {from_w}x{from_h} to {to_w}x{to_h}"
        ));
    }
    serde_json::to_string(&ConfigEnvelope { ok: true, config: &fitted }).expect("envelope serializes")
}

/// Replaces each number past the f32 range with the f32 limit of its sign,
/// except under `emitterTrack.keyframes`, where it is an error naming the
/// field's dotted path.
fn saturate_overflows(value: &mut serde_json::Value, path: String) -> Result<(), String> {
    use serde_json::Value;
    match value {
        Value::Number(number) => {
            let Some(wide) = number.as_f64() else { return Ok(()) };
            if (wide as f32).is_finite() {
                return Ok(());
            }
            if path.starts_with("emitterTrack.keyframes[") {
                return Err(path);
            }
            *value = (f32::MAX.copysign(wide as f32) as f64).into();
            Ok(())
        }
        Value::Object(map) => map.iter_mut().try_for_each(|(key, child)| {
            let path = if path.is_empty() { key.clone() } else { format!("{path}.{key}") };
            saturate_overflows(child, path)
        }),
        Value::Array(items) => {
            items.iter_mut().enumerate().try_for_each(|(i, child)| saturate_overflows(child, format!("{path}[{i}]")))
        }
        _ => Ok(()),
    }
}

/// The first number the fit scaled out of the f32 range, as a dotted path:
/// the keyframes it multiplies and the cull margin it may raise.
fn first_non_finite(config: &ParticleFxConfig) -> Option<String> {
    if config.cull_margin.is_some_and(|margin| !margin.is_finite()) {
        return Some("cullMargin".into());
    }
    let track = config.emitter_track.as_ref()?;
    track.keyframes.iter().enumerate().find_map(|(i, k)| {
        let fields = [("x", Some(k.x)), ("y", Some(k.y)), ("vx", k.vx), ("vy", k.vy)];
        let (name, _) = fields.into_iter().find(|(_, v)| v.is_some_and(|v| !v.is_finite()))?;
        Some(format!("emitterTrack.keyframes[{i}].{name}"))
    })
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

    fn confetti() -> serde_json::Value {
        serde_json::from_str(include_str!("../../../presets/confetti.brightfx.json")).unwrap()
    }

    fn fit(config: &serde_json::Value) -> serde_json::Value {
        serde_json::from_str(&fit_track_json(&config.to_string(), 1920.0, 1080.0, 1080.0, 1920.0)).unwrap()
    }

    #[test]
    fn a_number_past_f32_outside_the_keyframes_is_fitted_as_the_core_reads_it() {
        // Each of these `set_config` accepts: the pre-roll clamps to 60 s
        // and the trigger fires at the reset. Written back as the f32 limit,
        // they mean the same.
        let mut config = confetti();
        config["emitterTrack"]["preroll"] = serde_json::from_str("1e39").unwrap();
        config["emitterTrack"]["triggers"][0]["time"] = (-f64::MAX).into();
        let v = fit(&config);
        assert_eq!(v["ok"], true, "{v}");
        assert_eq!(v["config"]["emitterTrack"]["preroll"].as_f64().unwrap() as f32, f32::MAX);
        assert_eq!(v["config"]["emitterTrack"]["triggers"][0]["time"].as_f64().unwrap() as f32, -f32::MAX);

        let fitted: ParticleFxConfig = serde_json::from_value(v["config"].clone()).unwrap();
        let mut sim = brightfx_core::Simulation::new(fitted, 1);
        sim.seek(0.0);
        assert!(sim.particle_count() > 0, "the trigger at -f32::MAX must still start emission");
    }

    #[test]
    fn a_keyframe_past_f32_is_refused_by_name() {
        let mut config = confetti();
        config["emitterTrack"]["keyframes"][3]["time"] = serde_json::from_str("-1e39").unwrap();
        let v = fit(&config);
        assert_eq!(v["ok"], false, "{v}");
        assert_eq!(v["error"], "emitterTrack.keyframes[3].time is out of range: it does not fit in an f32");
    }

    #[test]
    fn a_keyframe_the_fit_scales_past_f32_blames_the_fit() {
        // 3e38 fits in an f32; doubled, it does not.
        let mut config = confetti();
        config["emitterTrack"]["keyframes"][2]["x"] = 3e38.into();
        let v: serde_json::Value =
            serde_json::from_str(&fit_track_json(&config.to_string(), 1920.0, 1080.0, 3840.0, 1080.0)).unwrap();
        assert_eq!(v["ok"], false, "{v}");
        assert_eq!(v["error"], "emitterTrack.keyframes[2].x overflows an f32 when fitted from 1920x1080 to 3840x1080");
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
