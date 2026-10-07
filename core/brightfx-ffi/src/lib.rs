//! C ABI for BrightFX. Every function here is a type translation around a
//! single `brightfx_core::abi` call — no logic lives in this crate, so it
//! cannot drift from `brightfx-wasm`.

use brightfx_core::abi::{err_envelope, AbiSimulation};
use std::ffi::{c_char, CStr, CString};

/// Opaque handle. Single-threaded: drive one handle from one thread.
pub struct BfxSimulation {
    inner: AbiSimulation,
}

/// Creates a simulation seeded with `seed`, starting from the default config.
/// Never fails; load a real config with `bfx_set_config`.
#[no_mangle]
pub extern "C" fn bfx_simulation_new(seed: u64) -> *mut BfxSimulation {
    Box::into_raw(Box::new(BfxSimulation {
        inner: AbiSimulation::new(seed),
    }))
}

/// Frees a simulation. Safe to call with NULL. The handle must not be used
/// afterwards.
///
/// # Safety
/// `sim` must be a pointer returned by `bfx_simulation_new`, freed at most once.
#[no_mangle]
pub unsafe extern "C" fn bfx_simulation_free(sim: *mut BfxSimulation) {
    if !sim.is_null() {
        drop(Box::from_raw(sim));
    }
}

/// Loads a JSON config. Returns an owned JSON envelope the caller must release
/// with `bfx_string_free`:
/// `{"ok":true,"clamped":[...],"warnings":[...]}` or `{"ok":false,"error":"..."}`.
///
/// # Safety
/// `sim` must be a valid handle or NULL; `json` must be a NUL-terminated
/// string or NULL.
#[no_mangle]
pub unsafe extern "C" fn bfx_set_config(
    sim: *mut BfxSimulation,
    json: *const c_char,
) -> *mut c_char {
    let Some(sim) = sim.as_mut() else {
        return to_owned_c_string(&err_envelope("null simulation handle"));
    };
    if json.is_null() {
        return to_owned_c_string(&err_envelope("null config string"));
    }
    match CStr::from_ptr(json).to_str() {
        Ok(text) => to_owned_c_string(&sim.inner.set_config(text)),
        Err(_) => to_owned_c_string(&err_envelope("config string is not valid UTF-8")),
    }
}

/// Releases a string returned by `bfx_set_config` or `bfx_set_viewport`
/// (any function in this library that returns an owned `char*`). Safe to
/// call with NULL.
///
/// # Safety
/// `text` must be a pointer returned by this library, freed at most once.
#[no_mangle]
pub unsafe extern "C" fn bfx_string_free(text: *mut c_char) {
    if !text.is_null() {
        drop(CString::from_raw(text));
    }
}

/// # Safety
/// `sim` must be a valid handle or NULL.
#[no_mangle]
pub unsafe extern "C" fn bfx_set_emitter(
    sim: *mut BfxSimulation,
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    active: bool,
) {
    if let Some(sim) = sim.as_mut() {
        sim.inner.set_emitter(x, y, vx, vy, active);
    }
}

/// # Safety
/// `sim` must be a valid handle or NULL.
#[no_mangle]
pub unsafe extern "C" fn bfx_trigger_burst(sim: *mut BfxSimulation) {
    if let Some(sim) = sim.as_mut() {
        sim.inner.trigger_burst();
    }
}

/// Advances the simulation by `dt` seconds.
///
/// # Safety
/// `sim` must be a valid handle or NULL.
#[no_mangle]
pub unsafe extern "C" fn bfx_advance(sim: *mut BfxSimulation, dt: f32) {
    if let Some(sim) = sim.as_mut() {
        sim.inner.advance(dt);
    }
}

