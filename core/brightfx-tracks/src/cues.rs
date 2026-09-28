//! Lyric cues in, one baked emitter track per effect job out.
//!
//! The cue table is a lyric array a composition builds verbatim from a
//! song's `.srt`. The layout is what the composition knows
//! and this crate does not: where each character's card sits, where the
//! lineup stands, and each character's accent color. Every job has exactly
//! one position, so its track is a single keyframe plus triggers.

use std::collections::BTreeMap;

use brightfx_core::schema::{
    EmitterKeyframe, EmitterTrack, EmitterTrigger, TriggerKind, MAX_EMITTER_TRACK_DURATION,
};
use serde::{Deserialize, Serialize};

/// Two lineup cues closer than this (seconds) merge into one window.
const MERGE_GAP: f32 = 0.05;

/// One lyric line. The composition's `LYRICS` rows carry more (`text`,
/// `mood`); serde ignores what is not declared here, and the generator
/// reads only these two.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cue {
    pub t: [f32; 2],
    pub who: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Layout {
    /// Anchor per card slug. A cue whose `who` has no entry here produces
    /// no entrance burst. A slug may not be one of `Role::RESERVED_ACCENT_KEYS`.
    #[serde(default)]
    pub cards: BTreeMap<String, Point>,
    /// Anchor for lineup windows and hero bursts.
    pub lineup: Point,
    /// Accent color per card slug, plus the reserved keys `"lineup"` and
    /// `"hero"` for those jobs (`Role::RESERVED_ACCENT_KEYS`).
    #[serde(default)]
    pub accent: BTreeMap<String, String>,
    /// `who` tokens that mean the whole lineup besides `"lineup"` itself,
    /// which always does (crew tokens such as `"puppies"`).
    #[serde(default)]
    pub lineup_tokens: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CueInput {
    pub cues: Vec<Cue>,
    pub layout: Layout,
    /// Track duration for every job, in seconds: the song length.
    pub duration: f32,
    /// Explicit hero beats (seconds), e.g. the guitar-solo hit.
    #[serde(default)]
    pub hero_times: Vec<f32>,
}

/// What a job is for. Serializes as the lowercase name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Entrance,
    Lineup,
    Hero,
}

