//! Glow is the shape's silhouette, blurred, in the particle color, drawn
//! under the shape. Blur is the expensive step, so halos are cached as
//! colorless alpha sprites keyed by shape, size, and blur; tint is applied
//! at stamp time so color-cycling modes do not defeat the cache.

use std::collections::HashMap;

use super::raster::Rasterizer;
use super::shapes::Shape;
use crate::schema::ParticleShape;

/// Bound on the cache's total sprite bytes, not sprite count: sprite size
/// scales with particle size and blur, so a count bound could still let the
/// cache grow into the gigabytes at in-range config values.
pub(crate) const MAX_GLOW_CACHE_BYTES: usize = 64 * 1024 * 1024;

/// Hard cap on a sprite's half-side in device pixels, so one sprite is at
/// most 4095 x 4095 x 4 bytes, under the cache budget, and can always be
/// held. Within a clamped config at `MAX_SCALE` the largest halo needs
/// about 740, so this only binds on out-of-range input through the Rust
/// API, where the halo beyond it is lost rather than the process.
pub(crate) const MAX_SPRITE_RADIUS: f32 = 2047.0;

/// The premultiplied source alpha below which a stamped pixel cannot change
/// any byte of the frame, in any blend mode. Every channel of every mode
/// moves the destination by at most the source alpha (premultiplied color
/// is bounded by it), and a move of under half a byte rounds back to the
/// byte that was there. `0.49` rather than `0.5` leaves room for the
/// byte-to-unit-to-byte round trip's float error. Halo pixels under it are
/// never stamped, and the sprite's nonzero box excludes them, which is what
/// keeps the stamp loop from paying for the halo's invisible outer skirt.
pub(crate) const MIN_VISIBLE_ALPHA: f32 = 0.49 / 255.0;

pub(crate) struct GlowSprite {
    pub side: usize,
    /// `side * side` alpha values, row-major, zero along every edge.
    pub alpha: Vec<f32>,
    /// Inclusive row/column bounds of a single square that is a superset of
    /// the region with any `alpha >= MIN_VISIBLE_ALPHA` on both axes. The
    /// square need not be centered on the pivot (`side / 2`): asymmetric
    /// shapes leave it off-center, which is why `footprint_radius` measures
    /// reach from the pivot rather than from the box's own width.
    pub min: usize,
    pub max: usize,
    /// Inclusive bounds of `col + row` and of `col - row` over the same
    /// pixels. With `min`/`max` they bound the visible region by an octagon,
    /// which the stamp loop clips each row to; the square alone leaves the
    /// corners of a round halo to be sampled and rejected.
    pub sum: (isize, isize),
    pub diff: (isize, isize),
    /// Per sprite row, the inclusive column range of visible pixels, or
    /// `(1, 0)` for a row with none. The unrotated stamp clips each frame
    /// row to exactly this.
    pub rows: Vec<(usize, usize)>,
}

impl GlowSprite {
    /// Wraps a mask, computing the visible-region bounds.
    pub fn from_alpha(side: usize, alpha: Vec<f32>) -> Self {
        let mut min = side.saturating_sub(1);
        let mut max = 0usize;
        let mut sum = (isize::MAX, isize::MIN);
        let mut diff = (isize::MAX, isize::MIN);
        let mut rows = vec![(1usize, 0usize); side];
        for row in 0..side {
            for col in 0..side {
                if alpha[row * side + col] >= MIN_VISIBLE_ALPHA {
                    min = min.min(row).min(col);
                    max = max.max(row).max(col);
                    let (r, c) = (row as isize, col as isize);
                    sum = (sum.0.min(c + r), sum.1.max(c + r));
                    diff = (diff.0.min(c - r), diff.1.max(c - r));
                    let (first, last) = rows[row];
                    rows[row] = if first > last { (col, col) } else { (first, col) };
                }
            }
        }
        if min > max {
            // Nothing visible (e.g. a fully transparent shape): collapse to
            // the center pixel so the bounds stay well-formed.
            let center = side / 2;
            min = center;
            max = center;
            let c = center as isize;
            sum = (2 * c, 2 * c);
            diff = (0, 0);
        }
        Self { side, alpha, min, max, sum, diff, rows }
    }
}