/// Runs baked playback on the config's emitter track through the 1/60 s
/// grid point at or after `time`, so every trigger authored at or before
/// `time` has fired and the simulation may sit up to one step past it: a
/// seek at or after the previous one steps forward
/// from it, and any other call sequence -- a backward seek, or any
/// `bfx_advance`, `bfx_trigger_burst`, `bfx_set_emitter`, or
/// `bfx_set_config` since -- replays from t=0, giving the same buffer
/// either way.
///
/// # Safety
/// `sim` must be a valid handle or NULL.
#[no_mangle]
pub unsafe extern "C" fn bfx_seek(sim: *mut BfxSimulation, time: f32) {
    if let Some(sim) = sim.as_mut() {
        sim.inner.seek(time);
    }
}

/// Start of the particle buffer: `bfx_particle_count() * bfx_particle_floats()`
/// contiguous floats. Returns NULL for a NULL handle. Valid until the next
/// `bfx_advance`, `bfx_seek`, or `bfx_trigger_burst`.
///
/// # Safety
/// `sim` must be a valid handle or NULL.
#[no_mangle]
pub unsafe extern "C" fn bfx_buffer_ptr(sim: *const BfxSimulation) -> *const f32 {
    match sim.as_ref() {
        Some(sim) => sim.inner.buffer_ptr(),
        None => std::ptr::null(),
    }
}

/// # Safety
/// `sim` must be a valid handle or NULL.
#[no_mangle]
pub unsafe extern "C" fn bfx_particle_count(sim: *const BfxSimulation) -> u32 {
    match sim.as_ref() {
        Some(sim) => sim.inner.particle_count(),
        None => 0,
    }
}

/// Floats per particle in the buffer. Hosts derive their stride from this
/// rather than hardcoding it.
#[no_mangle]
pub extern "C" fn bfx_particle_floats() -> u32 {
    AbiSimulation::PARTICLE_FLOATS
}

/// Whether a panic has poisoned this simulation. A poisoned simulation is
/// inert: every operation is a no-op, `bfx_particle_count` reports 0, and
/// `bfx_set_config` returns an error envelope. Recover by freeing the
/// handle and creating a new one. Returns `false` for a NULL handle.
///
/// # Safety
/// `sim` must be a valid handle or NULL.
#[no_mangle]
pub unsafe extern "C" fn bfx_is_poisoned(sim: *const BfxSimulation) -> bool {
    match sim.as_ref() {
        Some(sim) => sim.inner.is_poisoned(),
        None => false,
    }
}

/// Allocates the frame for a `width x height` device-pixel viewport at
/// `scale` device pixels per logical unit. Returns an owned envelope the
/// caller must release with `bfx_string_free`:
/// `{"ok":true,"clamped":[...],"warnings":[...]}` or `{"ok":false,"error":"..."}`.
///
/// # Safety
/// `sim` must be a valid handle or NULL.
#[no_mangle]
pub unsafe extern "C" fn bfx_set_viewport(
    sim: *mut BfxSimulation,
    width: u32,
    height: u32,
    scale: f32,
) -> *mut c_char {
    match sim.as_mut() {
        Some(sim) => to_owned_c_string(&sim.inner.set_viewport(width, height, scale)),
        None => to_owned_c_string(&err_envelope("null simulation handle")),
    }
}

/// Rasterizes the current particle buffer into the frame. A no-op until
/// `bfx_set_viewport` succeeds.
///
/// # Safety
/// `sim` must be a valid handle or NULL.
#[no_mangle]
pub unsafe extern "C" fn bfx_render(sim: *mut BfxSimulation) {
    if let Some(sim) = sim.as_mut() {
        sim.inner.render();
    }
}

/// Start of the frame: `bfx_frame_len()` bytes of premultiplied RGBA8,
/// row-major, `bfx_frame_width() * 4` bytes per row. Returns NULL for a
/// NULL handle. Valid until the next `bfx_render`; the address is stable
/// until the next `bfx_set_viewport`.
///
/// # Safety
/// `sim` must be a valid handle or NULL.
#[no_mangle]
pub unsafe extern "C" fn bfx_frame_ptr(sim: *const BfxSimulation) -> *const u8 {
    match sim.as_ref() {
        Some(sim) => sim.inner.frame_ptr(),
        None => std::ptr::null(),
    }
}

