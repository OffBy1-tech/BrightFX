//! Anti-aliased scanline rasterizer using signed-area coverage accumulation
//! (the font-rs technique). Each edge deposits signed area and cover into an
//! accumulation buffer; a prefix sum per row turns that into coverage.
//!
//! Coverage is `min(1, |sum|)`, and `add_polygon` normalizes every polygon
//! to positive orientation, so overlapping polygons union rather than
//! cancel. That is what lets a stroke be built from separate quads and join
//! circles. Holes drawn as opposite-winding subpaths are not supported; no
//! shape needs them.

pub(crate) struct Rasterizer {
    width: usize,
    height: usize,
    /// Reused by `add_stroke` for each join and cap disc, so stroking a
    /// contour allocates nothing per polygon.
    stroke_scratch: Vec<[f32; 2]>,
    /// Row stride is `width + 2`: x is clamped into `[0, width]` before an
    /// edge is drawn, so deposits land in columns `0..=width + 1`. Columns
    /// `>= width` are never read back, which is how edges to the right of
    /// the tile are clipped.
    acc: Vec<f32>,
}

impl Rasterizer {
    pub fn new() -> Self {
        Self { width: 0, height: 0, stroke_scratch: Vec::new(), acc: Vec::new() }
    }

    /// Adds every polygon of `contour` stroked at `width`; see `path::stroke_with`.
    pub fn add_stroke(&mut self, contour: &super::path::Contour, width: f32, round_caps: bool, tolerance: f32) {
        let mut scratch = std::mem::take(&mut self.stroke_scratch);
        super::path::stroke_with(contour, width, round_caps, tolerance, &mut scratch, |poly| self.add_polygon(poly));
        self.stroke_scratch = scratch;
    }

    /// Sets the tile size and clears it.
    pub fn resize(&mut self, width: usize, height: usize) {
        self.width = width;
        self.height = height;
        self.acc.clear();
        self.acc.resize((width + 2) * height, 0.0);
    }

    /// Adds a closed polygon in tile pixel coordinates. Orientation does not
    /// matter; every polygon unions with what is already there.
    pub fn add_polygon(&mut self, points: &[[f32; 2]]) {
        if points.len() < 3 {
            return;
        }
        // NaN survives every clamp and comparison below and would turn into
        // full-width bands in coverage_into; reject it like the degenerate
        // case above.
        if points.iter().any(|p| !p[0].is_finite() || !p[1].is_finite()) {
            return;
        }
        // Shoelace sign: normalize to positive orientation so polygons union.
        let mut area2 = 0.0f32;
        for i in 0..points.len() {
            let [x0, y0] = points[i];
            let [x1, y1] = points[(i + 1) % points.len()];
            area2 += x0 * y1 - x1 * y0;
        }
        let flip = area2 < 0.0;
        for i in 0..points.len() {
            let a = points[i];
            let b = points[(i + 1) % points.len()];
            let (a, b) = if flip { (b, a) } else { (a, b) };
            self.add_edge(a, b);
        }
    }

    /// Splits an edge where it crosses the tile's left and right borders,
    /// then clamps x so pieces outside become vertical edges on the border.
    /// Their cover still reaches the pixels inside the tile, which is what
    /// clipping against a larger canvas would produce.
    fn add_edge(&mut self, a: [f32; 2], b: [f32; 2]) {
        let w = self.width as f32;
        let mut ts = [0.0f32; 4];
        let mut n = 1; // ts[0] = 0.0
        let dx = b[0] - a[0];
        if dx != 0.0 {
            for border in [0.0, w] {
                let t = (border - a[0]) / dx;
                if t > 0.0 && t < 1.0 {
                    ts[n] = t;
                    n += 1;
                }
            }
        }
        ts[n] = 1.0;
        n += 1;
        let ts = &mut ts[..n];
        ts.sort_by(f32::total_cmp);
        for pair in ts.windows(2) {
            let (t0, t1) = (pair[0], pair[1]);
            if t1 <= t0 {
                continue;
            }
            let px = a[0] + (b[0] - a[0]) * t0;
            let py = a[1] + (b[1] - a[1]) * t0;
            let qx = a[0] + (b[0] - a[0]) * t1;
            let qy = a[1] + (b[1] - a[1]) * t1;
            self.draw_line(px.clamp(0.0, w), py, qx.clamp(0.0, w), qy);
        }
    }

