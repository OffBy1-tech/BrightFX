//! The FFI boundary's logic, in plain Rust with no FFI dependencies.
//!
//! `brightfx-ffi` and `brightfx-wasm` are thin type-translation layers over
//! this module. Keeping the behavior here means the two wrappers cannot
//! drift apart, and means it is testable with an ordinary `cargo test`.

use std::marker::PhantomData;
use std::panic::{catch_unwind, AssertUnwindSafe};

/// Runs `f`, converting a panic into `fallback` so it never unwinds across
/// the FFI boundary.
///
/// This depends on unwinding being enabled: under `panic = "abort"` a panic
/// terminates the process and this becomes a no-op. No workspace profile may
/// set that.
///
/// `guard` stops the unwind; it does NOT roll back partial mutation. A panic
/// part-way through a `&mut self` method leaves that state half-updated —
/// memory-safe, but logically torn. `AssertUnwindSafe` is therefore an
/// obligation on the caller, not a proof: any caller that mutates must mark
/// itself poisoned when `guard` returns the fallback, so the torn state is
/// never read back. `AbiSimulation::guard_mut` is the wrapper that does this;
/// prefer it over calling `guard` directly on a mutating path.
pub(crate) fn guard<T>(fallback: T, f: impl FnOnce() -> T) -> T {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or(fallback)
}

// The doc comment above is a hard requirement, so make it a build error
// rather than a convention: `catch_unwind` silently becomes a no-op under
// `panic = "abort"`, and every panic-containment guarantee in this module
// dies with it. `cfg(panic = ...)` is evaluated per profile, so this fires
// on a release build even though the test profile always unwinds.
//
// Scoped away from wasm32: that target's shipped std has no linkable
// `panic_unwind` runtime, so `-C panic=unwind` fails to *link* there (not
// just a profile default to override) -- `panic = "abort"` is the only
// buildable strategy on wasm32-unknown-unknown with the stable toolchain,
// independent of any `[profile]` in this workspace. That is a platform
// limitation the ABI guarantee cannot cover, not a policy violation this
// lint exists to catch; the native targets (`brightfx-ffi`'s C ABI) are
// where the "no workspace profile may set panic = abort" rule is both
// meaningful and enforceable.
#[cfg(all(panic = "abort", not(target_arch = "wasm32")))]
compile_error!(
    "brightfx-core requires unwinding: catch_unwind is a no-op under \
     panic = \"abort\", which voids the FFI boundary's panic containment"
);

/// `{"ok":true,"clamped":["glowRadius"],"warnings":[]}`
///
/// Built with `serde_json` rather than string concatenation so field names
/// and messages are escaped correctly.
pub(crate) fn ok_envelope(clamped: &[&'static str]) -> String {
    ok_envelope_with_warnings(clamped, &[])
}

/// As `ok_envelope`, with advisory `warnings`: things the config will do
/// that the host may not want, reported without rejecting or changing it.
pub(crate) fn ok_envelope_with_warnings(clamped: &[&'static str], warnings: &[String]) -> String {
    serde_json::json!({ "ok": true, "clamped": clamped, "warnings": warnings }).to_string()
}

/// Parses a config the way `set_config` does: JSON syntax, then the
/// `schemaVersion` gate, then the body. The error is the message
/// `set_config` would put in its envelope (`invalid JSON: ...`,
/// `missing or non-numeric schemaVersion`, `unsupported schemaVersion N
/// (...)`, `invalid config: ...`).
///
/// Public so every boundary that ingests a config (`brightfx-tracks`'
/// `fit_track_json`, for one) applies the same gate and the same prefixes
/// as `set_config`; a config one entry point rejects must not sail through
/// another. Does not clamp: the caller decides whether bounds apply.
pub fn parse_config(json: &str) -> Result<ParticleFxConfig, String> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|error| format!("invalid JSON: {error}"))?;

    // Version first. Deserializing first would report a renamed field in
    // a future config as a confusing serde error instead of this.
    let mut value = value;
    let version = value
        .get("schemaVersion")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| "missing or non-numeric schemaVersion".to_string())?;
    if !(MIN_SCHEMA_VERSION as u64..=SCHEMA_VERSION as u64).contains(&version) {
        return Err(unsupported_version_message(version));
    }
    migrate(&mut value, version);

    serde_json::from_value(value).map_err(|error| format!("invalid config: {error}"))
}

/// `{"ok":false,"error":"..."}`
///
/// Public because the FFI wrappers need it for their own boundary errors
/// (null handle, non-UTF-8 input). They must never hand-build this JSON —
/// one envelope implementation, or the two wrappers can drift.
pub fn err_envelope(message: &str) -> String {
    serde_json::json!({ "ok": false, "error": message }).to_string()
}

use crate::schema::{ParticleFxConfig, MIN_SCHEMA_VERSION, SCHEMA_VERSION};
use crate::simulation::{ParticleInstance, Simulation};

