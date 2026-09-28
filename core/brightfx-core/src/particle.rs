use std::collections::VecDeque;

/// Hard cap on live particles, matching Mouseflare's existing canvas
/// renderer (`maxParticles = 500`). When full, the oldest particle is
/// dropped to make room (FIFO), same as Mouseflare's `particles.shift()`.
pub(crate) const MAX_PARTICLES: usize = 500;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Particle {
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
    pub size: f32,
    pub start_size: f32,
    pub peak_size: f32,
    pub end_size: f32,
    pub alpha: f32,
    pub start_alpha: f32,
    pub peak_alpha: f32,
    pub end_alpha: f32,
    /// Base color captured at spawn time (normalized 0..1 RGB), used as-is
    /// for `ColorMode::Single` / `ColorMode::MultiPalette` (without stops)
    /// and `ColorMode::RandomPalette` (the stop it was dealt).
    pub color_rgb: [f32; 3],
    pub hue: f32,
    pub life: f32,
    pub max_life: f32,
    pub rotation: f32,
    pub rotation_speed: f32,
    pub turbulence_seed: f32,
}

pub(crate) struct ParticlePool {
    particles: VecDeque<Particle>,
    capacity: usize,
}

impl ParticlePool {
    pub fn new(capacity: usize) -> Self {
        Self {
            particles: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    pub fn spawn(&mut self, p: Particle) {
        if self.capacity == 0 {
            return; // zero-capacity pool never holds particles
        }
        if self.particles.len() >= self.capacity {
            self.particles.pop_front();
        }
        self.particles.push_back(p);
    }

    pub fn len(&self) -> usize {
        self.particles.len()
    }

    pub fn clear(&mut self) {
        self.particles.clear();
    }

    pub fn iter(&self) -> impl Iterator<Item = &Particle> {
        self.particles.iter()
    }

    pub fn retain_mut(&mut self, f: impl FnMut(&mut Particle) -> bool) {
        self.particles.retain_mut(f);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marker(x: f32) -> Particle {
        Particle {
            x,
            y: 0.0,
            vx: 0.0,
            vy: 0.0,
            size: 1.0,
            start_size: 1.0,
            peak_size: 1.0,
            end_size: 1.0,
            alpha: 1.0,
            start_alpha: 1.0,
            peak_alpha: 1.0,
            end_alpha: 1.0,
            color_rgb: [1.0, 1.0, 1.0],
            hue: 0.0,
            life: 0.0,
            max_life: 100.0,
            rotation: 0.0,
            rotation_speed: 0.0,
            turbulence_seed: 0.0,
        }
    }

    #[test]
    fn spawn_increments_len_up_to_capacity() {
        let mut pool = ParticlePool::new(3);
        pool.spawn(marker(0.0));
        pool.spawn(marker(1.0));
        pool.spawn(marker(2.0));
        assert_eq!(pool.len(), 3);
    }

    #[test]
    fn spawn_beyond_capacity_drops_oldest_fifo() {
        let mut pool = ParticlePool::new(3);
        pool.spawn(marker(0.0));
        pool.spawn(marker(1.0));
        pool.spawn(marker(2.0));
        pool.spawn(marker(3.0)); // should evict x=0.0
        assert_eq!(pool.len(), 3);
        let xs: Vec<f32> = pool.iter().map(|p| p.x).collect();
        assert_eq!(xs, vec![1.0, 2.0, 3.0]);
    }

    #[test]
    fn retain_mut_drops_particles_that_return_false() {
        let mut pool = ParticlePool::new(10);
        for i in 0..5 {
            pool.spawn(marker(i as f32));
        }
        pool.retain_mut(|p| p.x >= 2.0);
        assert_eq!(pool.len(), 3);
        let xs: Vec<f32> = pool.iter().map(|p| p.x).collect();
        assert_eq!(xs, vec![2.0, 3.0, 4.0]);
    }

    #[test]
    fn clear_empties_the_pool() {
        let mut pool = ParticlePool::new(10);
        pool.spawn(marker(0.0));
        pool.clear();
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn spawn_into_zero_capacity_pool_stays_empty() {
        let mut pool = ParticlePool::new(0);
        pool.spawn(marker(0.0));
        assert_eq!(pool.len(), 0, "zero-capacity pool should not hold particles");
        pool.spawn(marker(1.0));
        assert_eq!(pool.len(), 0, "zero-capacity pool should remain empty after multiple spawns");
        pool.spawn(marker(2.0));
        assert_eq!(pool.len(), 0, "zero-capacity pool should always stay empty");
    }
}