pub(crate) struct GlowCache {
    sprites: HashMap<u64, GlowSprite>,
    /// Sum of every cached sprite's `alpha.len() * size_of::<f32>()`.
    bytes: usize,
}

impl GlowCache {
    pub fn new() -> Self {
        Self { sprites: HashMap::new(), bytes: 0 }
    }

    // Exercised by tests that verify cache reuse and the eviction bound;
    // not yet queried by production code.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.sprites.len()
    }

    #[cfg(test)]
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    fn sprite_bytes(sprite: &GlowSprite) -> usize {
        sprite.alpha.len() * std::mem::size_of::<f32>()
    }

    /// Drops every sprite if a new one of `bytes` would not fit beside
    /// them. Called before that sprite is built, so the budget bounds what
    /// is allocated, not just what is kept. The whole cache goes rather
    /// than tracking recency: a full cache means the effect is cycling
    /// through sizes, and a rebuild costs one blur.
    pub fn make_room(&mut self, bytes: usize) {
        if self.bytes + bytes > MAX_GLOW_CACHE_BYTES {
            self.clear();
        }
    }

    /// The sprite for `key`, built with `build` if absent. `bytes` is the
    /// size the build will produce (`sprite_bytes_for_side`), checked
    /// against the budget before building. Bounded by bytes, not count:
    /// sprite size scales with particle size and blur, so a fixed count
    /// could still let memory grow unbounded.
    pub fn get_or_build(&mut self, key: u64, bytes: usize, build: impl FnOnce() -> GlowSprite) -> &GlowSprite {
        if !self.sprites.contains_key(&key) {
            self.make_room(bytes);
            let sprite = build();
            debug_assert_eq!(Self::sprite_bytes(&sprite), bytes, "sprite size was mispredicted");
            self.bytes += Self::sprite_bytes(&sprite);
            self.sprites.insert(key, sprite);
        }
        &self.sprites[&key]
    }

    pub fn clear(&mut self) {
        self.sprites.clear();
        self.bytes = 0;
    }
}

pub(crate) fn sprite_key(shape: ParticleShape, size_q: u32, blur_q: u32) -> u64 {
    ((shape as u64) << 48) | ((size_q as u64 & 0xFF_FFFF) << 24) | (blur_q as u64 & 0xFF_FFFF)
}

/// Box radii for three passes approximating a Gaussian of `sigma`
/// (Kutskir's "boxes for Gauss").
pub(crate) fn box_radii(sigma: f32) -> [usize; 3] {
    if sigma <= 0.0 {
        return [0; 3];
    }
    let n = 3.0f32;
    let w_ideal = (12.0 * sigma * sigma / n + 1.0).sqrt();
    let mut wl = w_ideal.floor() as i32;
    if wl % 2 == 0 {
        wl -= 1;
    }
    let wl = wl.max(1);
    let wu = wl + 2;
    let wlf = wl as f32;
    let m_ideal = (12.0 * sigma * sigma - n * wlf * wlf - 4.0 * n * wlf - 3.0 * n) / (-4.0 * wlf - 4.0);
    let m = m_ideal.round() as i32;
    let mut out = [0usize; 3];
    for (i, r) in out.iter_mut().enumerate() {
        let size = if (i as i32) < m { wl } else { wu };
        *r = ((size - 1) / 2) as usize;
    }
    out
}

/// One box blur pass of radius `r` over a `side x side` mask, horizontal
/// then vertical. Pixels outside the mask count as zero.
pub(crate) fn box_blur(alpha: &mut [f32], scratch: &mut Vec<f32>, side: usize, r: usize) {
    if r == 0 || side == 0 {
        return;
    }
    scratch.clear();
    scratch.resize(side * side, 0.0);
    let norm = 1.0 / (2 * r + 1) as f32;
    // Horizontal: alpha -> scratch.
    for y in 0..side {
        let row = &alpha[y * side..(y + 1) * side];
        let mut sum = 0.0f32;
        for x in 0..side {
            if x == 0 {
                sum = row.iter().take(r + 1).sum();
            } else {
                if x + r < side {
                    sum += row[x + r];
                }
                if x > r {
                    sum -= row[x - r - 1];
                }
            }
            scratch[y * side + x] = sum * norm;
        }
    }
    // Vertical: scratch -> alpha.
    for x in 0..side {
        let mut sum = 0.0f32;
        for y in 0..side {
            if y == 0 {
                sum = (0..=r.min(side - 1)).map(|yy| scratch[yy * side + x]).sum();
            } else {
                if y + r < side {
                    sum += scratch[(y + r) * side + x];
                }
                if y > r {
                    sum -= scratch[(y - r - 1) * side + x];
                }
            }
            alpha[y * side + x] = sum * norm;
        }
    }
}

