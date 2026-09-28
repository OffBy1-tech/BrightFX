//! Emitter-track generators. Everything that turns timing data (a lyric cue
//! table, a beat list, a sweep) into an `EmitterTrack` lives here, once,
//! so the Remotion composer (through wasm) and Content Compiler (natively)
//! run the same code. Pure Rust over the core's schema types; no platform
//! code, no rendering.

pub mod cues;
pub mod fit;
pub mod json;
pub mod presets;
pub mod sweep;

pub use cues::{generate_cue_tracks, Cue, CueInput, EffectJob, Layout, Point, Role};
pub use fit::fit_track;
pub use sweep::sweep_track;