/// A `Simulation` plus the boundary behavior hosts need: JSON config ingest,
/// result envelopes, and panic containment.
///
/// Not `Send` or `Sync` — a handle must be driven from one thread.
pub struct AbiSimulation {
    sim: Simulation,
    /// Set when `guard` catches a panic during a mutating call. A panic
    /// leaves the simulation logically torn, so rather than let the host keep
    /// reading that state frame after frame, every subsequent operation
    /// becomes inert and `set_config` reports the condition. The host
    /// recovers by dropping the handle and building a new one.
    poisoned: bool,
    /// The documented contract is that a handle is driven from one thread.
    /// Without this the compiler would happily allow `thread::spawn(move ||
    /// sim.advance(dt))`, so the guarantee would exist only in prose.
    /// `*const ()` is neither `Send` nor `Sync`, which propagates here.
    ///
    /// This field carries no data — it exists purely to remove the
    /// auto-trait. Removing it (or replacing it with something `Send`)
    /// would silently widen the contract this type promises, without any
    /// compiler error to catch the change.
    _not_thread_safe: PhantomData<*const ()>,
    #[cfg(feature = "render")]
    renderer: crate::render::Renderer,
}

impl AbiSimulation {
    /// Infallible by design: it starts from `ParticleFxConfig::default()`, so
    /// there is exactly one config-ingest path (`set_config`) rather than a
    /// fallible constructor duplicating the same parse/version/clamp chain.
    pub fn new(seed: u64) -> Self {
        Self {
            sim: Simulation::new(ParticleFxConfig::default(), seed),
            poisoned: false,
            _not_thread_safe: PhantomData,
            #[cfg(feature = "render")]
            renderer: crate::render::Renderer::new(),
        }
    }

    /// True once a panic has been caught during a mutating call.
    pub fn is_poisoned(&self) -> bool {
        self.poisoned
    }

    /// Runs a mutating operation under `guard`. Returns `None` without
    /// running it if the simulation is already poisoned, and poisons the
    /// simulation and returns `None` if it panics. Every mutating entry point
    /// goes through this rather than calling `guard` directly — that is what
    /// makes the torn state unreadable instead of merely uncaught, and keeps
    /// the poison-on-panic rule in exactly one place.
    ///
    /// Returning an `Option` rather than taking a fallback means a fallback
    /// like `set_config`'s error envelope is only built when it is needed:
    /// callers supply it with `unwrap_or_else`, and the `()` entry points
    /// simply drop the `Option`.
    fn poisoning<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> Option<T> {
        if self.poisoned {
            return None;
        }
        let result = guard(None, || Some(f(self)));
        if result.is_none() {
            self.poisoned = true;
        }
        result
    }

    /// `poisoning`, for operations on the simulation alone.
    fn guard_mut<T>(&mut self, f: impl FnOnce(&mut Simulation) -> T) -> Option<T> {
        self.poisoning(|this| f(&mut this.sim))
    }

    /// Parses, version-checks, clamps, and applies a config.
    ///
    /// Returns a JSON envelope: `{"ok":true,"clamped":[...],"warnings":[...]}`
    /// on success, or `{"ok":false,"error":"..."}` on failure. `warnings`
    /// are advisory (the config is applied as given), e.g. a spawn rate
    /// that will overflow the particle pool. Failure is atomic — the
    /// previous config stays in place and the simulation keeps rendering.
    ///
    /// Success does *not* reset the simulation: live particles keep their
    /// state and the new config applies from the next frame on. That is what
    /// makes live parameter editing usable; re-seeding is `seek(0)`'s job.
    ///
    /// It does clear the baked position, because the new config may carry
    /// a different `emitterTrack`: the next `seek` replays from t=0
    /// rather than stepping forward from a state the new track did not
    /// produce.
    pub fn set_config(&mut self, json: &str) -> String {
        self.guard_mut(|sim| Self::set_config_inner(sim, json))
            .unwrap_or_else(|| err_envelope(POISONED_MESSAGE))
    }

    fn set_config_inner(sim: &mut Simulation, json: &str) -> String {
        let mut config = match parse_config(json) {
            Ok(config) => config,
            Err(message) => return err_envelope(&message),
        };
        let clamped = config.clamp_to_bounds();
        let warnings = config.pool_warnings();
        sim.set_config(config);
        ok_envelope_with_warnings(&clamped, &warnings)
    }

    /// Test accessor for the atomic-rejection test; also handy for hosts
    /// confirming which effect is loaded.
    pub fn config_name(&self) -> &str {
        self.sim.config_name()
    }

    /// Number of particles in the buffer. `u32`, not `usize`: this is the
    /// boundary module, and the C ABI exposes it as `uint32_t`. Widening it
    /// per-platform would break the ABI on 64-bit hosts.
    ///
    /// `Simulation` keeps its pool and its buffer the same length, so this is
    /// the only count the ABI exposes — two counts that must agree would be a
    /// bug waiting to happen.
    pub fn particle_count(&self) -> u32 {
        if self.poisoned {
            return 0;
        }
        self.sim.particle_count() as u32
    }

