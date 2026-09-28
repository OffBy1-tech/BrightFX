//! Rasterizes the particle buffer into a premultiplied RGBA8 frame.
//!
//! Hosts blit; nothing platform-specific lives here.

mod blend;
mod raster;
mod path;
mod paint;
mod shapes;
mod glow;

use crate::schema::{BlendMode, ParticleFxConfig};
use crate::simulation::ParticleInstance;
use path::Transform;
use raster::Rasterizer;
use shapes::{Layer, Op, Shape};

/// Largest viewport side in device pixels. Bounds the frame allocation.
pub const MAX_VIEWPORT_SIDE: u32 = 8192;
pub const MIN_SCALE: f32 = 0.25;
pub const MAX_SCALE: f32 = 8.0;
/// Cap on glow blur in logical pixels, matching all three Mouseflare renderers.
pub const MAX_GLOW_BLUR: f32 = 20.0;
pub const BYTES_PER_PIXEL: usize = 4;

/// Canvas `shadowBlur` in logical pixels for a particle of `size`.
pub fn glow_blur_logical(glow_radius: f32, size: f32) -> f32 {
    (glow_radius * size / 6.0).clamp(0.0, MAX_GLOW_BLUR)
}

/// Largest deviation from a true curve, in device pixels.
const FLATTEN_TOLERANCE: f32 = 0.25;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    pub width: u32,
    pub height: u32,
    /// Device pixels per logical unit.
    pub scale: f32,
}

pub struct Renderer {
    viewport: Option<Viewport>,
    /// Premultiplied RGBA8, `width * height * 4` bytes. Reallocated only by
    /// `set_viewport`, so its address is stable between viewport changes.
    frame: Vec<u8>,
    /// Scratch rasterizer, reused across particles so the accumulation and
    /// coverage buffers are not reallocated per particle. Flattening and
    /// stroking still allocate per layer; the benchmark task decides whether
    /// that matters.
    raster: Rasterizer,
    coverage: Vec<f32>,
    glow: glow::GlowCache,
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

impl Renderer {
    pub fn new() -> Self {
        Self { viewport: None, frame: Vec::new(), raster: Rasterizer::new(), coverage: Vec::new(), glow: glow::GlowCache::new() }
    }

    /// Allocates the frame. Returns the clamped field names, or an error for
    /// a zero dimension, a non-finite scale, or a frame the allocator will
    /// not give (reported rather than aborting, so a host sees it in the
    /// envelope). On error the previous viewport and frame stay as they were.
    pub fn set_viewport(
        &mut self,
        width: u32,
        height: u32,
        scale: f32,
    ) -> Result<Vec<&'static str>, &'static str> {
        if width == 0 || height == 0 {
            return Err("viewport dimensions must be nonzero");
        }
        if !scale.is_finite() {
            return Err("viewport scale must be finite");
        }
        let mut clamped = Vec::new();
        let w = width.min(MAX_VIEWPORT_SIDE);
        if w != width {
            clamped.push("viewport.width");
        }
        let h = height.min(MAX_VIEWPORT_SIDE);
        if h != height {
            clamped.push("viewport.height");
        }
        let s = scale.clamp(MIN_SCALE, MAX_SCALE);
        if s != scale {
            clamped.push("viewport.scale");
        }
        let len = w as usize * h as usize * BYTES_PER_PIXEL;
        let mut frame = Vec::new();
        frame.try_reserve_exact(len).map_err(|_| "viewport frame could not be allocated")?;
        frame.resize(len, 0);
        self.viewport = Some(Viewport { width: w, height: h, scale: s });
        self.frame = frame;
        // Glow sprites bake the scale-dependent stroke minimum in, and the
        // cache key does not carry the scale, so start fresh.
        self.glow.clear();
        Ok(clamped)
    }

    pub fn viewport(&self) -> Option<Viewport> {
        self.viewport
    }

    /// The frame bytes; empty until a viewport is set.
    pub fn frame(&self) -> &[u8] {
        &self.frame
    }

    pub fn frame_width(&self) -> u32 {
        self.viewport.map_or(0, |v| v.width)
    }

