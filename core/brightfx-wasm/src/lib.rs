//! wasm-bindgen bindings for BrightFX. Like `brightfx-ffi`, every method is a
//! type translation around a single `brightfx_core::abi` call — no logic here.
//! The two free functions at the bottom do the same for `brightfx_tracks::json`,
//! which `brightfx-ffi` does not expose (the desktop apps bake no tracks).

use brightfx_core::abi::AbiSimulation;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct BrightFx {
    inner: AbiSimulation,
}

#[wasm_bindgen]
impl BrightFx {
    #[wasm_bindgen(constructor)]
    pub fn new(seed: u64) -> BrightFx {
        BrightFx {
            inner: AbiSimulation::new(seed),
        }
    }

    /// Loads a JSON config. Returns the JSON envelope as a string:
    /// `{"ok":true,"clamped":[...]}` or `{"ok":false,"error":"..."}`.
    ///
    /// This allocates, which can grow linear memory and detach any existing
    /// `Float32Array` view over the particle buffer. Callers must rebuild
    /// their view afterwards — see `readBuffer` in the JS helper.
    #[wasm_bindgen(js_name = setConfig)]
    pub fn set_config(&mut self, json: &str) -> String {
        self.inner.set_config(json)
    }

    #[wasm_bindgen(js_name = setEmitter)]
    pub fn set_emitter(&mut self, x: f32, y: f32, vx: f32, vy: f32, active: bool) {
        self.inner.set_emitter(x, y, vx, vy, active);
    }

    #[wasm_bindgen(js_name = triggerBurst)]
    pub fn trigger_burst(&mut self) {
        self.inner.trigger_burst();
    }

    pub fn advance(&mut self, dt: f32) {
        self.inner.advance(dt);
    }

    pub fn seek(&mut self, time: f32) {
        self.inner.seek(time);
    }

    /// Byte offset of the particle buffer within linear memory.
    ///
    /// `u32` is correct on wasm32, where pointers are 32-bit. This crate also
    /// builds as an `rlib` for the host (that is how `cargo test` compiles it),
    /// and there a 64-bit pointer would silently truncate to its low 32 bits —
    /// a plausible-looking offset that is simply wrong. Nothing calls this off
    /// wasm today, so the non-wasm arm fails loudly rather than returning a
    /// number it cannot vouch for.
    #[wasm_bindgen(js_name = bufferPtr)]
    pub fn buffer_ptr(&self) -> u32 {
        #[cfg(not(target_arch = "wasm32"))]
        {
            unreachable!(
                "bufferPtr is only meaningful on wasm32; a 64-bit host pointer \
                 cannot be represented as u32"
            )
        }
        #[cfg(target_arch = "wasm32")]
        {
            self.inner.buffer_ptr() as u32
        }
    }

    #[wasm_bindgen(js_name = particleCount)]
    pub fn particle_count(&self) -> u32 {
        self.inner.particle_count()
    }

    /// Allocates the frame. Returns the JSON envelope as a string, like
    /// `setConfig`. This allocates, so it can grow linear memory and detach
    /// existing views; rebuild them through `readFrame` / `readBuffer`.
    #[wasm_bindgen(js_name = setViewport)]
    pub fn set_viewport(&mut self, width: u32, height: u32, scale: f32) -> String {
        self.inner.set_viewport(width, height, scale)
    }

    /// Rasterizes the current particle buffer into the frame.
    pub fn render(&mut self) {
        self.inner.render();
    }

    /// Byte offset of the frame within linear memory. See `bufferPtr` for
    /// why this is `u32` and fails loudly off wasm32.
    #[wasm_bindgen(js_name = framePtr)]
    pub fn frame_ptr(&self) -> u32 {
        #[cfg(not(target_arch = "wasm32"))]
        {
            unreachable!("framePtr is only meaningful on wasm32")
        }
        #[cfg(target_arch = "wasm32")]
        {
            self.inner.frame_ptr() as u32
        }
    }

    /// Byte length of the frame: `frameWidth() * frameHeight() * 4`, or 0
    /// before a viewport is set.
    #[wasm_bindgen(js_name = frameLen)]
    pub fn frame_len(&self) -> u32 {
        self.inner.frame_len()
    }

    #[wasm_bindgen(js_name = frameWidth)]
    pub fn frame_width(&self) -> u32 {
        self.inner.frame_width()
    }

    #[wasm_bindgen(js_name = frameHeight)]
    pub fn frame_height(&self) -> u32 {
        self.inner.frame_height()
    }

    /// Whether a panic has poisoned this simulation. A poisoned simulation is
    /// inert: every operation is a no-op, `particleCount` reports 0, and
    /// `setConfig` returns an error envelope. Recover by dropping this
    /// instance and creating a new one.
    #[wasm_bindgen(js_name = isPoisoned)]
    pub fn is_poisoned(&self) -> bool {
        self.inner.is_poisoned()
    }
}

/// Floats per particle. Hosts derive their stride from this.
#[wasm_bindgen(js_name = particleFloats)]
pub fn particle_floats() -> u32 {
    AbiSimulation::PARTICLE_FLOATS
}

/// The module's `WebAssembly.Memory`. Exported explicitly so JS gets it the
/// same way regardless of which wasm-pack target was built.
///
/// Named `wasmMemory`, NOT `memory`: `wasm32-unknown-unknown` always exports a
/// cdylib's own linear memory under the literal name `memory`, so a
/// wasm-bindgen function of that name collides and the build fails with
/// "duplicate export name `memory` already defined". Renaming sidesteps it
/// entirely — the alternative, patching the generated glue after every build,
/// is a build step that must never be forgotten and breaks whenever
/// wasm-bindgen changes its output.
#[wasm_bindgen(js_name = wasmMemory)]
pub fn wasm_memory() -> JsValue {
    wasm_bindgen::memory()
}

/// `brightfx_tracks::json::fit_track_json`, verbatim. Returns
/// `{"ok":true,"config":{...}}` or `{"ok":false,"error":"..."}`.
#[wasm_bindgen(js_name = fitTrack)]
pub fn fit_track(config_json: &str, from_width: f32, from_height: f32, to_width: f32, to_height: f32) -> String {
    brightfx_tracks::json::fit_track_json(config_json, from_width, from_height, to_width, to_height)
}

/// `brightfx_tracks::json::generate_cue_tracks_json`, verbatim. Returns
/// `{"ok":true,"jobs":[...]}` or `{"ok":false,"error":"..."}`.
#[wasm_bindgen(js_name = generateCueTracks)]
pub fn generate_cue_tracks(input_json: &str) -> String {
    brightfx_tracks::json::generate_cue_tracks_json(input_json)
}
