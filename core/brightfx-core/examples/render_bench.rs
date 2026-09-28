//! Measures the render cost the spec budgets: 500 glowing particles at
//! 1280x800, target under 4 ms per frame in release.
//!
//! Run: `cargo run --release --example render_bench -p brightfx-core`
//!
//! Sparkle-star is the reference scene; the other shapes are there because
//! a fill-only benchmark cannot see the cost of stroked shapes (ring,
//! bubble, rune, lightning-bolt), which allocate per segment when stroked.

use brightfx_core::abi::AbiSimulation;
use brightfx_core::{BlendMode, ParticleFxConfig, ParticleShape};
use std::time::Instant;

const SHAPES: [ParticleShape; 6] = [
    ParticleShape::SparkleStar,
    ParticleShape::Circle,
    ParticleShape::GlowDisc,
    ParticleShape::Ring,
    ParticleShape::LightningBolt,
    ParticleShape::Rune,
];

fn scene(shape: ParticleShape, glow: bool) -> AbiSimulation {
    let mut config = ParticleFxConfig {
        shape,
        blend_mode: BlendMode::Lighter,
        glow_bloom: glow,
        glow_radius: 10.0,
        start_size: 6.0,
        peak_size: 9.0,
        lifetime_min: 120.0,
        lifetime_max: 120.0,
        ..Default::default()
    };
    config.emitter.spawn_burst_size = 60;

    let mut sim = AbiSimulation::new(7);
    let envelope = sim.set_config(&serde_json::to_string(&config).unwrap());
    assert!(envelope.contains("\"ok\":true"), "{envelope}");
    let envelope = sim.set_viewport(1280, 800, 1.0);
    assert!(envelope.contains("\"ok\":true"), "{envelope}");

    // Fill the pool: bursts of 60 until the 500-particle cap is reached.
    for i in 0..10 {
        sim.set_emitter(200.0 + i as f32 * 90.0, 400.0, 0.0, 0.0, true);
        sim.trigger_burst();
        sim.advance(0.015625);
    }
    assert_eq!(sim.particle_count(), 500);
    sim
}

/// Milliseconds per frame over `measured` renders after a warmup.
fn ms_per_frame(sim: &mut AbiSimulation, mut frame: impl FnMut(&mut AbiSimulation)) -> f64 {
    let warmup = 10;
    let measured = 100;
    for _ in 0..warmup {
        frame(sim);
    }
    let start = Instant::now();
    for _ in 0..measured {
        frame(sim);
    }
    (start.elapsed() / measured).as_secs_f64() * 1000.0
}

fn main() {
    println!("500 particles at 1280x800, ms per frame");
    println!("{:<14} {:>8} {:>10} {:>14}", "shape", "no glow", "glow", "glow, resizing");
    for shape in SHAPES {
        let plain = ms_per_frame(&mut scene(shape, false), |sim| sim.render());
        let steady = ms_per_frame(&mut scene(shape, true), |sim| sim.render());
        // A cache miss on every frame is the worst case: force it by cycling
        // the step so sizes keep changing.
        let mut i = 0;
        let resizing = ms_per_frame(&mut scene(shape, true), |sim| {
            sim.advance(0.015625 * (1 + i % 3) as f32);
            sim.render();
            i += 1;
        });
        println!("{:<14} {:>8.2} {:>10.2} {:>14.2}", format!("{shape:?}"), plain, steady, resizing);
    }
}