/// The side of the sprite `build_sprite` produces for these inputs, so its
/// memory can be budgeted before it is built. Shape extent, the blur's
/// total reach, and a zero border on each side, capped by `max_radius` and
/// by `MAX_SPRITE_RADIUS`.
pub(crate) fn sprite_side(shape: &Shape, size: f32, blur: f32, scale: f32, max_radius: f32) -> usize {
    let reach: usize = box_radii(blur * 0.5).iter().sum();
    let radius = (shape.extent * size + 2.0 * scale + reach as f32 + 2.0).min(max_radius).min(MAX_SPRITE_RADIUS);
    (radius * 2.0).ceil() as usize + 1
}

/// Bytes a sprite of `side` occupies in the cache.
pub(crate) fn sprite_bytes_for_side(side: usize) -> usize {
    side * side * std::mem::size_of::<f32>()
}

/// Rasterizes the shape unrotated at `size` device pixels, unions every
/// layer's alpha, and blurs it. `blur` is the canvas `shadowBlur` in device
/// pixels (sigma is half of it). `max_radius` caps the sprite's half-side in
/// device pixels; a halo larger than the frame can be truncated to it
/// without changing any visible pixel of a particle centered inside the
/// frame, as long as `max_radius` includes the blur's reach.
/// `MAX_SPRITE_RADIUS` caps it again for degenerate (huge size or blur)
/// inputs.
pub(crate) fn build_sprite(
    shape: &Shape,
    size: f32,
    blur: f32,
    scale: f32,
    max_radius: f32,
    raster: &mut Rasterizer,
    coverage: &mut Vec<f32>,
) -> GlowSprite {
    use super::paint::Paint;
    use super::path::Transform;

    let sigma = blur * 0.5;
    let radii = box_radii(sigma);
    let side = sprite_side(shape, size, blur, scale, max_radius);
    let center = side as f32 * 0.5;
    let mut alpha = vec![0.0f32; side * side];
    let transform = Transform::new(size, 0.0, center, center);

    for layer in shape.layers {
        raster.resize(side, side);
        super::rasterize_layer(layer, &transform, size, scale, raster);
        raster.coverage_into(coverage);
        let is_radial = matches!(layer.paint, Paint::Radial { .. });
        let radial_radius = match layer.paint {
            Paint::Radial { radius, .. } => radius * size,
            _ => 1.0,
        };
        for (i, &cov) in coverage.iter().enumerate() {
            if cov <= 0.0 {
                continue;
            }
            // Only Paint::Radial reads t; skip the sqrt otherwise.
            let t = if is_radial {
                let x = (i % side) as f32 + 0.5 - center;
                let y = (i / side) as f32 + 0.5 - center;
                (x * x + y * y).sqrt() / radial_radius
            } else {
                0.0
            };
            let la = layer.paint.alpha_at(t) * cov;
            alpha[i] += la * (1.0 - alpha[i]);
        }
    }

    let mut scratch = Vec::new();
    for r in radii {
        box_blur(&mut alpha, &mut scratch, side, r);
    }
    // The padding above guarantees the outer ring is mathematically zero;
    // three sequential running-sum blur passes leave float noise there
    // (down around 1e-7), so pin it back to the exact zero the type promises.
    for i in 0..side {
        alpha[i] = 0.0;
        alpha[(side - 1) * side + i] = 0.0;
        alpha[i * side] = 0.0;
        alpha[i * side + side - 1] = 0.0;
    }

    // Bounds of the visible alpha, so the stamp loop need not walk the full
    // (mostly-invisible) sprite footprint. One square box shared by both
    // axes rather than a per-axis box, purely to keep this cheap; the box is
    // a superset of the visible region and may sit off-center on the pivot
    // for an asymmetric shape (footprint_radius accounts for that).
    GlowSprite::from_alpha(side, alpha)
}