    /// Number of `f32` values per particle in the flat buffer. Hosts derive
    /// their stride from this instead of hardcoding it.
    ///
    /// Derived from `ParticleInstance` rather than written down: `simulation.rs`
    /// already asserts that struct is exactly 32 bytes, so adding a field there
    /// is a build error. Restating the float count as a literal would reopen
    /// that hole from the other side — the assert would force the byte number
    /// to be updated while this one silently kept reporting the old stride to
    /// every host.
    pub const PARTICLE_FLOATS: u32 =
        (std::mem::size_of::<ParticleInstance>() / std::mem::size_of::<f32>()) as u32;

    pub fn set_emitter(&mut self, x: f32, y: f32, vx: f32, vy: f32, active: bool) {
        self.guard_mut(|sim| sim.set_emitter(x, y, vx, vy, active));
    }

    /// Sets the logical-unit rectangle `cullMargin` is measured from, for
    /// a host with no viewport (sprite mode). `set_viewport` sets it itself.
    /// See `Simulation::set_bounds`.
    pub fn set_bounds(&mut self, width: f32, height: f32) {
        self.guard_mut(|sim| sim.set_bounds(width, height));
    }

    pub fn trigger_burst(&mut self) {
        self.guard_mut(|sim| sim.trigger_burst());
    }

    pub fn advance(&mut self, dt: f32) {
        self.guard_mut(|sim| sim.advance(dt));
    }

    /// Runs baked playback *through* the 1/60 s grid point at or after
    /// `time` (a `time` within tolerance of a grid point snaps to it), so
    /// every trigger authored at or before `time` has fired when `time`
    /// is rendered and the simulation may sit up to one step past it. A
    /// forward seek steps from the previous seek; anything else replays
    /// from t=0. The buffer is a pure function of `time` either way. A
    /// no-op when the config has no `emitterTrack`.
    pub fn seek(&mut self, time: f32) {
        self.guard_mut(|sim| sim.seek(time));
    }

    /// The particle buffer as a flat `f32` slice — the same values
    /// `buffer_ptr` exposes, but safe for Rust callers.
    ///
    /// This is the single place that reasons about pointer-and-length
    /// agreement. Callers that need it as a `Vec` should use
    /// `.to_vec()` on the result rather than re-deriving the raw read.
    pub fn buffer_slice(&self) -> &[f32] {
        let len = self.particle_count() as usize * Self::PARTICLE_FLOATS as usize;
        // SAFETY: `buffer_ptr` is the base of a `Vec<ParticleInstance>` that is
        // `#[repr(C)]` and exactly `PARTICLE_FLOATS` f32s wide (asserted at
        // compile time in `simulation.rs`), preallocated so it never moves, and
        // `particle_count` reports 0 when poisoned — so `len` never exceeds the
        // initialized region.
        unsafe { std::slice::from_raw_parts(self.buffer_ptr(), len) }
    }

    /// Start of the particle buffer, as a flat `f32` array of
    /// `particle_count() * PARTICLE_FLOATS` values.
    ///
    /// Valid until the next `advance`, `seek`, or `trigger_burst`. The
    /// address itself is stable for the life of the simulation (the buffer is
    /// preallocated), but hosts on WASM must still re-read it after
    /// `set_config`, which can grow linear memory and detach existing views.
    pub fn buffer_ptr(&self) -> *const f32 {
        self.sim.buffer().as_ptr() as *const f32
    }

    /// `poisoning`, for operations that read the simulation and write the
    /// renderer. Poisons on panic for the same reason: a half-drawn frame
    /// must not be handed to a host as if it were whole.
    #[cfg(feature = "render")]
    fn guard_render<T>(
        &mut self,
        f: impl FnOnce(&Simulation, &mut crate::render::Renderer) -> T,
    ) -> Option<T> {
        self.poisoning(|this| f(&this.sim, &mut this.renderer))
    }

    /// Allocates the frame for a `width x height` device-pixel viewport at
    /// `scale` device pixels per logical unit. Returns an envelope like
    /// `set_config`: `{"ok":true,"clamped":[...]}` with any of
    /// `viewport.width`, `viewport.height`, `viewport.scale`, or
    /// `{"ok":false,"error":"..."}` for a zero dimension.
    ///
    /// Reallocates the frame, so on WASM a host must re-read `frame_ptr`
    /// afterwards, as with `set_config` and the particle buffer.
    ///
    /// Also gives the simulation its bounds, `width / scale` by
    /// `height / scale` logical units, so `cullMargin` works with no further
    /// call. A rejected viewport leaves the bounds as they were.
    #[cfg(feature = "render")]
    pub fn set_viewport(&mut self, width: u32, height: u32, scale: f32) -> String {
        self.poisoning(|this| match this.renderer.set_viewport(width, height, scale) {
            Ok(clamped) => {
                // The renderer clamps width, height and scale, so read back
                // what it holds rather than the arguments.
                if let Some(vp) = this.renderer.viewport() {
                    this.sim.set_bounds(vp.width as f32 / vp.scale, vp.height as f32 / vp.scale);
                }
                ok_envelope(&clamped)
            }
            Err(message) => err_envelope(message),
        })
        .unwrap_or_else(|| err_envelope(POISONED_MESSAGE))
    }