    /// Deposits one edge's signed area and cover. Port of font-rs's
    /// accumulation, with y clipped to the tile and x already in `[0, w]`.
    fn draw_line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32) {
        if (y0 - y1).abs() <= f32::EPSILON {
            return;
        }
        let (dir, x0, y0, x1, y1) = if y0 < y1 {
            (1.0f32, x0, y0, x1, y1)
        } else {
            (-1.0f32, x1, y1, x0, y0)
        };
        let w = self.width as f32;
        let stride = self.width + 2;
        let dxdy = (x1 - x0) / (y1 - y0);
        let mut x = x0;
        // Re-derives x at y = 0 after the caller's clamp. Safe: this only
        // matters when y0 < 0 <= y1, so the result is a convex combination of
        // two endpoints already in [0, w] and stays there up to rounding.
        if y0 < 0.0 {
            x -= y0 * dxdy;
        }
        let y_start = y0.max(0.0) as usize;
        let y_end = (y1.ceil().max(0.0) as usize).min(self.height);
        for y in y_start..y_end {
            let row = y * stride;
            let dy = ((y + 1) as f32).min(y1) - (y as f32).max(y0);
            let xnext = (x + dxdy * dy).clamp(0.0, w);
            let d = dy * dir;
            let (xa, xb) = if x < xnext { (x, xnext) } else { (xnext, x) };
            let xa_floor = xa.floor();
            let xa_i = xa_floor as usize;
            let xb_ceil = xb.ceil();
            let xb_i = xb_ceil as usize;
            if xb_i <= xa_i + 1 {
                let xmf = 0.5 * (x + xnext) - xa_floor;
                self.acc[row + xa_i] += d - d * xmf;
                self.acc[row + xa_i + 1] += d * xmf;
            } else {
                let s = (xb - xa).recip();
                let xa_f = xa - xa_floor;
                let a0 = 0.5 * s * (1.0 - xa_f) * (1.0 - xa_f);
                let xb_f = xb - xb_ceil + 1.0;
                let am = 0.5 * s * xb_f * xb_f;
                self.acc[row + xa_i] += d * a0;
                if xb_i == xa_i + 2 {
                    self.acc[row + xa_i + 1] += d * (1.0 - a0 - am);
                } else {
                    let a1 = s * (1.5 - xa_f);
                    self.acc[row + xa_i + 1] += d * (a1 - a0);
                    for xi in xa_i + 2..xb_i - 1 {
                        self.acc[row + xi] += d * s;
                    }
                    let a2 = a1 + (xb_i - xa_i - 3) as f32 * s;
                    self.acc[row + xb_i - 1] += d * (1.0 - a2 - am);
                }
                self.acc[row + xb_i] += d * am;
            }
            x = xnext;
        }
    }

    /// Resolves the accumulation buffer into per-pixel coverage.
    pub fn coverage_into(&self, out: &mut Vec<f32>) {
        out.clear();
        out.resize(self.width * self.height, 0.0);
        let stride = self.width + 2;
        for y in 0..self.height {
            let mut sum = 0.0f32;
            for x in 0..self.width {
                sum += self.acc[y * stride + x];
                out[y * self.width + x] = sum.abs().min(1.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coverage(width: usize, height: usize, polygons: &[&[[f32; 2]]]) -> Vec<f32> {
        let mut r = Rasterizer::new();
        r.resize(width, height);
        for poly in polygons {
            r.add_polygon(poly);
        }
        let mut out = Vec::new();
        r.coverage_into(&mut out);
        out
    }

    fn sum(cov: &[f32]) -> f32 {
        cov.iter().sum()
    }

    #[test]
    fn an_empty_tile_has_zero_coverage() {
        let cov = coverage(4, 4, &[]);
        assert_eq!(cov.len(), 16);
        assert!(cov.iter().all(|&c| c == 0.0));
    }

    #[test]
    fn a_pixel_aligned_square_covers_exactly_its_pixels() {
        let square = [[2.0, 2.0], [6.0, 2.0], [6.0, 6.0], [2.0, 6.0]];
        let cov = coverage(8, 8, &[&square]);
        assert!((cov[3 * 8 + 3] - 1.0).abs() < 1e-5, "inside pixel");
        assert_eq!(cov[8 + 1], 0.0, "outside pixel");
        assert_eq!(cov[6 * 8 + 6], 0.0, "just past the far edge");
        assert!((sum(&cov) - 16.0).abs() < 1e-3, "area was {}", sum(&cov));
    }

    #[test]
    fn a_half_pixel_offset_square_has_quarter_coverage_at_its_corners() {
        let square = [[2.5, 2.5], [5.5, 2.5], [5.5, 5.5], [2.5, 5.5]];
        let cov = coverage(8, 8, &[&square]);
        assert!((cov[2 * 8 + 2] - 0.25).abs() < 1e-5, "corner {}", cov[2 * 8 + 2]);
        assert!((cov[2 * 8 + 3] - 0.5).abs() < 1e-5, "edge {}", cov[2 * 8 + 3]);
        assert!((cov[3 * 8 + 3] - 1.0).abs() < 1e-5, "center");
        assert!((sum(&cov) - 9.0).abs() < 1e-3);
    }

    #[test]
    fn a_triangle_covers_half_its_bounding_box() {
        let tri = [[0.0, 0.0], [8.0, 0.0], [0.0, 8.0]];
        let cov = coverage(8, 8, &[&tri]);
        assert!((sum(&cov) - 32.0).abs() < 1e-2, "area was {}", sum(&cov));
    }

    #[test]
    fn a_reversed_polygon_gives_the_same_coverage() {
        let cw = [[2.0, 2.0], [6.0, 2.0], [6.0, 6.0], [2.0, 6.0]];
        let ccw = [[2.0, 2.0], [2.0, 6.0], [6.0, 6.0], [6.0, 2.0]];
        assert_eq!(coverage(8, 8, &[&cw]), coverage(8, 8, &[&ccw]));
    }

    #[test]
    fn overlapping_polygons_union_instead_of_cancelling_or_doubling() {
        let a = [[2.0, 2.0], [6.0, 2.0], [6.0, 6.0], [2.0, 6.0]];
        let b = [[2.0, 2.0], [2.0, 6.0], [6.0, 6.0], [6.0, 2.0]]; // opposite orientation
        let cov = coverage(8, 8, &[&a, &b]);
        assert!((cov[3 * 8 + 3] - 1.0).abs() < 1e-5, "overlap must stay 1, got {}", cov[3 * 8 + 3]);
        assert!((sum(&cov) - 16.0).abs() < 1e-3);
    }

    #[test]
    fn a_polygon_hanging_off_the_left_edge_is_clipped_not_lost() {
        let square = [[-3.0, 1.0], [3.0, 1.0], [3.0, 5.0], [-3.0, 5.0]];
        let cov = coverage(8, 8, &[&square]);
        assert!((cov[2 * 8] - 1.0).abs() < 1e-5, "column 0 inside");
        assert!((sum(&cov) - 12.0).abs() < 1e-3, "visible area was {}", sum(&cov));
    }

    #[test]
    fn a_polygon_hanging_off_the_right_and_bottom_edges_is_clipped() {
        let square = [[5.0, 5.0], [12.0, 5.0], [12.0, 12.0], [5.0, 12.0]];
        let cov = coverage(8, 8, &[&square]);
        assert!((sum(&cov) - 9.0).abs() < 1e-3, "visible area was {}", sum(&cov));
    }

    #[test]
    fn a_polygon_entirely_outside_leaves_the_tile_empty() {
        let far = [[20.0, 20.0], [30.0, 20.0], [30.0, 30.0]];
        let above = [[1.0, -9.0], [5.0, -9.0], [5.0, -2.0]];
        let cov = coverage(8, 8, &[&far, &above]);
        assert!(cov.iter().all(|&c| c == 0.0));
    }

    #[test]
    fn a_polygon_spanning_the_whole_tile_horizontally_fills_its_rows() {
        let band = [[-5.0, 2.0], [13.0, 2.0], [13.0, 4.0], [-5.0, 4.0]];
        let cov = coverage(8, 8, &[&band]);
        assert!((sum(&cov) - 16.0).abs() < 1e-3, "area was {}", sum(&cov));
    }

    #[test]
    fn degenerate_polygons_are_ignored() {
        let cov = coverage(4, 4, &[&[[1.0, 1.0], [2.0, 2.0]]]);
        assert!(cov.iter().all(|&c| c == 0.0));
    }

    #[test]
    fn a_polygon_with_a_non_finite_vertex_is_ignored() {
        let nan = [[f32::NAN, 1.0], [5.0, 1.0], [5.0, 5.0]];
        let inf = [[1.0, f32::INFINITY], [5.0, 1.0], [5.0, 5.0]];
        let cov = coverage(8, 8, &[&nan, &inf]);
        assert!(cov.iter().all(|&c| c == 0.0), "non-finite input painted pixels");

        // And it must not poison a later, valid polygon in the same tile.
        let square = [[2.0, 2.0], [6.0, 2.0], [6.0, 6.0], [2.0, 6.0]];
        let cov = coverage(8, 8, &[&nan, &square]);
        assert!((sum(&cov) - 16.0).abs() < 1e-3, "area was {}", sum(&cov));
    }
}
