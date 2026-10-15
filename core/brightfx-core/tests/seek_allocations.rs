//! `seek` must not allocate once the config is set. On wasm an allocation
//! can grow linear memory, which detaches every view a host holds of the
//! particle buffer, and the docs only warn about that for `set_config`.
//! The saved pre-roll is what makes this worth a test: its storage is
//! reserved by `set_config`, and filling or restoring it copies into that.
//!
//! Its own test crate, because the counting allocator is process-wide.

mod common;

use common::seek::load_sim_for;
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

#[test]
fn seeking_through_and_back_into_a_preroll_does_not_allocate() {
    let mut sim = load_sim_for("ffi-preroll.config.json", 42);
    sim.set_bounds(200.0, 200.0);

    COUNTING.with(|c| c.set(true));
    // The first seek runs the pre-roll and saves its end; the backward
    // ones restore it; the forward ones step on from the last seek.
    for time in [1.0f32, 0.25, 0.0, 0.5, 0.75, 0.1] {
        sim.seek(time);
    }
    COUNTING.with(|c| c.set(false));

    assert!(sim.particle_count() > 0, "vacuous: nothing to copy");
    assert_eq!(ALLOCATIONS.load(Ordering::Relaxed), 0, "seek allocated");
}