    /// Rasterizes the current particle buffer into the frame. A no-op until
    /// `set_viewport` succeeds.
    #[cfg(feature = "render")]
    pub fn render(&mut self) {
        self.guard_render(|sim, renderer| renderer.render(sim.buffer(), sim.config()));
    }

    /// Start of the frame: `frame_len()` bytes of premultiplied RGBA8,
    /// row-major, `frame_width() * 4` bytes per row. Valid until the next
    /// `render`; the address is stable until the next `set_viewport`.
    #[cfg(feature = "render")]
    pub fn frame_ptr(&self) -> *const u8 {
        self.renderer.frame().as_ptr()
    }

    /// Byte length of the frame. 0 when poisoned or before a viewport is
    /// set, so a host's raw read of `frame_ptr` stays in bounds.
    #[cfg(feature = "render")]
    pub fn frame_len(&self) -> u32 {
        if self.poisoned {
            return 0;
        }
        self.renderer.frame().len() as u32
    }

    #[cfg(feature = "render")]
    pub fn frame_width(&self) -> u32 {
        if self.poisoned {
            return 0;
        }
        self.renderer.frame_width()
    }

    #[cfg(feature = "render")]
    pub fn frame_height(&self) -> u32 {
        if self.poisoned {
            return 0;
        }
        self.renderer.frame_height()
    }

    /// The frame as a byte slice, safe for Rust callers. The single place
    /// that reasons about pointer-and-length agreement for the frame.
    #[cfg(feature = "render")]
    pub fn frame_slice(&self) -> &[u8] {
        let len = self.frame_len() as usize;
        // SAFETY: `frame_ptr` is the base of the renderer's `Vec<u8>`, and
        // `frame_len` is that Vec's length or 0 when poisoned.
        unsafe { std::slice::from_raw_parts(self.frame_ptr(), len) }
    }
}

/// Returned by any entry point on a poisoned simulation, and by one that
/// poisons itself by panicking. The host's recovery is the same either way:
/// drop the handle and build a new one.
const POISONED_MESSAGE: &str = "simulation is poisoned by a panic; create a new one";

/// Brings a supported older config forward to `SCHEMA_VERSION`, one
/// version at a time. This is where each future step slots in.
fn migrate(value: &mut serde_json::Value, from: u64) {
    // 1 -> 2 -> 3 -> 4 only added vocabulary (and defaulted fields): the body
    // is already valid.
    if from < SCHEMA_VERSION as u64 {
        value["schemaVersion"] = serde_json::json!(SCHEMA_VERSION);
    }
}

