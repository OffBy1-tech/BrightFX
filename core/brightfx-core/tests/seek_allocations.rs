//! `seek` must not allocate once the config is set. On wasm an allocation
//! can grow linear memory, which detaches every view a host holds of the
//! particle buffer, and the docs only warn about that for `set_config`.
//! The saved pre-roll is what makes this worth a test: its storage is
//! reserved by `set_config`, and filling or restoring it copies into that.
//!
//! Its own test crate, because the counting allocator is process-wide.

mod common;

use brightfx_core::abi::AbiSimulation;
use common::fixtures_dir;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Counting;

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    /// Counts only this thread's allocations while set, so the test
    /// harness's own threads cannot add to the total.
    static COUNTING: Cell<bool> = const { Cell::new(false) };
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTING.with(Cell::get) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if COUNTING.with(Cell::get) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// The pre-roll fixture with `cullMargin` 0, so bounds turn culling on.
fn culled(bounds: Option<(f32, f32)>) -> AbiSimulation {
    let path = fixtures_dir().join("ffi-preroll.config.json");
    let mut config: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    config["cullMargin"] = 0.0.into();
    let mut sim = AbiSimulation::new(42);
    let envelope: serde_json::Value = serde_json::from_str(&sim.set_config(&config.to_string())).unwrap();
    assert_eq!(envelope["ok"], true, "{envelope}");
    if let Some((w, h)) = bounds {
        sim.set_bounds(w, h);
    }
    sim
}

#[test]
fn seeking_through_and_back_into_a_preroll_does_not_allocate() {
    // The emitter runs from x=-60 to 120, so a 100 px frame culls part of
    // the pool: the culling path runs under the counter too.
    let mut sim = culled(Some((100.0, 100.0)));
    let mut open = culled(None);
    open.seek(0.25);

    COUNTING.with(|c| c.set(true));
    // The first seek runs the pre-roll and saves its end; the backward
    // ones restore it; the forward ones step on from the last seek.
    for time in [1.0f32, 0.25, 0.0, 0.5, 0.75, 0.1, 0.25] {
        sim.seek(time);
    }
    COUNTING.with(|c| c.set(false));

    assert!(sim.particle_count() > 0, "vacuous: nothing to copy");
    assert!(sim.particle_count() < open.particle_count(), "vacuous: nothing was culled");
    assert_eq!(ALLOCATIONS.load(Ordering::Relaxed), 0, "seek allocated");
}
