//! Renders a fixed set of scenes and writes each frame as raw RGBA to a
//! directory, so a rendering change can be compared pixel for pixel against
//! frames recorded before it. Companion to `render_bench`: that measures
//! speed, this checks that a speedup changed nothing it should not have.
//!
//! Run: `cargo run --release --example frame_dump -p brightfx-core -- <dir>`

use brightfx_core::abi::AbiSimulation;
use brightfx_core::render::Renderer;
use brightfx_core::{BlendMode, ParticleFxConfig, ParticleInstance, ParticleShape};
use std::path::Path;

fn write(dir: &Path, name: &str, frame: &[u8]) {
    std::fs::write(dir.join(format!("{name}.rgba")), frame).unwrap();
}

fn main() {
    let dir = std::env::args().nth(1).expect("usage: frame_dump <dir>");
    let dir = Path::new(&dir);
    std::fs::create_dir_all(dir).unwrap();

    // The benchmark scene, exactly as render_bench sets it up.
    let mut config = ParticleFxConfig {
        shape: ParticleShape::SparkleStar,
        blend_mode: BlendMode::Lighter,
        glow_bloom: true,
        glow_radius: 10.0,
        start_size: 6.0,
        peak_size: 9.0,
        lifetime_min: 120.0,
        lifetime_max: 120.0,
        ..Default::default()
    };
    config.emitter.spawn_burst_size = 60;
    let mut sim = AbiSimulation::new(7);
    sim.set_config(&serde_json::to_string(&config).unwrap());
    sim.set_viewport(1280, 800, 1.0);
    for i in 0..10 {
        sim.set_emitter(200.0 + i as f32 * 90.0, 400.0, 0.0, 0.0, true);
        sim.trigger_burst();
        sim.advance(0.015625);
    }
    sim.render();
    write(dir, "bench", sim.frame_slice());

    // Every shape, six particles at assorted sizes and rotations, with glow,
    // in each blend mode, at two scales.
    let particles: Vec<ParticleInstance> = (0..6)
        .map(|i| {
            let t = i as f32;
            ParticleInstance {
                x: 30.0 + t * 25.0,
                y: 60.0 + (t * 1.3).sin() * 30.0,
                size: 3.0 + t * 4.0,
                rotation: t * 0.9,
                color: [1.0, 0.4 + t * 0.1, 0.2, 0.55 + t * 0.09],
            }
        })
        .collect();
    for shape in ParticleShape::ALL {
        for (mode_name, mode) in [
            ("over", BlendMode::SourceOver),
            ("lighter", BlendMode::Lighter),
            ("screen", BlendMode::Screen),
            ("dodge", BlendMode::ColorDodge),
        ] {
            for scale in [1.0f32, 2.0] {
                let cfg = ParticleFxConfig { shape, blend_mode: mode, glow_bloom: true, glow_radius: 10.0, ..Default::default() };
                let mut r = Renderer::new();
                r.set_viewport((200.0 * scale) as u32, (120.0 * scale) as u32, scale).unwrap();
                // Two renders: the second stamps from cached sprites.
                r.render(&particles, &cfg);
                r.render(&particles, &cfg);
                write(dir, &format!("{shape:?}-{mode_name}-x{scale}"), r.frame());
            }
        }
    }
    println!("wrote frames to {}", dir.display());
}