fn unsupported_version_message(found: u64) -> String {
    format!("unsupported schemaVersion {found} (this build supports {MIN_SCHEMA_VERSION}-{SCHEMA_VERSION})")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_config_applies_the_same_gate_as_set_config() {
        // fit_track_json and any future boundary that ingests a config must
        // share set_config's version check and error prefixes, or a config
        // set_config would reject sails through the other entry point.
        let default = serde_json::to_string(&ParticleFxConfig::default()).unwrap();
        assert_eq!(parse_config(&default).unwrap(), ParticleFxConfig::default());

        let err = parse_config("{ not json").unwrap_err();
        assert!(err.starts_with("invalid JSON"), "got: {err}");

        let err = parse_config(r#"{"schemaVersion": 5, "somethingEntirelyNew": true}"#).unwrap_err();
        assert!(err.contains("unsupported schemaVersion 5"), "got: {err}");

        let err = parse_config(r#"{"glowRadius": 1}"#).unwrap_err();
        assert_eq!(err, "missing or non-numeric schemaVersion");

        let err = parse_config(r#"{"schemaVersion": 1, "glowRadius": "wide"}"#).unwrap_err();
        assert!(err.starts_with("invalid config"), "got: {err}");
    }
    use crate::schema::{EmitterKeyframe, EmitterTrack, EmitterTrigger, TriggerKind};

    /// Panicking tests print a backtrace to stderr by default, which makes a
    /// passing run look like a failing one. This silences that for the
    /// duration of a deliberate panic.
    fn without_panic_output<T>(f: impl FnOnce() -> T) -> T {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let result = f();
        std::panic::set_hook(previous);
        result
    }

    /// Poisons a simulation the way a real panic would: through the guard,
    /// so the tests do not depend on any one entry point panicking.
    fn poison(sim: &mut AbiSimulation) {
        without_panic_output(|| sim.guard_mut(|_| panic!("boom")));
    }

    #[test]
    fn the_guard_returns_the_value_when_nothing_panics() {
        assert_eq!(guard(0, || 7), 7);
    }

    #[test]
    fn the_guard_returns_the_fallback_when_the_closure_panics() {
        let result = without_panic_output(|| guard(-1, || panic!("boom")));
        assert_eq!(result, -1, "a panic must not cross the boundary");
    }

    #[test]
    fn an_ok_envelope_lists_the_clamped_fields() {
        let json = ok_envelope(&["glowRadius", "drag"]);
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["ok"], true);
        assert_eq!(value["clamped"][0], "glowRadius");
        assert_eq!(value["clamped"][1], "drag");
    }

    #[test]
    fn an_ok_envelope_with_no_clamping_carries_an_empty_list() {
        let json = ok_envelope(&[]);
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["ok"], true);
        assert_eq!(value["clamped"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn an_error_envelope_carries_the_message() {
        let json = err_envelope("everything is on fire");
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["ok"], false);
        assert_eq!(value["error"], "everything is on fire");
    }

    #[test]
    fn an_error_envelope_escapes_quotes_in_the_message() {
        let json = err_envelope(r#"unexpected " character"#);
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["error"], r#"unexpected " character"#);
    }

    fn valid_config_json() -> String {
        serde_json::to_string(&ParticleFxConfig::default()).unwrap()
    }

    fn parse(envelope: &str) -> serde_json::Value {
        serde_json::from_str(envelope).unwrap()
    }

    #[test]
    fn a_new_simulation_starts_from_the_default_config() {
        let sim = AbiSimulation::new(42);
        assert_eq!(sim.particle_count(), 0);
        assert_eq!(sim.config_name(), "Untitled");
    }

    #[test]
    fn a_config_that_overflows_the_pool_is_applied_with_a_warning() {
        let mut config = ParticleFxConfig { lifetime_min: 300.0, lifetime_max: 300.0, ..Default::default() };
        config.emitter.spawn_rate_while_active = 5.0;
        let mut sim = AbiSimulation::new(42);
        let value = parse(&sim.set_config(&serde_json::to_string(&config).unwrap()));
        assert_eq!(value["ok"], true);
        assert_eq!(value["clamped"].as_array().unwrap().len(), 0);
        assert_eq!(value["warnings"].as_array().unwrap().len(), 1);
        assert!(value["warnings"][0].as_str().unwrap().contains("particle pool"));
    }

    #[test]
    fn a_valid_config_is_accepted_with_nothing_clamped() {
        let mut sim = AbiSimulation::new(42);
        let value = parse(&sim.set_config(&valid_config_json()));
        assert_eq!(value["ok"], true);
        assert_eq!(value["clamped"].as_array().unwrap().len(), 0);
        assert_eq!(value["warnings"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn an_out_of_range_field_is_clamped_and_reported() {
        let config = ParticleFxConfig {
            glow_radius: 9_000.0,
            ..Default::default()
        };
        let mut sim = AbiSimulation::new(42);

        let value = parse(&sim.set_config(&serde_json::to_string(&config).unwrap()));

        assert_eq!(value["ok"], true);
        assert_eq!(value["clamped"][0], "glowRadius");
    }

    #[test]
    fn malformed_json_is_rejected() {
        let mut sim = AbiSimulation::new(42);
        let value = parse(&sim.set_config("{ not json"));
        assert_eq!(value["ok"], false);
        assert!(value["error"].as_str().unwrap().contains("invalid JSON"));
    }

    #[test]
    fn a_config_with_a_newer_schema_version_is_rejected_clearly() {
        let mut value = serde_json::to_value(ParticleFxConfig::default()).unwrap();
        value["schemaVersion"] = serde_json::json!(999);
        let mut sim = AbiSimulation::new(42);

        let result = parse(&sim.set_config(&value.to_string()));

        assert_eq!(result["ok"], false);
        let message = result["error"].as_str().unwrap();
        assert!(message.contains("999"), "message must name the version: {message}");
        assert!(message.contains("unsupported schemaVersion"), "got: {message}");
    }

    #[test]
    fn the_version_is_checked_before_the_body_is_deserialized() {
        // A future config whose body today's struct cannot parse must still
        // produce the version error, not a confusing serde error.
        let json = r#"{"schemaVersion": 5, "somethingEntirelyNew": true}"#;
        let mut sim = AbiSimulation::new(42);

        let result = parse(&sim.set_config(json));

        let message = result["error"].as_str().unwrap();
        assert!(message.contains("unsupported schemaVersion"), "got: {message}");
    }

    #[test]
    fn older_versions_still_load_and_read_back_as_the_current_version() {
        // v2, v3 and v4 only added vocabulary and defaulted fields, so an
        // older body is a valid current body once relabelled --
        // spinDirection and cullMargin default when absent.
        for version in [1u64, 2, 3] {
            let mut value = serde_json::to_value(ParticleFxConfig::default()).unwrap();
            value["schemaVersion"] = serde_json::json!(version);
            value.as_object_mut().unwrap().remove("spinDirection");
            value.as_object_mut().unwrap().remove("cullMargin");

            let config = parse_config(&value.to_string()).unwrap();
            assert_eq!(config.schema_version, SCHEMA_VERSION, "v{version}");
            assert_eq!(config.spin_direction, crate::schema::SpinDirection::Fixed, "v{version}");
            assert_eq!(config.cull_margin, None, "v{version}");

            let mut sim = AbiSimulation::new(42);
            assert_eq!(parse(&sim.set_config(&value.to_string()))["ok"], true, "v{version}");
        }
    }

    #[test]
    fn the_current_version_is_4_and_loads() {
        assert_eq!(SCHEMA_VERSION, 4);
        let json = serde_json::to_string(&ParticleFxConfig::default()).unwrap();
        assert_eq!(parse_config(&json).unwrap().schema_version, 4);
    }

    #[test]
    fn versions_outside_the_supported_range_are_rejected_clearly() {
        for version in [0u64, 5] {
            let mut value = serde_json::to_value(ParticleFxConfig::default()).unwrap();
            value["schemaVersion"] = serde_json::json!(version);
            let message = parse_config(&value.to_string()).unwrap_err();
            assert!(message.contains(&format!("unsupported schemaVersion {version}")), "got: {message}");
            assert!(message.contains("1-4"), "message must name the supported range: {message}");
        }
    }

    #[test]
    fn a_config_missing_its_schema_version_is_rejected() {
        let mut value = serde_json::to_value(ParticleFxConfig::default()).unwrap();
        value.as_object_mut().unwrap().remove("schemaVersion");
        let mut sim = AbiSimulation::new(42);

        let result = parse(&sim.set_config(&value.to_string()));

        assert_eq!(result["ok"], false);
        assert!(result["error"].as_str().unwrap().contains("schemaVersion"));
    }

    #[test]
    fn a_rejected_config_leaves_the_previous_one_in_place() {
        let good = ParticleFxConfig {
            name: "Keeper".into(),
            ..Default::default()
        };
        let mut sim = AbiSimulation::new(42);
        assert_eq!(parse(&sim.set_config(&serde_json::to_string(&good).unwrap()))["ok"], true);

        assert_eq!(parse(&sim.set_config("{ garbage"))["ok"], false);

        assert_eq!(sim.config_name(), "Keeper", "config was partially applied");
    }

    /// Reads the buffer the way a host does: as a flat f32 array.
    fn read_buffer(sim: &AbiSimulation) -> Vec<f32> {
        sim.buffer_slice().to_vec()
    }

    #[test]
    fn advancing_an_active_emitter_produces_particles() {
        let mut sim = AbiSimulation::new(42);
        sim.set_config(&valid_config_json());
        sim.set_emitter(10.0, 20.0, 1.0, 0.0, true);

        for _ in 0..30 {
            sim.advance(0.015625);
        }

        assert!(sim.particle_count() > 0, "an active emitter spawned nothing");
    }

    #[test]
    fn the_flat_buffer_holds_eight_finite_floats_per_particle() {
        let mut sim = AbiSimulation::new(42);
        sim.set_config(&valid_config_json());
        sim.set_emitter(10.0, 20.0, 1.0, 0.0, true);
        for _ in 0..30 {
            sim.advance(0.015625);
        }

        let flat = read_buffer(&sim);

        assert_eq!(
            flat.len(),
            sim.particle_count() as usize * AbiSimulation::PARTICLE_FLOATS as usize
        );
        assert!(flat.iter().all(|v| v.is_finite()), "buffer contains NaN or inf");
    }

    #[test]
    fn the_flat_buffer_matches_the_typed_buffer_field_for_field() {
        let mut sim = AbiSimulation::new(42);
        sim.set_config(&valid_config_json());
        sim.set_emitter(10.0, 20.0, 1.0, 0.0, true);
        for _ in 0..30 {
            sim.advance(0.015625);
        }

        let flat = read_buffer(&sim);
        let typed = sim.sim.buffer();
        assert!(typed.len() > 1, "test is stronger with several particles");

        // Every particle, every field -- a reordered struct or a wrong stride
        // would slip past a check of index 0 alone.
        for (index, particle) in typed.iter().enumerate() {
            let base = index * AbiSimulation::PARTICLE_FLOATS as usize;
            assert_eq!(flat[base], particle.x, "particle {index} x");
            assert_eq!(flat[base + 1], particle.y, "particle {index} y");
            assert_eq!(flat[base + 2], particle.size, "particle {index} size");
            assert_eq!(flat[base + 3], particle.rotation, "particle {index} rotation");
            for channel in 0..4 {
                assert_eq!(
                    flat[base + 4 + channel],
                    particle.color[channel],
                    "particle {index} color[{channel}]"
                );
            }
        }
    }

    #[test]
    fn a_burst_spawns_particles_immediately() {
        let mut sim = AbiSimulation::new(42);
        sim.set_config(&valid_config_json());
        assert_eq!(sim.particle_count(), 0);

        sim.trigger_burst();

        assert!(sim.particle_count() > 0, "burst produced no particles");
    }

    #[test]
    fn the_buffer_pointer_is_never_null_even_when_empty() {
        let sim = AbiSimulation::new(42);
        assert_eq!(sim.particle_count(), 0);
        assert!(!sim.buffer_ptr().is_null(), "hosts must always get a real address");
    }

    #[test]
    fn a_poisoned_simulation_stops_mutating_and_reports_the_condition() {
        // guard() catches the unwind but cannot undo a half-finished mutation,
        // so a panicked simulation must go inert rather than let the host keep
        // reading torn state frame after frame.
        let mut sim = AbiSimulation::new(42);
        sim.set_config(&valid_config_json());
        sim.set_emitter(0.0, 0.0, 1.5, 0.0, true);
        for _ in 0..30 {
            sim.advance(0.015625);
        }
        let count_before = sim.particle_count();
        assert!(count_before > 0, "test is vacuous with no particles");
        assert!(!sim.is_poisoned());

        poison(&mut sim);

        assert!(sim.is_poisoned(), "a caught panic must poison the simulation");
        assert_eq!(sim.particle_count(), 0, "poisoned sim must report no particles");

        // Further mutation is inert. This checks the underlying pool, NOT
        // particle_count(): that reports 0 for any poisoned simulation, so
        // asserting on it here would pass whether or not mutation actually
        // stopped -- the very thing this test exists to prove.
        let pool_before = sim.sim.particle_count();
        sim.advance(0.015625);
        sim.trigger_burst();
        assert_eq!(sim.sim.particle_count(), pool_before, "poisoned sim still mutated");

        // And config loads report the condition instead of silently succeeding.
        let value = parse(&sim.set_config(&valid_config_json()));
        assert_eq!(value["ok"], false);
        assert!(value["error"].as_str().unwrap().contains("poisoned"));
    }

    #[test]
    fn a_poisoned_simulation_reports_no_particles_so_host_reads_stay_in_bounds() {
        // particle_count() sizes the host's raw read of buffer_ptr(). The count
        // comes from the pool and the pointer from the buffer, and a panic
        // caught mid-rebuild can leave those disagreeing -- so a poisoned
        // simulation must not hand out a length it cannot vouch for.
        let mut sim = AbiSimulation::new(42);
        sim.set_config(&valid_config_json());
        sim.set_emitter(0.0, 0.0, 1.5, 0.0, true);
        for _ in 0..30 {
            sim.advance(0.015625);
        }
        assert!(sim.particle_count() > 0, "test is vacuous with no particles");

        poison(&mut sim);

        assert_eq!(sim.particle_count(), 0, "poisoned sim must report no particles");
        assert!(!sim.buffer_ptr().is_null(), "pointer stays valid, just zero-length");
        assert_eq!(read_buffer(&sim).len(), 0, "host read must be empty, not stale");
    }

    #[test]
    fn a_healthy_simulation_is_not_poisoned_by_ordinary_use() {
        let mut sim = AbiSimulation::new(42);
        sim.set_config(&valid_config_json());
        sim.set_emitter(0.0, 0.0, 1.5, 0.0, true);
        for _ in 0..30 {
            sim.advance(0.015625);
        }
        sim.trigger_burst();
        sim.seek(0.5);
        assert!(!sim.is_poisoned(), "ordinary use must never poison");
    }

    #[test]
    fn a_successful_set_config_does_not_reset_live_particles() {
        // The spec depends on this: parameter edits in the authoring tool must
        // be visible immediately without the preview flashing back to empty.
        let mut sim = AbiSimulation::new(42);
        sim.set_config(&valid_config_json());
        sim.set_emitter(0.0, 0.0, 1.5, 0.0, true);
        for _ in 0..40 {
            sim.advance(0.015625);
        }
        let before = sim.particle_count();
        assert!(before > 0, "test is vacuous with no live particles");

        let edited = ParticleFxConfig {
            start_size: 12.0,
            ..Default::default()
        };
        let value = parse(&sim.set_config(&serde_json::to_string(&edited).unwrap()));
        assert_eq!(value["ok"], true);

        assert_eq!(sim.particle_count(), before, "set_config wiped live particles");
    }

    #[test]
    fn seeking_without_an_emitter_track_is_a_no_op() {
        let mut sim = AbiSimulation::new(42);
        sim.set_config(&valid_config_json()); // default config has no track
        sim.seek(5.0);
        assert_eq!(sim.particle_count(), 0, "seek invented particles with no track");
    }

    #[test]
    fn seeking_the_same_time_twice_reproduces_the_same_buffer() {
        let config = ParticleFxConfig {
            emitter_track: Some(EmitterTrack {
                duration: 2.0,
                keyframes: vec![
                    EmitterKeyframe { time: 0.0, x: 0.0, y: 0.0, vx: Some(1.0), vy: Some(0.0) },
                    EmitterKeyframe { time: 2.0, x: 60.0, y: 0.0, vx: Some(1.0), vy: Some(0.0) },
                ],
                triggers: vec![EmitterTrigger { time: 0.0, kind: TriggerKind::StartContinuous }],
            }),
            ..Default::default()
        };
        let json = serde_json::to_string(&config).unwrap();

        let mut a = AbiSimulation::new(7);
        a.set_config(&json);
        a.seek(1.0);

        let mut b = AbiSimulation::new(7);
        b.set_config(&json);
        b.seek(1.0);

        assert_eq!(a.particle_count(), b.particle_count());
        assert_eq!(read_buffer(&a), read_buffer(&b));
    }

    #[cfg(feature = "render")]
    mod render {
        use super::*;

        fn simulate_some_particles(sim: &mut AbiSimulation) {
            sim.set_config(&valid_config_json());
            sim.set_emitter(40.0, 30.0, 1.5, 0.0, true);
            for _ in 0..30 {
                sim.advance(0.015625);
            }
            assert!(sim.particle_count() > 0, "test is vacuous with no particles");
        }

        #[test]
        fn set_viewport_returns_an_ok_envelope_with_clamped_fields() {
            let mut sim = AbiSimulation::new(1);
            let value = parse(&sim.set_viewport(64, 48, 1.0));
            assert_eq!(value["ok"], true);
            assert_eq!(value["clamped"].as_array().unwrap().len(), 0);

            let value = parse(&sim.set_viewport(64, 48, 100.0));
            assert_eq!(value["ok"], true);
            assert_eq!(value["clamped"][0], "viewport.scale");
        }

        #[test]
        fn set_viewport_feeds_the_bounds_in_logical_units() {
            let mut sim = AbiSimulation::new(1);
            sim.set_viewport(200, 100, 2.0);
            assert_eq!(sim.sim.bounds(), Some((100.0, 50.0)));
        }

        #[test]
        fn the_bounds_follow_the_clamped_scale() {
            let mut sim = AbiSimulation::new(1);
            // 100.0 clamps to MAX_SCALE (8.0).
            sim.set_viewport(80, 40, 100.0);
            assert_eq!(sim.sim.bounds(), Some((10.0, 5.0)));
        }

        #[test]
        fn a_rejected_viewport_leaves_the_bounds_alone() {
            let mut sim = AbiSimulation::new(1);
            sim.set_viewport(80, 40, 1.0);
            sim.set_viewport(0, 40, 1.0);
            assert_eq!(sim.sim.bounds(), Some((80.0, 40.0)));
        }

        #[test]
        fn set_bounds_sets_them_without_a_viewport() {
            let mut sim = AbiSimulation::new(1);
            sim.set_bounds(320.0, 180.0);
            assert_eq!(sim.sim.bounds(), Some((320.0, 180.0)));
            assert_eq!(sim.frame_len(), 0, "set_bounds must not allocate a frame");
        }

        #[test]
        fn a_zero_viewport_is_rejected_with_a_message() {
            let mut sim = AbiSimulation::new(1);
            let value = parse(&sim.set_viewport(0, 48, 1.0));
            assert_eq!(value["ok"], false);
            assert!(value["error"].as_str().unwrap().contains("nonzero"));
        }

        #[test]
        fn frame_accessors_report_zero_until_a_viewport_is_set() {
            let sim = AbiSimulation::new(1);
            assert_eq!(sim.frame_len(), 0);
            assert_eq!(sim.frame_width(), 0);
            assert_eq!(sim.frame_height(), 0);
            assert!(sim.frame_slice().is_empty());
            assert!(!sim.frame_ptr().is_null(), "hosts must always get a real address");
        }

        #[test]
        fn render_fills_the_frame_with_the_particles() {
            let mut sim = AbiSimulation::new(1);
            simulate_some_particles(&mut sim);
            sim.set_viewport(80, 60, 1.0);
            sim.render();
            assert_eq!(sim.frame_len(), 80 * 60 * 4);
            assert_eq!(sim.frame_width(), 80);
            assert_eq!(sim.frame_height(), 60);
            assert_eq!(sim.frame_slice().len(), 80 * 60 * 4);
            assert!(sim.frame_slice().iter().any(|&b| b != 0), "rendered nothing");
        }

        #[test]
        fn render_without_a_viewport_is_a_no_op() {
            let mut sim = AbiSimulation::new(1);
            simulate_some_particles(&mut sim);
            sim.render();
            assert_eq!(sim.frame_len(), 0);
        }

        #[test]
        fn a_poisoned_simulation_reports_no_frame_and_does_not_render() {
            let mut sim = AbiSimulation::new(1);
            simulate_some_particles(&mut sim);
            sim.set_viewport(80, 60, 1.0);
            poison(&mut sim);
            sim.render();
            assert!(sim.is_poisoned());
            assert_eq!(sim.frame_len(), 0);
            assert!(sim.frame_slice().is_empty());
            assert!(sim.renderer.frame().iter().all(|&b| b == 0), "poisoned render still drew");
            let value = parse(&sim.set_viewport(10, 10, 1.0));
            assert_eq!(value["ok"], false);
            assert!(value["error"].as_str().unwrap().contains("poisoned"));
        }

        #[test]
        fn a_panic_during_render_poisons_the_simulation() {
            let mut sim = AbiSimulation::new(1);
            sim.set_viewport(8, 8, 1.0);
            without_panic_output(|| sim.guard_render(|_, _| panic!("boom")));
            assert!(sim.is_poisoned());
        }
    }
}
