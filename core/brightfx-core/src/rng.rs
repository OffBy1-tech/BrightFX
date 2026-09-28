/// A small deterministic PRNG (SplitMix64) used instead of a platform RNG so
/// that a given seed always produces the same particle sequence — required
/// for reproducible timeline scrubbing and golden-output tests.
pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }

    /// Returns a value in [0.0, 1.0).
    pub fn f32(&mut self) -> f32 {
        ((self.next_u64() >> 40) as f32) / (1u64 << 24) as f32
    }

    /// Returns a value in [min, max). If min == max, always returns min.
    pub fn range(&mut self, min: f32, max: f32) -> f32 {
        min + self.f32() * (max - min)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_produces_same_sequence() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        for _ in 0..20 {
            assert_eq!(a.f32(), b.f32());
        }
    }

    #[test]
    fn different_seeds_diverge() {
        let mut a = Rng::new(1);
        let mut b = Rng::new(2);
        let seq_a: Vec<f32> = (0..10).map(|_| a.f32()).collect();
        let seq_b: Vec<f32> = (0..10).map(|_| b.f32()).collect();
        assert_ne!(seq_a, seq_b);
    }

    #[test]
    fn f32_stays_in_unit_range() {
        let mut rng = Rng::new(7);
        for _ in 0..1000 {
            let v = rng.f32();
            assert!((0.0..1.0).contains(&v), "value out of range: {v}");
        }
    }

    #[test]
    fn range_collapses_when_min_equals_max() {
        let mut rng = Rng::new(99);
        for _ in 0..50 {
            assert_eq!(rng.range(5.0, 5.0), 5.0);
        }
    }

    #[test]
    fn range_stays_within_bounds() {
        let mut rng = Rng::new(123);
        for _ in 0..1000 {
            let v = rng.range(-3.0, 3.0);
            assert!((-3.0..3.0).contains(&v), "value out of range: {v}");
        }
    }
}