/// Byte length of the frame; 0 before a viewport is set or when poisoned.
///
/// # Safety
/// `sim` must be a valid handle or NULL.
#[no_mangle]
pub unsafe extern "C" fn bfx_frame_len(sim: *const BfxSimulation) -> u32 {
    match sim.as_ref() {
        Some(sim) => sim.inner.frame_len(),
        None => 0,
    }
}

/// Width of the frame in device pixels; 0 before a viewport is set or when
/// poisoned.
///
/// # Safety
/// `sim` must be a valid handle or NULL.
#[no_mangle]
pub unsafe extern "C" fn bfx_frame_width(sim: *const BfxSimulation) -> u32 {
    match sim.as_ref() {
        Some(sim) => sim.inner.frame_width(),
        None => 0,
    }
}

/// Height of the frame in device pixels; 0 before a viewport is set or when
/// poisoned.
///
/// # Safety
/// `sim` must be a valid handle or NULL.
#[no_mangle]
pub unsafe extern "C" fn bfx_frame_height(sim: *const BfxSimulation) -> u32 {
    match sim.as_ref() {
        Some(sim) => sim.inner.frame_height(),
        None => 0,
    }
}

/// Leaks the string to the caller, who must return it via `bfx_string_free`.
/// The interior-NUL case cannot arise from our own JSON, but is handled
/// rather than unwrapped so no input can panic across the boundary.
fn to_owned_c_string(text: &str) -> *mut c_char {
    match CString::new(text) {
        Ok(owned) => owned.into_raw(),
        Err(_) => CString::new(r#"{"ok":false,"error":"internal encoding error"}"#)
            .expect("static string has no interior NUL")
            .into_raw(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::{CStr, CString};

    fn envelope_from(ptr: *mut c_char) -> serde_json::Value {
        assert!(!ptr.is_null());
        let text = unsafe { CStr::from_ptr(ptr) }.to_str().unwrap().to_owned();
        unsafe { bfx_string_free(ptr) };
        serde_json::from_str(&text).unwrap()
    }

    #[test]
    fn a_simulation_can_be_created_configured_advanced_and_freed() {
        let sim = bfx_simulation_new(42);
        assert!(!sim.is_null());

        let config = CString::new(
            serde_json::to_string(&brightfx_core::ParticleFxConfig::default()).unwrap(),
        )
        .unwrap();
        let envelope = envelope_from(unsafe { bfx_set_config(sim, config.as_ptr()) });
        assert_eq!(envelope["ok"], true);

        unsafe { bfx_set_emitter(sim, 0.0, 0.0, 1.0, 0.0, true) };
        for _ in 0..30 {
            unsafe { bfx_advance(sim, 0.015625) };
        }

        assert!(unsafe { bfx_particle_count(sim) } > 0);
        assert!(!unsafe { bfx_buffer_ptr(sim) }.is_null());

        unsafe { bfx_simulation_free(sim) };
    }

    #[test]
    fn the_stride_accessor_reports_eight_floats() {
        assert_eq!(bfx_particle_floats(), 8);
    }

    #[test]
    fn every_entry_point_tolerates_a_null_handle() {
        let config = CString::new("{}").unwrap();

        // None of these may dereference null; each returns a safe default.
        assert_eq!(unsafe { bfx_particle_count(std::ptr::null()) }, 0);
        assert!(unsafe { bfx_buffer_ptr(std::ptr::null()) }.is_null());
        unsafe { bfx_set_emitter(std::ptr::null_mut(), 0.0, 0.0, 0.0, 0.0, true) };
        unsafe { bfx_advance(std::ptr::null_mut(), 0.016) };
        unsafe { bfx_seek(std::ptr::null_mut(), 1.0) };
        unsafe { bfx_trigger_burst(std::ptr::null_mut()) };
        unsafe { bfx_simulation_free(std::ptr::null_mut()) };
        // Documented safe on NULL like the rest; hosts free envelopes they
        // may have received as NULL.
        unsafe { bfx_string_free(std::ptr::null_mut()) };

        let envelope = envelope_from(unsafe { bfx_set_config(std::ptr::null_mut(), config.as_ptr()) });
        assert_eq!(envelope["ok"], false);
    }

    #[test]
    fn a_null_config_string_is_rejected_rather_than_dereferenced() {
        let sim = bfx_simulation_new(1);
        let envelope = envelope_from(unsafe { bfx_set_config(sim, std::ptr::null()) });
        assert_eq!(envelope["ok"], false);
        unsafe { bfx_simulation_free(sim) };
    }

    #[test]
    fn non_utf8_config_bytes_are_rejected_rather_than_panicking() {
        let sim = bfx_simulation_new(1);
        let invalid = [0xffu8, 0xfe, 0x00];
        let envelope =
            envelope_from(unsafe { bfx_set_config(sim, invalid.as_ptr() as *const c_char) });
        assert_eq!(envelope["ok"], false);
        unsafe { bfx_simulation_free(sim) };
    }

    #[test]
    fn a_healthy_handle_reports_not_poisoned_and_a_null_handle_reports_false() {
        let sim = bfx_simulation_new(1);
        assert!(!unsafe { bfx_is_poisoned(sim) });
        assert!(!unsafe { bfx_is_poisoned(std::ptr::null()) });
        unsafe { bfx_simulation_free(sim) };
    }

    #[test]
    fn a_frame_can_be_requested_rendered_and_read() {
        let sim = bfx_simulation_new(42);
        let config = CString::new(
            serde_json::to_string(&brightfx_core::ParticleFxConfig::default()).unwrap(),
        )
        .unwrap();
        envelope_from(unsafe { bfx_set_config(sim, config.as_ptr()) });
        unsafe { bfx_set_emitter(sim, 40.0, 30.0, 1.0, 0.0, true) };
        for _ in 0..30 {
            unsafe { bfx_advance(sim, 0.015625) };
        }

        let envelope = envelope_from(unsafe { bfx_set_viewport(sim, 80, 60, 1.0) });
        assert_eq!(envelope["ok"], true);
        unsafe { bfx_render(sim) };

        let len = unsafe { bfx_frame_len(sim) };
        assert_eq!(len, 80 * 60 * 4);
        assert_eq!(unsafe { bfx_frame_width(sim) }, 80);
        assert_eq!(unsafe { bfx_frame_height(sim) }, 60);
        let ptr = unsafe { bfx_frame_ptr(sim) };
        assert!(!ptr.is_null());
        let bytes = unsafe { std::slice::from_raw_parts(ptr, len as usize) };
        assert!(bytes.iter().any(|&b| b != 0), "rendered nothing");

        unsafe { bfx_simulation_free(sim) };
    }

    #[test]
    fn a_zero_viewport_returns_an_error_envelope() {
        let sim = bfx_simulation_new(1);
        let envelope = envelope_from(unsafe { bfx_set_viewport(sim, 0, 10, 1.0) });
        assert_eq!(envelope["ok"], false);
        unsafe { bfx_simulation_free(sim) };
    }

    #[test]
    fn frame_entry_points_tolerate_a_null_handle() {
        assert_eq!(unsafe { bfx_frame_len(std::ptr::null()) }, 0);
        assert_eq!(unsafe { bfx_frame_width(std::ptr::null()) }, 0);
        assert_eq!(unsafe { bfx_frame_height(std::ptr::null()) }, 0);
        assert!(unsafe { bfx_frame_ptr(std::ptr::null()) }.is_null());
        unsafe { bfx_render(std::ptr::null_mut()) };
        let envelope = envelope_from(unsafe { bfx_set_viewport(std::ptr::null_mut(), 8, 8, 1.0) });
        assert_eq!(envelope["ok"], false);
    }
}