/// Half-side of the square, centered on the sprite's pivot, that contains
/// the nonzero box at every rotation: the pivot's reach to the box's far
/// edge, times sqrt(2) for the diagonal, plus a pixel of sampling slack.
/// The box need not be centered on the pivot; asymmetric shapes are not.
pub(crate) fn footprint_radius(sprite: &GlowSprite) -> f32 {
    let half = sprite.side as f32 * 0.5;
    let reach = (half - sprite.min as f32).max((sprite.max + 1) as f32 - half);
    reach * std::f32::consts::SQRT_2 + 1.0
}

/// The sprite-coordinate interval, on both axes, outside which `sample`
/// cannot return anything visible: a sample at `x` reads pixels
/// `floor(x - 0.5)` and the next, so it can see the visible box from half a
/// pixel before `min` up to one and a half past `max`. Padded by another
/// half pixel so a caller's float error only ever includes more.
pub(crate) fn sample_reach(sprite: &GlowSprite) -> (f32, f32) {
    (sprite.min as f32 - 1.0, sprite.max as f32 + 2.0)
}

/// The same reach along the diagonals, as intervals of `x + y` and `x - y`.
/// A sample at `(x, y)` that can see visible pixel `(col, row)` has
/// `x` in `[col - 0.5, col + 1.5)` and `y` in `[row - 0.5, row + 1.5)`, so
/// `x + y` in `[sum - 1, sum + 3)` and `x - y` in `(diff - 2, diff + 2)`,
/// each padded by half a pixel.
pub(crate) fn sample_reach_diagonals(sprite: &GlowSprite) -> ((f32, f32), (f32, f32)) {
    (
        (sprite.sum.0 as f32 - 1.5, sprite.sum.1 as f32 + 3.5),
        (sprite.diff.0 as f32 - 2.5, sprite.diff.1 as f32 + 2.5),
    )
}

/// Bilinear sample at sprite pixel coordinates; zero outside.
#[inline(always)]
pub(crate) fn sample(sprite: &GlowSprite, x: f32, y: f32) -> f32 {
    let x = x - 0.5;
    let y = y - 0.5;
    if x < 0.0 || y < 0.0 {
        return 0.0;
    }
    let x0 = x.floor();
    let y0 = y.floor();
    let (xi, yi) = (x0 as usize, y0 as usize);
    if xi + 1 >= sprite.side || yi + 1 >= sprite.side {
        return 0.0;
    }
    let fx = x - x0;
    let fy = y - y0;
    let at = |xx: usize, yy: usize| sprite.alpha[yy * sprite.side + xx];
    let top = at(xi, yi) * (1.0 - fx) + at(xi + 1, yi) * fx;
    let bottom = at(xi, yi + 1) * (1.0 - fx) + at(xi + 1, yi + 1) * fx;
    top * (1.0 - fy) + bottom * fy
}

#[cfg(test)]
mod tests {
    use super::super::shapes::shape_for;
    use super::*;

    #[test]
    fn zero_sigma_means_no_blur() {
        assert_eq!(box_radii(0.0), [0, 0, 0]);
    }

    #[test]
    fn larger_sigma_means_larger_boxes() {
        let small: usize = box_radii(1.0).iter().sum();
        let large: usize = box_radii(5.0).iter().sum();
        assert!(large > small, "{large} vs {small}");
        assert!(box_radii(5.0).iter().all(|&r| r >= 3), "{:?}", box_radii(5.0));
    }