    pub fn frame_height(&self) -> u32 {
        self.viewport.map_or(0, |v| v.height)
    }
}

/// One particle in device pixels.
struct Placement {
    cx: f32,
    cy: f32,
    size: f32,
    rotation: f32,
}

/// Next `[start, end)` run of positive coverage in `cov_row` at or after
/// `from`, or `None` when the row has no more.
fn next_run(cov_row: &[f32], from: usize) -> Option<(usize, usize)> {
    let len = cov_row.len();
    let mut tx = from;
    while tx < len {
        // NaN fails both this test and the run test below; without the
        // negation it would never advance.
        #[allow(clippy::neg_cmp_op_on_partial_ord)]
        if !(cov_row[tx] > 0.0) {
            tx += 1;
            continue;
        }
        let start = tx;
        while tx < len && cov_row[tx] > 0.0 {
            tx += 1;
        }
        return Some((start, tx));
    }
    None
}

/// The `dx` interval where `a * dx + b` lies in `[lo, hi]`: the whole line
/// when `a` is negligible and `b` already does, nothing when it does not.
fn axis_span(a: f32, b: f32, lo: f32, hi: f32) -> Option<(f32, f32)> {
    if a.abs() < 1e-6 {
        return (lo <= b && b <= hi).then_some((f32::NEG_INFINITY, f32::INFINITY));
    }
    let (p, q) = ((lo - b) / a, (hi - b) / a);
    Some(if p <= q { (p, q) } else { (q, p) })
}

/// Pixel bounds of the square of half-side `radius` around `(cx, cy)`,
/// clipped to the frame. `None` when nothing is inside.
fn tile_bounds(vp: &Viewport, cx: f32, cy: f32, radius: f32) -> Option<(usize, usize, usize, usize)> {
    let x0 = (cx - radius).floor().max(0.0);
    let y0 = (cy - radius).floor().max(0.0);
    let x1 = (cx + radius).ceil().min(vp.width as f32);
    let y1 = (cy + radius).ceil().min(vp.height as f32);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some((x0 as usize, y0 as usize, x1 as usize, y1 as usize))
}

/// Rasterizes one layer's coverage into `raster`, which must already be
/// sized to the tile. `size` is the particle size in device pixels;
/// `scale` converts logical minimum stroke widths.
pub(super) fn rasterize_layer(layer: &Layer, transform: &Transform, size: f32, scale: f32, raster: &mut Rasterizer) {
    let contours = path::flatten(&layer.geometry, transform, FLATTEN_TOLERANCE);
    match layer.op {
        Op::Fill => {
            for c in &contours {
                raster.add_polygon(&c.points);
            }
        }
        Op::Stroke { width, min_width, round_caps } => {
            let w = (width * size).max(min_width * scale);
            for c in &contours {
                raster.add_stroke(c, w, round_caps, FLATTEN_TOLERANCE);
            }
        }
    }
}

impl Renderer {
    /// Clears the frame and draws every particle in buffer order. A no-op
    /// until a viewport is set.
    pub fn render(&mut self, particles: &[ParticleInstance], config: &ParticleFxConfig) {
        let Some(vp) = self.viewport else { return };
        self.frame.fill(0);
        let shape = shapes::shape_for(config.shape);
        for p in particles {
            let alpha = p.color[3];
            let valid = !(alpha.is_nan() || alpha <= 0.0)
                && p.x.is_finite()
                && p.y.is_finite()
                && p.size.is_finite()
                && p.size > 0.0
                && p.rotation.is_finite()
                && p.color[0].is_finite()
                && p.color[1].is_finite()
                && p.color[2].is_finite();
            if !valid {
                continue;
            }
            // Exactly zero means no particle; any positive size is floored to
            // the shape's canvas minimum. Keep the raw check above separate.
            let size = p.size.max(shape.min_size);
            let placement = Placement {
                cx: p.x * vp.scale,
                cy: p.y * vp.scale,
                size: size * vp.scale,
                rotation: p.rotation,
            };
            let tint = [p.color[0], p.color[1], p.color[2]];
            if config.glow_bloom && config.glow_radius > 0.0 {
                let blur = glow_blur_logical(config.glow_radius, size) * vp.scale;
                if blur > 0.0 {
                    self.draw_glow(config.shape, shape, &placement, tint, alpha, config.blend_mode, blur, vp);
                }
            }
            self.draw_shape(shape, &placement, tint, alpha, config.blend_mode, vp);
        }
    }

