//! Shape geometry in shape units, and its flattening into device-space
//! polygons. Circles are four cubic Béziers, so nothing here calls
//! `sin`/`cos` except `Transform::new`, once per particle.

/// Similarity transform: scale, rotate, translate. Rotation is in radians
/// in a y-down space, so a positive angle turns clockwise on screen, the
/// same as `CanvasRenderingContext2D.rotate`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Transform {
    a: f32,
    b: f32,
    c: f32,
    d: f32,
    e: f32,
    f: f32,
}

impl Transform {
    pub fn new(scale: f32, rotation: f32, tx: f32, ty: f32) -> Self {
        let (sin, cos) = rotation.sin_cos();
        Self { a: cos * scale, b: sin * scale, c: -sin * scale, d: cos * scale, e: tx, f: ty }
    }

    #[cfg(test)]
    pub fn identity() -> Self {
        Self { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 }
    }

    pub fn apply(&self, x: f32, y: f32) -> [f32; 2] {
        [self.a * x + self.c * y + self.e, self.b * x + self.d * y + self.f]
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum PathCmd {
    Move(f32, f32),
    Line(f32, f32),
    /// Two control points then the end point.
    Cubic(f32, f32, f32, f32, f32, f32),
    Close,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Geometry {
    Circle { cx: f32, cy: f32, r: f32 },
    Path(&'static [PathCmd]),
}

pub(crate) struct Contour {
    pub points: Vec<[f32; 2]>,
    pub closed: bool,
}

/// Control-point distance for a quarter circle as cubic Bézier.
pub(crate) const KAPPA: f32 = 0.552_284_8;

/// Flattens a geometry through `transform` into polylines. `tolerance` is
/// the largest allowed deviation from the true curve, in output units.
pub(crate) fn flatten(geometry: &Geometry, transform: &Transform, tolerance: f32) -> Vec<Contour> {
    match *geometry {
        Geometry::Circle { cx, cy, r } => {
            let k = KAPPA * r;
            let cmds = [
                PathCmd::Move(cx + r, cy),
                PathCmd::Cubic(cx + r, cy + k, cx + k, cy + r, cx, cy + r),
                PathCmd::Cubic(cx - k, cy + r, cx - r, cy + k, cx - r, cy),
                PathCmd::Cubic(cx - r, cy - k, cx - k, cy - r, cx, cy - r),
                PathCmd::Cubic(cx + k, cy - r, cx + r, cy - k, cx + r, cy),
                PathCmd::Close,
            ];
            flatten_commands(&cmds, transform, tolerance)
        }
        Geometry::Path(cmds) => flatten_commands(cmds, transform, tolerance),
    }
}

/// Flattens path commands into device-space polylines. Close returns the
/// current point to the subpath's start, matching canvas semantics, so a
/// following Line or Cubic draws from there.
fn flatten_commands(cmds: &[PathCmd], t: &Transform, tolerance: f32) -> Vec<Contour> {
    let mut contours = Vec::new();
    let mut current: Vec<[f32; 2]> = Vec::new();
    let mut subpath_start: Option<[f32; 2]> = None;

    fn flush(current: &mut Vec<[f32; 2]>, closed: bool, contours: &mut Vec<Contour>) {
        if current.len() >= 2 {
            contours.push(Contour { points: std::mem::take(current), closed });
        } else {
            current.clear();
        }
    }

    for cmd in cmds {
        match *cmd {
            PathCmd::Move(x, y) => {
                flush(&mut current, false, &mut contours);
                let pt = t.apply(x, y);
                current.push(pt);
                subpath_start = Some(pt);
            }
            PathCmd::Line(x, y) => {
                // After Close, continue from the subpath start (canvas semantics).
                if current.is_empty() {
                    if let Some(start) = subpath_start {
                        current.push(start);
                    }
                }
                current.push(t.apply(x, y));
            }
            PathCmd::Cubic(x1, y1, x2, y2, x3, y3) => {
                // After Close, continue from the subpath start (canvas semantics).
                if current.is_empty() {
                    if let Some(start) = subpath_start {
                        current.push(start);
                    }
                }
                let p0 = match current.last() {
                    Some(p) => *p,
                    None => continue,
                };
                flatten_cubic(p0, t.apply(x1, y1), t.apply(x2, y2), t.apply(x3, y3), tolerance, &mut current);
            }
            PathCmd::Close => flush(&mut current, true, &mut contours),
        }
    }
    flush(&mut current, false, &mut contours);
    contours
}

/// Uniform subdivision with the segment count from Wang's formula, so the
/// polyline stays within `tolerance` of the curve. Ends exactly at `p3`.
fn flatten_cubic(
    p0: [f32; 2],
    p1: [f32; 2],
    p2: [f32; 2],
    p3: [f32; 2],
    tolerance: f32,
    out: &mut Vec<[f32; 2]>,
) {
    let dd1 = ((p0[0] - 2.0 * p1[0] + p2[0]).powi(2) + (p0[1] - 2.0 * p1[1] + p2[1]).powi(2)).sqrt();
    let dd2 = ((p1[0] - 2.0 * p2[0] + p3[0]).powi(2) + (p1[1] - 2.0 * p2[1] + p3[1]).powi(2)).sqrt();
    let dd = dd1.max(dd2);
    let n = ((0.75 * dd / tolerance.max(1e-4)).sqrt().ceil() as usize).clamp(1, 64);
    for i in 1..n {
        let t = i as f32 / n as f32;
        let mt = 1.0 - t;
        let a = mt * mt * mt;
        let b = 3.0 * mt * mt * t;
        let c = 3.0 * mt * t * t;
        let d = t * t * t;
        out.push([
            a * p0[0] + b * p1[0] + c * p2[0] + d * p3[0],
            a * p0[1] + b * p1[1] + c * p2[1] + d * p3[1],
        ]);
    }
    out.push(p3);
}

/// A circle as a polygon in output units.
#[cfg(test)]
pub(crate) fn circle_polygon(center: [f32; 2], r: f32, tolerance: f32) -> Vec<[f32; 2]> {
    let mut out = Vec::new();
    circle_points_into(center, r, tolerance, &mut out);
    out
}

/// `circle_polygon` into a reused buffer. The four cubics are evaluated
/// exactly as `flatten` evaluates them through the identity transform, so
/// the two produce the same points bit for bit.
fn circle_points_into(center: [f32; 2], r: f32, tolerance: f32, out: &mut Vec<[f32; 2]>) {
    out.clear();
    let [cx, cy] = center;
    let k = KAPPA * r;
    let start = [cx + r, cy];
    out.push(start);
    for [p1, p2, p3] in [
        [[cx + r, cy + k], [cx + k, cy + r], [cx, cy + r]],
        [[cx - k, cy + r], [cx - r, cy + k], [cx - r, cy]],
        [[cx - r, cy - k], [cx - k, cy - r], [cx, cy - r]],
        [[cx + k, cy - r], [cx + r, cy - k], [cx + r, cy]],
    ] {
        let p0 = *out.last().unwrap();
        flatten_cubic(p0, p1, p2, p3, tolerance, out);
    }
}

/// The most a bevel join may fall short of a round one, in output pixels,
/// before the join is drawn round. A bevel closes the wedge between two
/// segment quads with a straight edge; a round join closes it with an arc
/// of radius `half` spanning the turn angle, and the two differ by the
/// arc's sagitta, `half * (1 - cos(turn / 2))`. Below this the difference
/// cannot reach a twentieth of a pixel of coverage, and the bevel costs
/// three edges where the disc costs a dozen or more. A flattened curve
/// turns by a few degrees per segment, so its joins are all bevels; the
/// sharp corners of a rune or a bolt stay round.
const BEVEL_MAX_SAGITTA: f32 = 0.05;

/// Expands a polyline into polygons whose union is the stroke, handing each
/// to `emit`: one quad per segment, a join at every vertex (a bevel where
/// that is indistinguishable from round, a disc otherwise), and a disc at
/// each end when `round_caps` is set. Butt caps end flush with the segment.
/// Canvas uses miter joins by default; at particle sizes the difference is
/// invisible.
///
/// `scratch` holds each disc in turn, so a stroke with dozens of joins
/// allocates nothing per polygon; the caller keeps it between strokes.
pub(crate) fn stroke_with(
    contour: &Contour,
    width: f32,
    round_caps: bool,
    tolerance: f32,
    scratch: &mut Vec<[f32; 2]>,
    mut emit: impl FnMut(&[[f32; 2]]),
) {
    let half = width * 0.5;
    let pts = &contour.points;
    if pts.is_empty() || half <= 0.0 {
        return;
    }
    let n = pts.len();
    let segments = if contour.closed { n } else { n - 1 };
    for i in 0..segments {
        let p = pts[i];
        let q = pts[(i + 1) % n];
        let dx = q[0] - p[0];
        let dy = q[1] - p[1];
        let len = (dx * dx + dy * dy).sqrt();
        if len <= 1e-6 {
            continue;
        }
        let nx = -dy / len * half;
        let ny = dx / len * half;
        emit(&[
            [p[0] + nx, p[1] + ny],
            [q[0] + nx, q[1] + ny],
            [q[0] - nx, q[1] - ny],
            [p[0] - nx, p[1] - ny],
        ]);
    }
    let joins = if contour.closed { 0..n } else { 1..n.saturating_sub(1) };
    for i in joins {
        let p = pts[i];
        // Unit normals of the segments into and out of the vertex. A zero
        // length segment has no direction; keep the join round then.
        let normal = |a: [f32; 2], b: [f32; 2]| {
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let len = (dx * dx + dy * dy).sqrt();
            (len > 1e-6).then(|| [-dy / len, dx / len])
        };
        let incoming = normal(pts[(i + n - 1) % n], p);
        let outgoing = normal(p, pts[(i + 1) % n]);
        if let (Some(n1), Some(n2)) = (incoming, outgoing) {
            // cos(turn) is the normals' dot product; the sagitta needs
            // cos(turn / 2), from the half-angle identity.
            let cos_turn = (n1[0] * n2[0] + n1[1] * n2[1]).clamp(-1.0, 1.0);
            let cos_half = ((1.0 + cos_turn) * 0.5).sqrt();
            if half * (1.0 - cos_half) <= BEVEL_MAX_SAGITTA {
                // Both sides: one closes the outer wedge, the other lies
                // inside the stroke where the quads already overlap, which
                // is cheaper than working out which is which.
                emit(&[p, [p[0] + n1[0] * half, p[1] + n1[1] * half], [p[0] + n2[0] * half, p[1] + n2[1] * half]]);
                emit(&[p, [p[0] - n1[0] * half, p[1] - n1[1] * half], [p[0] - n2[0] * half, p[1] - n2[1] * half]]);
                continue;
            }
        }
        circle_points_into(p, half, tolerance, scratch);
        emit(scratch);
    }
    if !contour.closed && round_caps {
        circle_points_into(pts[0], half, tolerance, scratch);
        emit(scratch);
        if n > 1 {
            circle_points_into(pts[n - 1], half, tolerance, scratch);
            emit(scratch);
        }
    }
}

/// `stroke_with`, collecting the polygons. For tests; production strokes
/// go through `Rasterizer::add_stroke` and never materialize the list.
#[cfg(test)]
pub(crate) fn stroke(contour: &Contour, width: f32, round_caps: bool, tolerance: f32) -> Vec<Vec<[f32; 2]>> {
    let mut polys = Vec::new();
    stroke_with(contour, width, round_caps, tolerance, &mut Vec::new(), |poly| polys.push(poly.to_vec()));
    polys
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::raster::Rasterizer;

    fn shoelace(points: &[[f32; 2]]) -> f32 {
        let mut area2 = 0.0;
        for i in 0..points.len() {
            let [x0, y0] = points[i];
            let [x1, y1] = points[(i + 1) % points.len()];
            area2 += x0 * y1 - x1 * y0;
        }
        area2.abs() * 0.5
    }

    #[test]
    fn identity_leaves_points_alone() {
        assert_eq!(Transform::identity().apply(3.0, -4.0), [3.0, -4.0]);
    }

    #[test]
    fn a_quarter_turn_maps_plus_x_to_plus_y_in_a_y_down_space() {
        let t = Transform::new(1.0, std::f32::consts::FRAC_PI_2, 0.0, 0.0);
        let [x, y] = t.apply(1.0, 0.0);
        assert!(x.abs() < 1e-6 && (y - 1.0).abs() < 1e-6, "got ({x}, {y})");
    }

    #[test]
    fn scale_and_translate_compose_in_that_order() {
        let t = Transform::new(2.0, 0.0, 10.0, 20.0);
        assert_eq!(t.apply(1.0, 1.0), [12.0, 22.0]);
    }

    #[test]
    fn a_circle_flattens_to_one_closed_contour_with_the_right_area() {
        let circle = Geometry::Circle { cx: 0.0, cy: 0.0, r: 10.0 };
        // 0.02 gives ~56 segments; an inscribed 56-gon is within 0.25% of
        // the true area, so the 0.5% check below has headroom.
        let contours = flatten(&circle, &Transform::identity(), 0.02);
        assert_eq!(contours.len(), 1);
        assert!(contours[0].closed);
        let area = shoelace(&contours[0].points);
        let expected = std::f32::consts::PI * 100.0;
        assert!((area - expected).abs() / expected < 0.005, "area {area} vs {expected}");
    }

    #[test]
    fn a_circle_flattens_through_the_transform() {
        let circle = Geometry::Circle { cx: 1.0, cy: 0.0, r: 1.0 };
        let t = Transform::new(5.0, 0.0, 100.0, 50.0);
        let contours = flatten(&circle, &t, 0.05);
        let xs: Vec<f32> = contours[0].points.iter().map(|p| p[0]).collect();
        let max_x = xs.iter().cloned().fold(f32::MIN, f32::max);
        let min_x = xs.iter().cloned().fold(f32::MAX, f32::min);
        assert!((max_x - 110.0).abs() < 0.1, "max x {max_x}");
        assert!((min_x - 100.0).abs() < 0.1, "min x {min_x}");
    }

    #[test]
    fn a_line_path_keeps_its_vertices_and_close_flag() {
        static TRI: [PathCmd; 4] =
            [PathCmd::Move(0.0, 0.0), PathCmd::Line(1.0, 0.0), PathCmd::Line(0.0, 1.0), PathCmd::Close];
        let contours = flatten(&Geometry::Path(&TRI), &Transform::new(2.0, 0.0, 0.0, 0.0), 0.1);
        assert_eq!(contours.len(), 1);
        assert!(contours[0].closed);
        assert_eq!(contours[0].points, vec![[0.0, 0.0], [2.0, 0.0], [0.0, 2.0]]);
    }

    #[test]
    fn an_open_path_is_not_closed() {
        static LINE: [PathCmd; 2] = [PathCmd::Move(0.0, 0.0), PathCmd::Line(1.0, 1.0)];
        let contours = flatten(&Geometry::Path(&LINE), &Transform::identity(), 0.1);
        assert_eq!(contours.len(), 1);
        assert!(!contours[0].closed);
    }

    #[test]
    fn move_starts_a_new_contour() {
        static TWO: [PathCmd; 4] = [
            PathCmd::Move(0.0, 0.0),
            PathCmd::Line(1.0, 0.0),
            PathCmd::Move(5.0, 5.0),
            PathCmd::Line(6.0, 5.0),
        ];
        let contours = flatten(&Geometry::Path(&TWO), &Transform::identity(), 0.1);
        assert_eq!(contours.len(), 2);
    }

    #[test]
    fn a_cubic_ends_exactly_at_its_end_point_and_bows_toward_its_controls() {
        static ARC: [PathCmd; 2] =
            [PathCmd::Move(0.0, 0.0), PathCmd::Cubic(0.0, 10.0, 10.0, 10.0, 10.0, 0.0)];
        let contours = flatten(&Geometry::Path(&ARC), &Transform::identity(), 0.1);
        let pts = &contours[0].points;
        assert_eq!(*pts.last().unwrap(), [10.0, 0.0]);
        assert!(pts.len() > 4, "a 10-unit bow at 0.1 tolerance needs several segments");
        let max_y = pts.iter().map(|p| p[1]).fold(f32::MIN, f32::max);
        assert!((max_y - 7.5).abs() < 0.2, "cubic midpoint y should be 7.5, got {max_y}");
    }

    #[test]
    fn tighter_tolerance_means_more_segments() {
        let circle = Geometry::Circle { cx: 0.0, cy: 0.0, r: 10.0 };
        let coarse = flatten(&circle, &Transform::identity(), 1.0)[0].points.len();
        let fine = flatten(&circle, &Transform::identity(), 0.01)[0].points.len();
        assert!(fine > coarse, "fine {fine} vs coarse {coarse}");
    }

    #[test]
    fn drawing_after_close_continues_from_the_subpath_start() {
        // Canvas semantics: closePath moves the current point back to the
        // subpath's start, so a lineTo after it draws from there.
        static CMDS: [PathCmd; 5] = [
            PathCmd::Move(0.0, 0.0),
            PathCmd::Line(4.0, 0.0),
            PathCmd::Line(4.0, 4.0),
            PathCmd::Close,
            PathCmd::Line(0.0, 9.0),
        ];
        let contours = flatten(&Geometry::Path(&CMDS), &Transform::identity(), 0.1);
        assert_eq!(contours.len(), 2);
        assert!(contours[0].closed);
        assert!(!contours[1].closed);
        assert_eq!(contours[1].points, vec![[0.0, 0.0], [0.0, 9.0]]);
    }

    /// Rasterizes stroke polygons so the tests measure what a pixel sees.
    fn stroke_coverage(contour: &Contour, width: f32, round_caps: bool) -> Vec<f32> {
        let mut r = Rasterizer::new();
        r.resize(40, 40);
        for poly in stroke(contour, width, round_caps, 0.05) {
            r.add_polygon(&poly);
        }
        let mut cov = Vec::new();
        r.coverage_into(&mut cov);
        cov
    }

    fn stroke_area(contour: &Contour, width: f32, round_caps: bool) -> f32 {
        stroke_coverage(contour, width, round_caps).iter().sum()
    }

    #[test]
    fn a_butt_capped_segment_covers_length_times_width() {
        let seg = Contour { points: vec![[10.0, 20.0], [30.0, 20.0]], closed: false };
        let area = stroke_area(&seg, 4.0, false);
        assert!((area - 80.0).abs() < 0.5, "area {area}");
    }

    #[test]
    fn round_caps_add_one_full_disc() {
        let seg = Contour { points: vec![[10.0, 20.0], [30.0, 20.0]], closed: false };
        let area = stroke_area(&seg, 4.0, true);
        let expected = 80.0 + std::f32::consts::PI * 4.0;
        // The caps are 16-gons at this tolerance, ~2.5% under a true disc.
        assert!((area - expected).abs() < 1.0, "area {area} vs {expected}");
    }

    #[test]
    fn a_closed_square_strokes_as_a_frame_with_no_gaps_at_the_corners() {
        let square = Contour {
            points: vec![[10.0, 10.0], [30.0, 10.0], [30.0, 30.0], [10.0, 30.0]],
            closed: true,
        };
        let area = stroke_area(&square, 2.0, false);
        // Four 20x2 sides = 160, corners are round joins (slightly under a
        // square corner's 4 x 1 = 4). Must be well above 156 (gaps) and at
        // most 160.
        assert!(area > 158.0 && area <= 160.5, "area {area}");
    }

    #[test]
    fn a_polyline_join_has_no_notch() {
        // A right-angle elbow at (20, 20). The two segment quads end flush at
        // the joint, leaving a wedge on the outside of the bend (toward +y
        // here) that only the join disc covers. Half of the pixel at the
        // joint lies in that wedge, so without a join it reads ~0.5; the
        // radius-2 disc covers all of it.
        let bend = Contour { points: vec![[5.0, 5.0], [20.0, 20.0], [35.0, 5.0]], closed: false };
        let cov = stroke_coverage(&bend, 4.0, false);
        let joint = cov[20 * 40 + 20];
        assert!(joint > 0.99, "joint pixel coverage {joint}; the join disc is missing");
        let far = cov[24 * 40 + 20];
        assert!(far < 0.1, "pixel 4px past the joint should be outside the stroke, got {far}");
    }

    #[test]
    fn a_gentle_turn_is_bevelled_and_a_sharp_one_stays_round() {
        // A 10 degree turn at 4px width: the bevel is within 0.02px of the
        // arc, so it is used and costs two triangles. A right angle at the
        // same width would fall 0.6px short, so it gets the disc.
        let gentle = Contour { points: vec![[5.0, 20.0], [20.0, 20.0], [34.8, 22.6]], closed: false };
        let sharp = Contour { points: vec![[5.0, 5.0], [20.0, 20.0], [35.0, 5.0]], closed: false };
        let polys = |c: &Contour| stroke(c, 4.0, false, 0.05);
        assert!(polys(&gentle).iter().skip(2).all(|p| p.len() == 3), "gentle join should be two triangles");
        assert!(polys(&sharp).iter().skip(2).any(|p| p.len() > 8), "sharp join should be a disc");
        // And the bevelled stroke still has no notch at the joint.
        let cov = stroke_coverage(&gentle, 4.0, false);
        let joint = cov[20 * 40 + 20];
        assert!(joint > 0.99, "joint pixel coverage {joint}");
    }

    #[test]
    fn a_zero_width_stroke_produces_nothing() {
        let seg = Contour { points: vec![[10.0, 20.0], [30.0, 20.0]], closed: false };
        assert!(stroke(&seg, 0.0, true, 0.05).is_empty());
    }

    #[test]
    fn circle_polygon_has_the_right_area() {
        let poly = circle_polygon([5.0, 5.0], 3.0, 0.02);
        let area = shoelace(&poly);
        let expected = std::f32::consts::PI * 9.0;
        assert!((area - expected).abs() / expected < 0.01, "area {area}");
    }
}