    #[test]
    fn a_box_blur_conserves_mass_away_from_the_edges() {
        let side = 21;
        let mut alpha = vec![0.0; side * side];
        alpha[10 * side + 10] = 1.0;
        let mut scratch = Vec::new();
        box_blur(&mut alpha, &mut scratch, side, 2);
        let total: f32 = alpha.iter().sum();
        assert!((total - 1.0).abs() < 1e-4, "mass {total}");
        assert!((alpha[10 * side + 10] - 1.0 / 25.0).abs() < 1e-5, "center of a 5x5 box");
        assert!((alpha[8 * side + 12] - 1.0 / 25.0).abs() < 1e-5, "corner of the box");
        assert_eq!(alpha[7 * side + 10], 0.0, "outside the box");
    }

    #[test]
    fn a_zero_radius_pass_leaves_the_mask_alone() {
        let side = 5;
        let mut alpha = vec![0.0; side * side];
        alpha[12] = 0.7;
        let before = alpha.clone();
        box_blur(&mut alpha, &mut Vec::new(), side, 0);
        assert_eq!(alpha, before);
    }

    #[test]
    fn the_cache_reuses_a_sprite_for_the_same_key() {
        let mut cache = GlowCache::new();
        let mut builds = 0;
        for _ in 0..3 {
            cache.get_or_build(sprite_key(ParticleShape::Circle, 10, 4), 4, || {
                builds += 1;
                GlowSprite::from_alpha(1, vec![0.0])
            });
        }
        assert_eq!(builds, 1);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn the_cache_clears_when_it_reaches_its_byte_bound() {
        let mut cache = GlowCache::new();
        // Each sprite costs alpha.len() * 4 bytes; 20MiB per sprite blows
        // the 64MiB budget on the fourth insertion.
        let sprite_len = 5 * 1024 * 1024;
        let sprite_bytes = sprite_len * std::mem::size_of::<f32>();
        let build = || GlowSprite::from_alpha(1, vec![0.0; sprite_len]);
        for i in 0..3u32 {
            cache.get_or_build(sprite_key(ParticleShape::Circle, i, 0), sprite_bytes, build);
        }
        assert_eq!(cache.len(), 3);
        assert!(cache.bytes() <= MAX_GLOW_CACHE_BYTES);

        // The 4th sprite would push bytes from 60MiB to 80MiB, past the
        // 64MiB budget, so the cache clears before building it.
        cache.get_or_build(sprite_key(ParticleShape::Ring, 0, 0), sprite_bytes, build);
        assert_eq!(cache.len(), 1, "the cache clears once the byte budget is exceeded");
        assert!(cache.bytes() <= MAX_GLOW_CACHE_BYTES, "bytes {} exceed the budget", cache.bytes());
    }

    #[test]
    fn the_cache_makes_room_before_a_sprite_is_built() {
        // The budget bounds allocation, not just retention: the room is
        // made from the predicted size, before the build runs.
        let mut cache = GlowCache::new();
        let sprite_len = 5 * 1024 * 1024;
        let sprite_bytes = sprite_len * std::mem::size_of::<f32>();
        for i in 0..3u32 {
            cache.get_or_build(sprite_key(ParticleShape::Circle, i, 0), sprite_bytes, || {
                GlowSprite::from_alpha(1, vec![0.0; sprite_len])
            });
        }
        cache.make_room(sprite_bytes);
        assert_eq!(cache.len(), 0, "room is made by clearing, before anything is allocated");
        assert_eq!(cache.bytes(), 0);
    }

    #[test]
    fn a_sprite_is_capped_so_it_always_fits_the_budget() {
        // Absurd size and blur through the Rust API, with no frame to bound
        // the halo: the side is capped rather than the process aborting.
        let side = sprite_side(shape_for(ParticleShape::Circle), 1e6, 1e6, 8.0, f32::INFINITY);
        assert_eq!(side, (MAX_SPRITE_RADIUS * 2.0) as usize + 1);
        assert!(sprite_bytes_for_side(side) <= MAX_GLOW_CACHE_BYTES, "one sprite must fit the budget");
        // And the prediction is what build_sprite produces.
        let mut raster = Rasterizer::new();
        let mut coverage = Vec::new();
        let sprite = build_sprite(shape_for(ParticleShape::Circle), 40.0, 30.0, 2.0, 500.0, &mut raster, &mut coverage);
        assert_eq!(sprite.side, sprite_side(shape_for(ParticleShape::Circle), 40.0, 30.0, 2.0, 500.0));
    }

    #[test]
    fn different_shapes_sizes_and_blurs_get_different_keys() {
        let a = sprite_key(ParticleShape::Circle, 10, 4);
        assert_ne!(a, sprite_key(ParticleShape::Ring, 10, 4));
        assert_ne!(a, sprite_key(ParticleShape::Circle, 11, 4));
        assert_ne!(a, sprite_key(ParticleShape::Circle, 10, 5));
    }

    #[test]
    fn a_sprite_extends_past_the_shape_and_is_zero_at_its_edges() {
        let mut raster = Rasterizer::new();
        let mut coverage = Vec::new();
        let sprite = build_sprite(shape_for(ParticleShape::Circle), 6.0, 8.0, 1.0, f32::INFINITY, &mut raster, &mut coverage);
        let c = sprite.side / 2;
        let at = |x: usize, y: usize| sprite.alpha[y * sprite.side + x];
        assert!(at(c, c) > 0.5, "center {}", at(c, c));
        assert!(at(c + 9, c) > 0.0, "3px outside a 6px radius still glows: {}", at(c + 9, c));
        assert!(at(c + 9, c) < at(c, c), "and fades outward");
        for i in 0..sprite.side {
            assert_eq!(at(i, 0), 0.0);
            assert_eq!(at(0, i), 0.0);
            assert_eq!(at(i, sprite.side - 1), 0.0);
            assert_eq!(at(sprite.side - 1, i), 0.0);
        }
    }

    #[test]
    fn sampling_interpolates_and_is_zero_outside() {
        let sprite = GlowSprite::from_alpha(3, vec![0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0]);
        assert!((sample(&sprite, 1.5, 1.5) - 1.0).abs() < 1e-6, "pixel center");
        assert!((sample(&sprite, 1.0, 1.5) - 0.5).abs() < 1e-6, "halfway to the left neighbor");
        assert_eq!(sample(&sprite, -1.0, 1.5), 0.0);
        assert_eq!(sample(&sprite, 1.5, 10.0), 0.0);
    }

    #[test]
    fn the_nonzero_box_sits_strictly_inside_the_sprite_and_covers_the_center() {
        let mut raster = Rasterizer::new();
        let mut coverage = Vec::new();
        let sprite = build_sprite(shape_for(ParticleShape::Circle), 6.0, 8.0, 1.0, f32::INFINITY, &mut raster, &mut coverage);
        assert!(sprite.min > 0, "min {} should be inside the zero border", sprite.min);
        assert!(sprite.max < sprite.side - 1, "max {} should be inside the zero border (side {})", sprite.max, sprite.side);
        let center = sprite.side / 2;
        assert!(sprite.min <= center && center <= sprite.max, "box [{}, {}] should contain the center {center}", sprite.min, sprite.max);
    }

    #[test]
    fn the_footprint_covers_the_rotated_box_for_every_shape_and_rotation() {
        use super::super::shapes::{shape_for, ALL};
        let mut raster = Rasterizer::new();
        let mut coverage = Vec::new();
        for shape in ALL {
            let sprite = build_sprite(shape_for(shape), 9.0, 12.0, 1.0, f32::INFINITY, &mut raster, &mut coverage);
            let half = sprite.side as f32 * 0.5;
            let radius = footprint_radius(&sprite);
            // Pixel-edge corners of the nonzero box, relative to the pivot.
            let lo = sprite.min as f32 - half;
            let hi = (sprite.max + 1) as f32 - half;
            for k in 0..16 {
                let angle = k as f32 * std::f32::consts::TAU / 16.0;
                let (sin, cos) = angle.sin_cos();
                for (x, y) in [(lo, lo), (lo, hi), (hi, lo), (hi, hi)] {
                    let rx = cos * x - sin * y;
                    let ry = sin * x + cos * y;
                    assert!(
                        rx.abs() <= radius && ry.abs() <= radius,
                        "{shape:?} at {angle:.2} rad: box corner ({rx:.1}, {ry:.1}) outside footprint {radius:.1} (box {}..={}, side {})",
                        sprite.min, sprite.max, sprite.side
                    );
                }
            }
        }
    }
}