    fn draw_shape(&mut self, shape: &Shape, pl: &Placement, tint: [f32; 3], alpha: f32, mode: BlendMode, vp: Viewport) {
        // Extent plus the logical-pixel stroke minimum plus two pixels of
        // anti-aliasing slack.
        let radius = shape.extent * pl.size + 2.0 * vp.scale + 2.0;
        let Some((x0, y0, x1, y1)) = tile_bounds(&vp, pl.cx, pl.cy, radius) else { return };
        let (tw, th) = (x1 - x0, y1 - y0);
        let origin = [pl.cx - x0 as f32, pl.cy - y0 as f32];
        let transform = Transform::new(pl.size, pl.rotation, origin[0], origin[1]);
        let width = vp.width as usize;
        let (pixels, _) = self.frame.as_chunks_mut::<BYTES_PER_PIXEL>();

        for layer in shape.layers {
            self.raster.resize(tw, th);
            rasterize_layer(layer, &transform, pl.size, vp.scale, &mut self.raster);
            self.raster.coverage_into(&mut self.coverage);
            let tile = Tile { coverage: &self.coverage, tw, th, x0, y0, origin };
            blend::dispatch_blend!(mode, B => paint_layer::<B>(pixels, width, &tile, layer, pl.size, tint, alpha));
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_glow(
        &mut self,
        shape_id: crate::schema::ParticleShape,
        shape: &Shape,
        pl: &Placement,
        tint: [f32; 3],
        alpha: f32,
        mode: BlendMode,
        blur: f32,
        vp: Viewport,
    ) {
        // Half-pixel quantization keeps the cache small without visible steps.
        let size_q = (pl.size * 2.0).round() as u32;
        let blur_q = (blur * 2.0).round() as u32;
        let key = glow::sprite_key(shape_id, size_q, blur_q);
        // A halo larger than the frame diagonal (plus the blur's own reach)
        // can be truncated to it without changing any visible pixel of a
        // particle centered inside the frame. The reach must come from the
        // same quantized blur the sprite is built with, or the slack below
        // is spent covering the rounding difference.
        let reach_estimate = glow::box_radii(blur_q as f32 / 2.0 * 0.5).iter().sum::<usize>() as f32;
        let max_radius = (vp.width as f32).hypot(vp.height as f32) + reach_estimate + 2.0;
        let (size, blur) = (size_q as f32 / 2.0, blur_q as f32 / 2.0);
        let bytes = glow::sprite_bytes_for_side(glow::sprite_side(shape, size, blur, vp.scale, max_radius));
        let Self { glow, raster, coverage, frame, .. } = self;
        let sprite = glow.get_or_build(key, bytes, || {
            glow::build_sprite(shape, size, blur, vp.scale, max_radius, raster, coverage)
        });

        // Sized from the pivot's reach to the box's far edge, not the box's
        // own width, since asymmetric shapes leave the box off-center.
        let footprint = glow::footprint_radius(sprite);
        let Some((x0, y0, x1, y1)) = tile_bounds(&vp, pl.cx, pl.cy, footprint) else { return };
        let (sin, cos) = pl.rotation.sin_cos();
        let width = vp.width as usize;
        let (pixels, _) = frame.as_chunks_mut::<BYTES_PER_PIXEL>();
        // Any tint channel above one would break the visibility bound in
        // the stamp loop; colors are in range, but the guard costs nothing.
        let visible = glow::MIN_VISIBLE_ALPHA / tint[0].max(tint[1]).max(tint[2]).max(1.0);
        let stamp = GlowStamp { sprite, cx: pl.cx, cy: pl.cy, sin, cos, tint, alpha, visible, x0, y0, x1, y1 };
        if shape.symmetric {
            blend::dispatch_blend!(mode, B => stamp_glow_unrotated::<B>(pixels, width, &stamp));
        } else {
            blend::dispatch_blend!(mode, B => stamp_glow::<B>(pixels, width, &stamp));
        }
    }
}

/// `stamp_glow` for a halo that is the same at every rotation, so the
/// rotation is ignored. With the sprite axis-aligned, every pixel of a frame
/// row reads the same two sprite rows with the same vertical weight, and
/// successive pixels read successive columns with the same horizontal
/// weight, so the sample is a fixed lerp of two adjacent columns in each
/// of two rows, and each frame row is clipped to exactly the sprite rows'
/// visible extent. This does not reproduce `stamp_glow` bit for bit (it
/// resamples the same halo on a different grid); it is used only where the
/// difference has no meaning.
fn stamp_glow_unrotated<B: blend::Blender>(pixels: &mut [[u8; BYTES_PER_PIXEL]], width: usize, st: &GlowStamp) {
    let GlowStamp { sprite, cx, cy, tint, alpha, visible, x0, y0, x1, y1, .. } = *st;
    let side = sprite.side;
    let half = side as f32 * 0.5;
    // Frame pixel (px, py) has its center at (px + 0.5, py + 0.5); in sprite
    // space that is (px + 0.5 - cx + half, ...), and `sample` reads pixels
    // from half a pixel before that, so the sampled sprite position is
    // (px + ox, py + oy) with the halves cancelled.
    let (ox, oy) = (half - cx, half - cy);
    for py in y0..y1 {
        let y = py as f32 + oy;
        if y < 0.0 {
            continue;
        }
        let yi = y.floor() as usize;
        if yi + 1 >= side {
            break;
        }
        let fy = y - yi as f32;
        let row0 = &sprite.alpha[yi * side..(yi + 1) * side];
        let row1 = &sprite.alpha[(yi + 1) * side..(yi + 2) * side];
        // Columns either row has visible pixels in; a sample reads `xi`
        // and `xi + 1`, so `xi` from one before the first to the last.
        let (a0, a1) = sprite.rows[yi];
        let (b0, b1) = sprite.rows[yi + 1];
        let (first, last) = match (a0 <= a1, b0 <= b1) {
            (true, true) => (a0.min(b0), a1.max(b1)),
            (true, false) => (a0, a1),
            (false, true) => (b0, b1),
            (false, false) => continue,
        };
        let xi_lo = first.saturating_sub(1);
        let xi_hi = last.min(side - 2);
        // Frame pixel px samples at x = px + ox, so xi = floor(x). Walk the
        // px whose xi lies in [xi_lo, xi_hi], within the tile.
        let px_lo = ((xi_lo as f32 - ox).ceil().max(x0 as f32)) as usize;
        let px_hi = ((xi_hi as f32 + 1.0 - ox).ceil().min(x1 as f32)) as usize;
        if px_hi <= px_lo {
            continue;
        }
        let x = px_lo as f32 + ox;
        let xi = x.floor() as usize;
        let fx = x - xi as f32;
        if xi < xi_lo || xi > xi_hi {
            continue;
        }
        let count = (px_hi - px_lo).min(xi_hi + 1 - xi);
        let (wx0, wx1, wy0, wy1) = (1.0 - fx, fx, 1.0 - fy, fy);
        let row_base = py * width;
        let top = row0[xi..xi + count + 1].windows(2);
        let bottom = row1[xi..xi + count + 1].windows(2);
        for (i, (t, b)) in top.zip(bottom).enumerate() {
            let a = (t[0] * wx0 + t[1] * wx1) * wy0 + (b[0] * wx0 + b[1] * wx1) * wy1;
            let k = a * alpha;
            if k < visible {
                continue;
            }
            let src = [tint[0] * k, tint[1] * k, tint[2] * k, k];
            blend::composite_pixel::<B>(src, &mut pixels[row_base + px_lo + i]);
        }
    }
}

/// One layer's rasterized coverage and where its tile sits in the frame.
struct Tile<'a> {
    coverage: &'a [f32],
    tw: usize,
    th: usize,
    x0: usize,
    y0: usize,
    /// The particle center in tile coordinates.
    origin: [f32; 2],
}

/// Composites one layer's coverage into the frame. Generic over the blend
/// mode so the per-pixel path has no match in it.
fn paint_layer<B: blend::Blender>(
    pixels: &mut [[u8; BYTES_PER_PIXEL]],
    width: usize,
    tile: &Tile,
    layer: &Layer,
    size: f32,
    tint: [f32; 3],
    alpha: f32,
) {
    let Tile { coverage, tw, th, x0, y0, origin } = *tile;
    let is_radial = matches!(layer.paint, paint::Paint::Radial { .. });
    let radial_radius = match layer.paint {
        paint::Paint::Radial { radius, .. } => radius * size,
        _ => 1.0,
    };
    for ty in 0..th {
        // Hoist the row-invariant frame offset out of the per-pixel path,
        // and skip runs of zero coverage (the sprite's corners) without
        // touching the frame or converting bytes.
        let row_base = (y0 + ty) * width + x0;
        let cov_row = &coverage[ty * tw..ty * tw + tw];
        let mut tx = 0;
        while let Some((run_start, run_end)) = next_run(cov_row, tx) {
            tx = run_end;
            let dy = ty as f32 + 0.5 - origin[1];
            for (i, &cov) in cov_row[run_start..run_end].iter().enumerate() {
                let txi = run_start + i;
                // Only Paint::Radial reads t; skip the sqrt otherwise.
                let t = if is_radial {
                    let dx = txi as f32 + 0.5 - origin[0];
                    (dx * dx + dy * dy).sqrt() / radial_radius
                } else {
                    0.0
                };
                let c = layer.paint.color_at(tint, t);
                let k = cov * alpha;
                let src = [c[0] * k, c[1] * k, c[2] * k, c[3] * k];
                blend::composite_pixel::<B>(src, &mut pixels[row_base + txi]);
            }
        }
    }
}

/// One particle's halo, ready to stamp: the sprite, where and how it is
/// placed, and the frame rows and columns it may touch.
struct GlowStamp<'a> {
    sprite: &'a glow::GlowSprite,
    cx: f32,
    cy: f32,
    sin: f32,
    cos: f32,
    tint: [f32; 3],
    alpha: f32,
    /// Sprite alpha times particle alpha below which a pixel is skipped;
    /// see `glow::MIN_VISIBLE_ALPHA`.
    visible: f32,
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
}

/// Stamps a halo into the frame. Generic over the blend mode so the
/// per-pixel path has no match in it.
fn stamp_glow<B: blend::Blender>(pixels: &mut [[u8; BYTES_PER_PIXEL]], width: usize, st: &GlowStamp) {
    let GlowStamp { sprite, cx, cy, sin, cos, tint, alpha, visible, x0, y0, x1, y1 } = *st;
    let half = sprite.side as f32 * 0.5;
    // The tile is the square around the rotated visible box, most of which
    // is outside the halo. Rather than walk and reject those pixels, each
    // row is clipped to where it crosses the octagon that bounds the
    // visible region (the box plus its diagonal bounds).
    let (lo, hi) = glow::sample_reach(sprite);
    let ((sum_lo, sum_hi), (diff_lo, diff_hi)) = glow::sample_reach_diagonals(sprite);
    for py in y0..y1 {
        let dy = py as f32 + 0.5 - cy;
        // Pixel px has dx = px + 0.5 - cx, and in sprite space
        //   sx = cos*dx + (sin*dy + half),  sy = -sin*dx + (cos*dy + half),
        // so each bound is linear in dx. Keep the dx where all four hold.
        let (bx, by) = (sin * dy + half, cos * dy + half);
        let mut span = (f32::NEG_INFINITY, f32::INFINITY);
        for (a, b, l, h) in [
            (cos, bx, lo, hi),
            (-sin, by, lo, hi),
            (cos - sin, bx + by, sum_lo, sum_hi),
            (cos + sin, bx - by, diff_lo, diff_hi),
        ] {
            let Some((s0, s1)) = axis_span(a, b, l, h) else { span = (1.0, 0.0); break };
            span = (span.0.max(s0), span.1.min(s1));
        }
        let (dlo, dhi) = span;
        if dhi < dlo {
            continue;
        }
        // A pixel of slack each side so float error can only add pixels,
        // which the visibility test then rejects as before.
        let px_start = ((dlo + cx - 0.5).floor() as isize - 1).max(x0 as isize) as usize;
        let px_end = ((dhi + cx - 0.5).ceil() as isize + 2).min(x1 as isize).max(px_start as isize) as usize;
        let row_base = py * width;
        for px in px_start..px_end {
            let dx = px as f32 + 0.5 - cx;
            // Inverse rotation into sprite space. Computed per pixel, in
            // this association: stepping by (cos, -sin) along the row or
            // hoisting the row terms changes the rounding by an ulp, which
            // shows up as off-by-one bytes.
            let sx = cos * dx + sin * dy + half;
            let sy = -sin * dx + cos * dy + half;
            let a = glow::sample(sprite, sx, sy);
            let k = a * alpha;
            // Exact, not a shortcut: below this the composite could not
            // change a byte, so skipping it leaves the frame identical.
            if k < visible {
                continue;
            }
            let src = [tint[0] * k, tint[1] * k, tint[2] * k, k];
            blend::composite_pixel::<B>(src, &mut pixels[row_base + px]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_run_skips_nan_and_zero_and_finds_positive_runs() {
        let row = [0.0, f32::NAN, 0.5, 0.7, 0.0, 0.2];
        assert_eq!(next_run(&row, 0), Some((2, 4)));
        assert_eq!(next_run(&row, 4), Some((5, 6)));
        assert_eq!(next_run(&row, 6), None);
    }

    #[test]
    fn next_run_is_none_for_an_all_nan_row() {
        let row = [f32::NAN, f32::NAN, f32::NAN];
        assert_eq!(next_run(&row, 0), None);
    }

    #[test]
    fn a_new_renderer_has_no_frame() {
        let r = Renderer::new();
        assert!(r.viewport().is_none());
        assert!(r.frame().is_empty());
        assert_eq!(r.frame_width(), 0);
        assert_eq!(r.frame_height(), 0);
    }

    #[test]
    fn set_viewport_allocates_width_times_height_times_four_bytes() {
        let mut r = Renderer::new();
        let clamped = r.set_viewport(64, 32, 1.0).unwrap();
        assert!(clamped.is_empty());
        assert_eq!(r.frame().len(), 64 * 32 * 4);
        assert_eq!(r.frame_width(), 64);
        assert_eq!(r.frame_height(), 32);
        assert_eq!(r.viewport(), Some(Viewport { width: 64, height: 32, scale: 1.0 }));
    }

    #[test]
    fn a_zero_dimension_is_an_error() {
        let mut r = Renderer::new();
        assert!(r.set_viewport(0, 10, 1.0).is_err());
        assert!(r.set_viewport(10, 0, 1.0).is_err());
        assert!(r.frame().is_empty(), "a failed set_viewport must not allocate");
    }

    #[test]
    fn a_non_finite_scale_is_an_error() {
        let mut r = Renderer::new();
        assert!(r.set_viewport(10, 10, f32::NAN).is_err());
        assert!(r.set_viewport(10, 10, f32::INFINITY).is_err());
    }

    #[test]
    fn oversized_dimensions_and_scale_are_clamped_and_reported() {
        let mut r = Renderer::new();
        let clamped = r.set_viewport(100_000, 100_000, 99.0).unwrap();
        assert_eq!(clamped, vec!["viewport.width", "viewport.height", "viewport.scale"]);
        let v = r.viewport().unwrap();
        assert_eq!(v.width, MAX_VIEWPORT_SIDE);
        assert_eq!(v.height, MAX_VIEWPORT_SIDE);
        assert_eq!(v.scale, MAX_SCALE);
    }

    #[test]
    fn a_tiny_scale_is_raised_to_the_minimum() {
        let mut r = Renderer::new();
        let clamped = r.set_viewport(10, 10, 0.01).unwrap();
        assert_eq!(clamped, vec!["viewport.scale"]);
        assert_eq!(r.viewport().unwrap().scale, MIN_SCALE);
    }

    #[test]
    fn set_viewport_clears_the_frame() {
        let mut r = Renderer::new();
        r.set_viewport(4, 4, 1.0).unwrap();
        r.frame[0] = 200;
        r.set_viewport(4, 4, 1.0).unwrap();
        assert!(r.frame().iter().all(|&b| b == 0));
    }

    use crate::schema::{BlendMode, ParticleFxConfig, ParticleShape};
    use crate::simulation::ParticleInstance;

    fn particle(x: f32, y: f32, size: f32, rotation: f32, color: [f32; 4]) -> ParticleInstance {
        ParticleInstance { x, y, size, rotation, color }
    }

    fn config(shape: ParticleShape, blend: BlendMode) -> ParticleFxConfig {
        ParticleFxConfig { shape, blend_mode: blend, glow_bloom: false, ..Default::default() }
    }

    fn render(shape: ParticleShape, blend: BlendMode, particles: &[ParticleInstance], w: u32, h: u32, scale: f32) -> Vec<u8> {
        let mut r = Renderer::new();
        r.set_viewport(w, h, scale).unwrap();
        r.render(particles, &config(shape, blend));
        r.frame().to_vec()
    }

    fn pixel(frame: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * w + x) * 4) as usize;
        [frame[i], frame[i + 1], frame[i + 2], frame[i + 3]]
    }

    const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];

    #[test]
    fn a_circle_paints_its_center_opaque_and_leaves_the_outside_clear() {
        let f = render(ParticleShape::Circle, BlendMode::SourceOver, &[particle(20.0, 20.0, 5.0, 0.0, RED)], 40, 40, 1.0);
        assert_eq!(pixel(&f, 40, 20, 20), [255, 0, 0, 255]);
        assert_eq!(pixel(&f, 40, 20, 27), [0, 0, 0, 0], "7px out of a 5px radius");
        assert_eq!(pixel(&f, 40, 2, 2), [0, 0, 0, 0]);
    }

    #[test]
    fn render_clears_the_previous_frame() {
        let mut r = Renderer::new();
        r.set_viewport(40, 40, 1.0).unwrap();
        let cfg = config(ParticleShape::Circle, BlendMode::SourceOver);
        r.render(&[particle(20.0, 20.0, 5.0, 0.0, RED)], &cfg);
        r.render(&[], &cfg);
        assert!(r.frame().iter().all(|&b| b == 0));
    }

    #[test]
    fn particles_with_non_finite_color_or_rotation_are_skipped_and_do_not_erase_neighbors() {
        let cfg = glow_config(ParticleShape::Circle, 10.0);
        let good = particle(30.0, 30.0, 6.0, 0.0, [1.0, 1.0, 1.0, 1.0]);
        let reference = render_with(&cfg, &[good], 60, 60, 1.0);
        for bad in [
            particle(30.0, 30.0, 6.0, f32::NAN, RED),
            particle(30.0, 30.0, 6.0, 0.0, [f32::NAN, 0.0, 0.0, 1.0]),
            particle(30.0, 30.0, 6.0, 0.0, [1.0, f32::INFINITY, 0.0, 1.0]),
        ] {
            let frame = render_with(&cfg, &[good, bad], 60, 60, 1.0);
            assert_eq!(frame, reference, "a non-finite particle changed the frame");
        }
        assert_eq!(pixel(&reference, 60, 30, 30), [255, 255, 255, 255]);
    }

    #[test]
    fn particle_alpha_premultiplies_the_whole_pixel() {
        let f = render(ParticleShape::Circle, BlendMode::SourceOver, &[particle(20.0, 20.0, 5.0, 0.0, [1.0, 0.0, 0.0, 0.5])], 40, 40, 1.0);
        let [r, g, b, a] = pixel(&f, 40, 20, 20);
        assert!((r as i32 - 128).abs() <= 1 && g == 0 && b == 0 && (a as i32 - 128).abs() <= 1, "got {:?}", [r, g, b, a]);
    }

    #[test]
    fn blend_mode_decides_how_overlapping_particles_combine() {
        let two = [particle(20.0, 20.0, 6.0, 0.0, [1.0, 0.0, 0.0, 0.5]), particle(20.0, 20.0, 6.0, 0.0, [1.0, 0.0, 0.0, 0.5])];
        let over = render(ParticleShape::Circle, BlendMode::SourceOver, &two, 40, 40, 1.0);
        let lighter = render(ParticleShape::Circle, BlendMode::Lighter, &two, 40, 40, 1.0);
        assert!((pixel(&over, 40, 20, 20)[3] as i32 - 191).abs() <= 1, "0.5 over 0.5 = 0.75");
        assert_eq!(pixel(&lighter, 40, 20, 20)[3], 255, "0.5 + 0.5 = 1.0");
    }

    #[test]
    fn scale_grows_the_footprint_in_device_pixels() {
        let one = render(ParticleShape::Circle, BlendMode::SourceOver, &[particle(20.0, 20.0, 5.0, 0.0, RED)], 80, 80, 1.0);
        let two = render(ParticleShape::Circle, BlendMode::SourceOver, &[particle(20.0, 20.0, 5.0, 0.0, RED)], 80, 80, 2.0);
        assert_eq!(pixel(&one, 80, 40, 40), [0, 0, 0, 0], "at scale 1 the circle is around (20, 20)");
        assert_eq!(pixel(&two, 80, 40, 40), [255, 0, 0, 255], "at scale 2 it is around (40, 40)");
        assert_eq!(pixel(&two, 80, 48, 40), [255, 0, 0, 255], "8px inside a 10px device radius");
    }

    #[test]
    fn every_shape_draws_something_and_stays_inside_its_extent_at_every_size_with_and_without_glow() {
        const W: u32 = 220;
        const CENTER: f32 = 110.0;
        for shape in shapes::ALL {
            let extent = shapes::shape_for(shape).extent;
            for &size in &[2.0f32, 8.0, 30.0] {
                for glow_on in [false, true] {
                    let cfg = if glow_on { glow_config(shape, 10.0) } else { config(shape, BlendMode::SourceOver) };
                    let f = render_with(&cfg, &[particle(CENTER, CENTER, size, 0.7, RED)], W, W, 1.0);
                    let limit = if glow_on {
                        // Three sigma plus slack, matching the halo's blur reach.
                        let blur = glow_blur_logical(10.0, size);
                        extent * size + 3.0 * (blur * 0.5) + 6.0
                    } else {
                        extent * size + 3.0
                    };
                    let mut painted = 0;
                    for y in 0..W {
                        for x in 0..W {
                            if pixel(&f, W, x, y)[3] > 0 {
                                painted += 1;
                                let d = ((x as f32 + 0.5 - CENTER).powi(2) + (y as f32 + 0.5 - CENTER).powi(2)).sqrt();
                                assert!(
                                    d <= limit,
                                    "{shape:?} size {size} glow {glow_on} painted ({x}, {y}) at {d} beyond {limit}"
                                );
                            }
                        }
                    }
                    if size <= 2.0 {
                        assert!(painted > 0, "{shape:?} size {size} glow {glow_on} painted nothing");
                    } else {
                        assert!(painted > 10, "{shape:?} size {size} glow {glow_on} painted only {painted} pixels");
                    }
                }
            }
        }
    }

    #[test]
    fn a_quarter_turn_transposes_a_diamond() {
        let upright = render(ParticleShape::Diamond, BlendMode::SourceOver, &[particle(20.5, 20.5, 12.0, 0.0, RED)], 41, 41, 1.0);
        let turned = render(ParticleShape::Diamond, BlendMode::SourceOver, &[particle(20.5, 20.5, 12.0, std::f32::consts::FRAC_PI_2, RED)], 41, 41, 1.0);
        let mut off = 0;
        for y in 0..41 {
            for x in 0..41 {
                let a = pixel(&upright, 41, x, y)[3] as i32;
                let b = pixel(&turned, 41, y, x)[3] as i32;
                if (a - b).abs() > 3 {
                    off += 1;
                }
            }
        }
        assert!(off <= 4, "{off} pixels differ between the rotated diamond and the transposed one");
        assert_eq!(pixel(&upright, 41, 20, 10)[3], 255, "upright diamond is tall");
        assert_eq!(pixel(&upright, 41, 10, 20)[3], 0, "and narrow");
        assert_eq!(pixel(&turned, 41, 10, 20)[3], 255, "turned diamond is wide");
    }

    /// Painted rows and columns (alpha over half) of a frame: (width, height).
    fn painted_extent(f: &[u8], w: u32) -> (u32, u32) {
        let (mut x0, mut x1, mut y0, mut y1) = (w, 0, w, 0);
        for y in 0..w {
            for x in 0..w {
                if pixel(f, w, x, y)[3] > 127 {
                    x0 = x0.min(x);
                    x1 = x1.max(x);
                    y0 = y0.min(y);
                    y1 = y1.max(y);
                }
            }
        }
        (x1 + 1 - x0, y1 + 1 - y0)
    }

    #[test]
    fn a_capsule_is_twice_its_size_long_and_a_2_3rd_of_that_wide() {
        // size is the half-length, as for the diamond: size 20 is a 40 px
        // capsule, 40 / 2.3 = 17.4 px across.
        let f = render(ParticleShape::Capsule, BlendMode::SourceOver, &[particle(40.5, 40.5, 20.0, 0.0, RED)], 81, 81, 1.0);
        let (w, h) = painted_extent(&f, 81);
        assert!((39..=41).contains(&h), "capsule is {h} px long");
        assert!((16..=19).contains(&w), "capsule is {w} px wide");
    }

    #[test]
    fn a_capsule_has_round_ends_and_straight_sides() {
        let f = render(ParticleShape::Capsule, BlendMode::SourceOver, &[particle(40.5, 40.5, 20.0, 0.0, RED)], 81, 81, 1.0);
        // The half-width is 8.7: a side 7 px out is solid along the whole
        // straight run (half-length 20 - 8.7 = 11.3), where a lens or a
        // diamond would already be narrowing.
        for dy in [-10i32, 0, 10] {
            assert_eq!(pixel(&f, 81, 47, (40 + dy) as u32)[3], 255, "side at dy {dy}");
        }
        // The corners of the bounding box are cut away by the round ends.
        assert_eq!(pixel(&f, 81, 47, 58)[3], 0, "bottom-right corner");
        assert_eq!(pixel(&f, 81, 33, 22)[3], 0, "top-left corner");
        // But the tip itself is painted.
        assert_eq!(pixel(&f, 81, 40, 59)[3], 255, "bottom tip");
    }

    #[test]
    fn a_quarter_turn_lays_a_capsule_on_its_side() {
        let turned = render(
            ParticleShape::Capsule,
            BlendMode::SourceOver,
            &[particle(40.5, 40.5, 20.0, std::f32::consts::FRAC_PI_2, RED)],
            81,
            81,
            1.0,
        );
        let (w, h) = painted_extent(&turned, 81);
        assert!((39..=41).contains(&w), "turned capsule is {w} px long");
        assert!((16..=19).contains(&h), "turned capsule is {h} px tall");
    }

    #[test]
    fn a_tiny_circle_still_paints_its_minimum_half_pixel_radius() {
        let f = render(ParticleShape::Circle, BlendMode::SourceOver, &[particle(20.5, 20.5, 0.01, 0.0, RED)], 40, 40, 1.0);
        assert!(pixel(&f, 40, 20, 20)[3] > 100, "min_size 0.5 gives a visible dot");
    }

    #[test]
    fn particles_off_frame_or_invalid_are_skipped_without_panicking() {
        let ps = [
            particle(-500.0, 20.0, 5.0, 0.0, RED),
            particle(20.0, 5000.0, 5.0, 0.0, RED),
            particle(f32::NAN, 20.0, 5.0, 0.0, RED),
            particle(20.0, 20.0, 0.0, 0.0, RED),
            particle(20.0, 20.0, 5.0, 0.0, [1.0, 0.0, 0.0, 0.0]),
        ];
        let f = render(ParticleShape::Circle, BlendMode::SourceOver, &ps, 40, 40, 1.0);
        assert!(f.iter().all(|&b| b == 0));
    }

    #[test]
    fn a_particle_straddling_the_frame_edge_is_clipped() {
        let f = render(ParticleShape::Circle, BlendMode::SourceOver, &[particle(0.0, 20.0, 6.0, 0.0, RED)], 40, 40, 1.0);
        assert_eq!(pixel(&f, 40, 0, 20), [255, 0, 0, 255]);
        // 2px margin inside a 6px radius, matching the margin used by
        // scale_grows_the_footprint (8px inside a 10px radius): 1px in (the
        // brief's original x=5) sits at the circle's horizontal tangent,
        // where true analytic coverage of that pixel cell is provably
        // ~0.972 (converges there even at a 0.0001px flatten tolerance),
        // never reaching opaque: the tangent pixel of a radius-6 circle
        // centered on an integer x has analytic coverage about 0.97, so the
        // probe sits one pixel further in.
        assert_eq!(pixel(&f, 40, 4, 20), [255, 0, 0, 255]);
        assert_eq!(pixel(&f, 40, 7, 20), [0, 0, 0, 0]);
    }

    #[test]
    fn render_without_a_viewport_is_a_no_op() {
        let mut r = Renderer::new();
        r.render(&[particle(1.0, 1.0, 5.0, 0.0, RED)], &config(ParticleShape::Circle, BlendMode::SourceOver));
        assert!(r.frame().is_empty());
    }

    fn glow_config(shape: ParticleShape, glow_radius: f32) -> ParticleFxConfig {
        ParticleFxConfig { shape, blend_mode: BlendMode::SourceOver, glow_bloom: true, glow_radius, ..Default::default() }
    }

    #[test]
    fn glow_paints_outside_the_shape_where_no_glow_leaves_nothing() {
        let p = [particle(30.0, 30.0, 6.0, 0.0, RED)];
        let mut plain = Renderer::new();
        plain.set_viewport(60, 60, 1.0).unwrap();
        plain.render(&p, &config(ParticleShape::Circle, BlendMode::SourceOver));
        let mut glowing = Renderer::new();
        glowing.set_viewport(60, 60, 1.0).unwrap();
        glowing.render(&p, &glow_config(ParticleShape::Circle, 10.0));
        // 10px from center, 4px outside a 6px radius.
        assert_eq!(pixel(plain.frame(), 60, 40, 30)[3], 0);
        let halo = pixel(glowing.frame(), 60, 40, 30);
        assert!(halo[3] > 0, "glow should reach 4px outside the shape");
        assert!(halo[0] >= halo[3] - 1 && halo[1] == 0, "glow carries the tint: {halo:?}");
        assert_eq!(pixel(glowing.frame(), 60, 30, 30), [255, 0, 0, 255], "the shape itself is still solid");
    }

    #[test]
    fn glow_blur_follows_the_mouseflare_formula_and_cap() {
        assert!((glow_blur_logical(6.0, 6.0) - 6.0).abs() < 1e-6);
        assert!((glow_blur_logical(10.0, 3.0) - 5.0).abs() < 1e-6);
        assert_eq!(glow_blur_logical(30.0, 40.0), MAX_GLOW_BLUR);
        assert_eq!(glow_blur_logical(0.0, 40.0), 0.0);
    }

    #[test]
    fn an_absurd_particle_renders_within_the_glow_budget() {
        // Through the Rust API nothing clamps a particle's size. The halo
        // is bounded by the frame diagonal and by MAX_SPRITE_RADIUS, and
        // the cache never holds more than its budget.
        let mut r = Renderer::new();
        r.set_viewport(512, 512, 1.0).unwrap();
        let cfg = glow_config(ParticleShape::Circle, 10.0);
        r.render(&[particle(256.0, 256.0, 1e6, 0.0, RED)], &cfg);
        assert!(r.glow.bytes() <= glow::MAX_GLOW_CACHE_BYTES);
        assert!(r.frame().iter().any(|&b| b != 0), "painted nothing");
    }

    #[test]
    fn glow_sprites_are_reused_across_frames_and_particles() {
        let mut r = Renderer::new();
        r.set_viewport(80, 80, 1.0).unwrap();
        let cfg = glow_config(ParticleShape::Circle, 8.0);
        let ps = [particle(20.0, 20.0, 6.0, 0.0, RED), particle(50.0, 50.0, 6.0, 1.0, [0.0, 1.0, 0.0, 1.0])];
        r.render(&ps, &cfg);
        r.render(&ps, &cfg);
        assert_eq!(r.glow.len(), 1, "same shape, size, and blur share one sprite regardless of color or rotation");
    }

    #[test]
    fn set_viewport_drops_glow_sprites_built_at_the_old_scale() {
        let mut r = Renderer::new();
        r.set_viewport(80, 80, 1.0).unwrap();
        let cfg = glow_config(ParticleShape::Ring, 8.0);
        r.render(&[particle(20.0, 20.0, 6.0, 0.0, RED)], &cfg);
        assert_eq!(r.glow.len(), 1);
        r.set_viewport(80, 80, 2.0).unwrap();
        assert_eq!(r.glow.len(), 0, "sprites bake the scale in; a new viewport must not reuse them");
    }

    #[test]
    fn a_symmetric_halo_ignores_rotation_entirely() {
        // Circle, glow-disc, ring, and smoke-puff halos are stamped
        // unrotated; a rotated particle must produce the very same bytes.
        for shape in [ParticleShape::Circle, ParticleShape::GlowDisc, ParticleShape::Ring, ParticleShape::SmokePuff] {
            let cfg = glow_config(shape, 12.0);
            let upright = render_with(&cfg, &[particle(30.3, 29.6, 8.0, 0.0, RED)], 60, 60, 1.0);
            let turned = render_with(&cfg, &[particle(30.3, 29.6, 8.0, 1.3, RED)], 60, 60, 1.0);
            // The shape itself does rotate, so compare only outside it.
            let extent = shapes::shape_for(shape).extent * 8.0 + 3.0;
            for y in 0..60 {
                for x in 0..60 {
                    let d = ((x as f32 + 0.5 - 30.3).powi(2) + (y as f32 + 0.5 - 29.6).powi(2)).sqrt();
                    if d > extent {
                        assert_eq!(pixel(&upright, 60, x, y), pixel(&turned, 60, x, y), "{shape:?} halo at ({x}, {y})");
                    }
                }
            }
            assert!(upright.iter().any(|&b| b != 0), "{shape:?} painted nothing");
        }
    }

    #[test]
    fn glow_is_rotated_with_the_particle() {
        // A long thin bolt glows along its length; rotated a quarter turn
        // the halo must move with it.
        let far_right = |rotation: f32| {
            let f = render_with(&glow_config(ParticleShape::LightningBolt, 12.0), &[particle(30.0, 30.0, 10.0, rotation, RED)], 60, 60, 1.0);
            pixel(&f, 60, 30, 46)[3]
        };
        let upright = far_right(0.0);
        let turned = far_right(std::f32::consts::FRAC_PI_2);
        assert!(upright > turned, "below the bolt: upright {upright} should out-glow turned {turned}");
    }

    fn render_with(cfg: &ParticleFxConfig, particles: &[ParticleInstance], w: u32, h: u32, scale: f32) -> Vec<u8> {
        let mut r = Renderer::new();
        r.set_viewport(w, h, scale).unwrap();
        r.render(particles, cfg);
        r.frame().to_vec()
    }
}