impl Role {
    /// The `layout.accent` keys that color the lineup and hero jobs. Card
    /// slugs live in the same map, so these are off limits as slugs.
    pub const RESERVED_ACCENT_KEYS: [&'static str; 2] = ["lineup", "hero"];
}

/// The `who` that always means the whole lineup, whatever `lineup_tokens` says.
const LINEUP_WHO: &str = "lineup";

impl Layout {
    fn is_lineup(&self, who: &str) -> bool {
        who == LINEUP_WHO || self.lineup_tokens.iter().any(|token| token == who)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectJob {
    pub role: Role,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub who: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    pub track: EmitterTrack,
}

fn static_track(at: Point, duration: f32, triggers: Vec<EmitterTrigger>) -> EmitterTrack {
    EmitterTrack {
        duration,
        keyframes: vec![EmitterKeyframe { time: 0.0, x: at.x, y: at.y, vx: Some(0.0), vy: Some(0.0) }],
        triggers,
    }
}

fn trigger(time: f32, kind: TriggerKind) -> EmitterTrigger {
    EmitterTrigger { time, kind }
}

fn finite_point(name: &str, p: Point) -> Result<(), String> {
    if p.x.is_finite() && p.y.is_finite() {
        Ok(())
    } else {
        Err(format!("{name} must be finite, got ({}, {})", p.x, p.y))
    }
}

/// Rejects input the generator would otherwise bake into a track that
/// misbehaves silently, and returns the last time a trigger can fire.
///
/// Every number must be finite: serde parses an out-of-range literal like
/// `1e40` to infinity without error, and serializes non-finite floats as
/// `null`, which the core then refuses. A trigger past
/// `min(duration, MAX_EMITTER_TRACK_DURATION)` is refused too: `seek`
/// never reaches it, so it would be baked with `ok:true` and never fire.
/// A cue's end may run past the track (its Stop simply never fires), but
/// it may not precede its start: `seek` replays triggers time-sorted, so
/// an inverted window would Stop before it Starts and emit to the end.
fn validate(input: &CueInput) -> Result<f32, String> {
    let duration = input.duration;
    if !duration.is_finite() || duration <= 0.0 {
        return Err(format!("duration must be a positive finite number of seconds, got {duration}"));
    }
    let reach = duration.min(MAX_EMITTER_TRACK_DURATION);
    let past_reach = |what: &str, time: f32| {
        let ceiling = if reach < duration {
            format!(" (the simulation plays at most {MAX_EMITTER_TRACK_DURATION} s of a track)")
        } else {
            String::new()
        };
        format!("{what} at {time} s is past the last reachable time, {reach} s{ceiling}")
    };

    let layout = &input.layout;
    finite_point("layout.lineup", layout.lineup)?;
    for (slug, point) in &layout.cards {
        if Role::RESERVED_ACCENT_KEYS.contains(&slug.as_str()) {
            return Err(format!(
                "layout.cards may not use the reserved slug {slug:?}: layout.accent.{slug} colors the {slug} job"
            ));
        }
        finite_point(&format!("layout.cards.{slug}"), *point)?;
    }

    for (i, cue) in input.cues.iter().enumerate() {
        let [start, end] = cue.t;
        let what = format!("cue {i} ({:?})", cue.who);
        if !start.is_finite() || !end.is_finite() {
            return Err(format!("{what} must have finite times, got [{start}, {end}]"));
        }
        if start < 0.0 {
            return Err(format!("{what} starts before the track, at {start} s"));
        }
        if end < start {
            return Err(format!("{what} ends at {end} s, before it starts at {start} s"));
        }
        if start > reach {
            return Err(past_reach(&what, start));
        }
    }

    for (i, &time) in input.hero_times.iter().enumerate() {
        let what = format!("heroTimes[{i}]");
        if !time.is_finite() || time < 0.0 {
            return Err(format!("{what} must be a non-negative finite time, got {time}"));
        }
        if time > reach {
            return Err(past_reach(&what, time));
        }
    }

    Ok(reach)
}

/// Entrance and lineup classification are independent: a `who` present in
/// both `layout.cards` and `layout.lineup_tokens` feeds both jobs, so a crew
/// token that also has a card bursts at its card entrance and opens a
/// lineup window. `"lineup"` itself is always a lineup token.
///
/// Cues may arrive in any order; lineup windows are built by time. Errors
/// name the offending cue, hero beat, or layout field (see `validate`).
pub fn generate_cue_tracks(input: &CueInput) -> Result<Vec<EffectJob>, String> {
    validate(input)?;
    let layout = &input.layout;
    let mut jobs: Vec<EffectJob> = Vec::new();

    // Entrances: one job per character, in order of first appearance, a
    // burst at each of that character's cue starts.
    for cue in &input.cues {
        let Some(&at) = layout.cards.get(&cue.who) else { continue };
        let burst = trigger(cue.t[0], TriggerKind::Burst);
        match jobs.iter_mut().find(|job| job.who.as_deref() == Some(cue.who.as_str())) {
            Some(job) => job.track.triggers.push(burst),
            None => jobs.push(EffectJob {
                role: Role::Entrance,
                who: Some(cue.who.clone()),
                color: layout.accent.get(&cue.who).cloned(),
                track: static_track(at, input.duration, vec![burst]),
            }),
        }
    }

    // Lineup: continuous emission for each window, cues sorted by start
    // (validate guarantees finite times) and adjacent ones merged.
    let mut spans: Vec<[f32; 2]> = input
        .cues
        .iter()
        .filter(|cue| layout.is_lineup(&cue.who))
        .map(|cue| cue.t)
        .collect();
    spans.sort_by(|a, b| a[0].total_cmp(&b[0]));
    let mut windows: Vec<[f32; 2]> = Vec::new();
    for span in spans {
        match windows.last_mut() {
            Some(last) if span[0] <= last[1] + MERGE_GAP => last[1] = last[1].max(span[1]),
            _ => windows.push(span),
        }
    }
    if !windows.is_empty() {
        let mut triggers = Vec::with_capacity(windows.len() * 2);
        for w in &windows {
            triggers.push(trigger(w[0], TriggerKind::StartContinuous));
            triggers.push(trigger(w[1], TriggerKind::StopContinuous));
        }
        jobs.push(EffectJob {
            role: Role::Lineup,
            who: None,
            color: layout.accent.get("lineup").cloned(),
            track: static_track(layout.lineup, input.duration, triggers),
        });
    }

    // Hero: a burst at each explicit time, at the lineup anchor.
    if !input.hero_times.is_empty() {
        let triggers = input.hero_times.iter().map(|&t| trigger(t, TriggerKind::Burst)).collect();
        jobs.push(EffectJob {
            role: Role::Hero,
            who: None,
            color: layout.accent.get("hero").cloned(),
            track: static_track(layout.lineup, input.duration, triggers),
        });
    }

    Ok(jobs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cue(t0: f32, t1: f32, who: &str) -> Cue {
        Cue { t: [t0, t1], who: who.into() }
    }

    fn input() -> CueInput {
        let mut cards = BTreeMap::new();
        cards.insert("rex".to_string(), Point { x: 960.0, y: 486.0 });
        cards.insert("bop".to_string(), Point { x: 960.0, y: 486.0 });
        let mut accent = BTreeMap::new();
        accent.insert("rex".to_string(), "#37A24A".to_string());
        accent.insert("lineup".to_string(), "#E8632A".to_string());
        CueInput {
            cues: vec![
                cue(24.3, 27.0, "rex"),
                cue(27.0, 30.0, "rex"),
                cue(32.4, 34.8, "bop"),
                cue(35.1, 37.8, "lineup"),
                cue(37.8, 40.5, "lineup"),
                cue(45.8, 48.5, "puppies"),
                cue(60.0, 62.0, "narrator"),
            ],
            layout: Layout {
                cards,
                lineup: Point { x: 960.0, y: 507.0 },
                accent,
                lineup_tokens: vec!["lineup".into(), "puppies".into()],
            },
            duration: 153.3,
            hero_times: vec![67.0],
        }
    }

    fn jobs(input: &CueInput) -> Vec<EffectJob> {
        generate_cue_tracks(input).unwrap()
    }

    fn lineup_times(jobs: &[EffectJob]) -> Vec<(f32, TriggerKind)> {
        let lineup = jobs.iter().find(|j| j.role == Role::Lineup).unwrap();
        lineup.track.triggers.iter().map(|t| (t.time, t.kind)).collect()
    }

    #[test]
    fn entrances_group_by_character_in_first_appearance_order() {
        let jobs = jobs(&input());
        assert_eq!(jobs[0].role, Role::Entrance);
        assert_eq!(jobs[0].who.as_deref(), Some("rex"));
        assert_eq!(jobs[0].color.as_deref(), Some("#37A24A"));
        assert_eq!(jobs[0].track.triggers.len(), 2);
        assert_eq!(jobs[0].track.triggers[1].time, 27.0);
        assert_eq!(jobs[1].who.as_deref(), Some("bop"));
        assert_eq!(jobs[1].color, None);
    }

    #[test]
    fn adjacent_lineup_cues_merge_and_separated_ones_do_not() {
        let jobs = jobs(&input());
        assert_eq!(
            lineup_times(&jobs),
            vec![
                (35.1, TriggerKind::StartContinuous),
                (40.5, TriggerKind::StopContinuous),
                (45.8, TriggerKind::StartContinuous),
                (48.5, TriggerKind::StopContinuous),
            ]
        );
        let lineup = jobs.iter().find(|j| j.role == Role::Lineup).unwrap();
        assert_eq!(lineup.track.keyframes[0].x, 960.0);
        assert_eq!(lineup.color.as_deref(), Some("#E8632A"));
    }

    #[test]
    fn lineup_cues_are_windowed_by_time_not_by_table_order() {
        // An .srt with a late-inserted line, or a cue table assembled per
        // character, arrives out of order. Merging against only the last
        // window would swallow [10,12] and [30,50] into [35,40].
        let mut i = input();
        i.cues.retain(|c| c.who != "lineup" && c.who != "puppies");
        i.cues.push(cue(35.0, 40.0, "lineup"));
        i.cues.push(cue(10.0, 12.0, "lineup"));
        i.cues.push(cue(30.0, 50.0, "puppies"));
        assert_eq!(
            lineup_times(&jobs(&i)),
            vec![
                (10.0, TriggerKind::StartContinuous),
                (12.0, TriggerKind::StopContinuous),
                (30.0, TriggerKind::StartContinuous),
                (50.0, TriggerKind::StopContinuous),
            ]
        );
    }

    #[test]
    fn hero_times_become_bursts_at_the_lineup_anchor() {
        let jobs = jobs(&input());
        let hero = jobs.last().unwrap();
        assert_eq!(hero.role, Role::Hero);
        assert_eq!(hero.track.triggers, vec![trigger(67.0, TriggerKind::Burst)]);
        assert_eq!(hero.track.keyframes[0].y, 507.0);
    }

    #[test]
    fn unknown_who_produces_nothing_and_every_track_has_one_keyframe() {
        let jobs = jobs(&input());
        assert_eq!(jobs.len(), 4, "rex, bop, lineup, hero -- not narrator");
        assert!(jobs.iter().all(|j| j.track.keyframes.len() == 1 && j.track.duration == 153.3));
    }

    #[test]
    fn no_lineup_cues_and_no_hero_times_means_no_such_jobs() {
        let mut i = input();
        i.cues.retain(|c| c.who == "rex");
        i.hero_times.clear();
        let jobs = jobs(&i);
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].role, Role::Entrance);
    }

    #[test]
    fn a_who_that_is_both_a_card_and_a_lineup_token_feeds_both_jobs() {
        // The two classifications are independent by design: a crew token
        // can also have a card, and then its cues burst at the card and
        // open a lineup window.
        let mut i = input();
        i.layout.cards.insert("puppies".to_string(), Point { x: 300.0, y: 300.0 });
        let jobs = jobs(&i);
        let puppies = jobs.iter().find(|j| j.who.as_deref() == Some("puppies")).unwrap();
        assert_eq!(puppies.role, Role::Entrance);
        assert_eq!(puppies.track.triggers, vec![trigger(45.8, TriggerKind::Burst)]);
        assert!(lineup_times(&jobs).contains(&(45.8, TriggerKind::StartContinuous)));
    }

    #[test]
    fn who_lineup_always_means_the_lineup_even_without_lineup_tokens() {
        // The spec: "lineup" or a crew token. A spec-shaped input that
        // omits lineupTokens must not silently produce no lineup job.
        let mut i = input();
        i.layout.lineup_tokens.clear();
        let jobs = jobs(&i);
        assert_eq!(
            lineup_times(&jobs),
            vec![(35.1, TriggerKind::StartContinuous), (40.5, TriggerKind::StopContinuous)],
            "puppies is no longer a token, lineup still is"
        );
    }

    #[test]
    fn roles_serialize_as_the_documented_lowercase_strings() {
        assert_eq!(serde_json::to_string(&Role::Entrance).unwrap(), "\"entrance\"");
        assert_eq!(serde_json::to_string(&Role::Lineup).unwrap(), "\"lineup\"");
        assert_eq!(serde_json::to_string(&Role::Hero).unwrap(), "\"hero\"");
        assert_eq!(serde_json::from_str::<Role>("\"hero\"").unwrap(), Role::Hero);
    }

    #[test]
    fn a_cue_that_ends_before_it_starts_is_rejected() {
        // Simulation::seek replays triggers time-sorted, so an inverted
        // lineup window would fire its Stop before its Start and emit
        // until the end of the track.
        let mut i = input();
        i.cues[3] = cue(40.0, 35.0, "lineup");
        let err = generate_cue_tracks(&i).unwrap_err();
        assert!(err.contains("cue 3") && err.contains("lineup") && err.contains("40") && err.contains("35"), "got: {err}");
    }

    #[test]
    fn a_trigger_past_the_reachable_end_of_the_track_is_rejected() {
        // seek caps at min(duration, MAX_EMITTER_TRACK_DURATION), so a
        // trigger beyond that would be baked with ok:true and never fire.
        let mut i = input();
        i.hero_times = vec![200.0];
        let err = generate_cue_tracks(&i).unwrap_err();
        assert!(err.contains("heroTimes[0]") && err.contains("200") && err.contains("153.3"), "got: {err}");

        let mut i = input();
        i.cues[0] = cue(160.0, 161.0, "rex");
        let err = generate_cue_tracks(&i).unwrap_err();
        assert!(err.contains("cue 0") && err.contains("160"), "got: {err}");

        let mut i = input();
        i.duration = 720.0;
        i.cues[0] = cue(650.0, 651.0, "rex");
        let err = generate_cue_tracks(&i).unwrap_err();
        assert!(err.contains("650") && err.contains("600"), "must name the ceiling: {err}");
    }

    #[test]
    fn a_cue_may_end_after_the_track_but_not_start_before_zero() {
        // A last lyric line running past the audio is harmless: its Stop
        // simply never fires. A negative start is a bad table.
        let mut i = input();
        i.cues[4] = cue(37.8, 154.0, "lineup");
        assert!(generate_cue_tracks(&i).is_ok());
        i.cues[0] = cue(-1.0, 2.0, "rex");
        assert!(generate_cue_tracks(&i).unwrap_err().contains("cue 0"));
    }

    #[test]
    fn non_finite_numbers_anywhere_are_rejected() {
        // serde_json parses an out-of-range literal like 1e40 to infinity
        // without error and would write it back out as null.
        let mut i = input();
        i.duration = f32::NAN;
        assert!(generate_cue_tracks(&i).unwrap_err().contains("duration"));
        let mut i = input();
        i.duration = 0.0;
        assert!(generate_cue_tracks(&i).unwrap_err().contains("duration"));
        let mut i = input();
        i.hero_times = vec![f32::INFINITY];
        assert!(generate_cue_tracks(&i).unwrap_err().contains("heroTimes[0]"));
        let mut i = input();
        i.cues[6] = cue(f32::INFINITY, 62.0, "narrator");
        assert!(generate_cue_tracks(&i).unwrap_err().contains("cue 6"), "unused cues are validated too");
        let mut i = input();
        i.layout.lineup = Point { x: f32::NAN, y: 0.0 };
        assert!(generate_cue_tracks(&i).unwrap_err().contains("layout.lineup"));
        let mut i = input();
        i.layout.cards.insert("bop".into(), Point { x: 0.0, y: f32::INFINITY });
        assert!(generate_cue_tracks(&i).unwrap_err().contains("layout.cards.bop"));
    }

    #[test]
    fn a_card_slug_may_not_use_a_reserved_accent_key() {
        // accent.lineup and accent.hero color those jobs; a character
        // named "hero" would silently share the hero job's color.
        for key in ["hero", "lineup"] {
            let mut i = input();
            i.layout.cards.insert(key.to_string(), Point { x: 0.0, y: 0.0 });
            let err = generate_cue_tracks(&i).unwrap_err();
            assert!(err.contains(key) && err.contains("reserved"), "got: {err}");
        }
    }
}
