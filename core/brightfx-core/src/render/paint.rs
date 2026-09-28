//! What a layer is painted with. Everything evaluates to premultiplied RGBA
//! at full coverage; the caller scales by coverage and particle alpha.

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum GradientColor {
    White,
    Tint,
    Transparent,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Paint {
    /// The particle's color.
    Tint,
    /// Fixed white at this alpha, for cores, facets, and glints.
    White(f32),
    /// Radial gradient centered on the shape origin. `radius` is in shape
    /// units; stops are (offset in `0..=1`, color) ascending.
    Radial { radius: f32, stops: &'static [(f32, GradientColor)] },
}

fn stop_color(color: GradientColor, tint: [f32; 3]) -> [f32; 4] {
    match color {
        GradientColor::White => [1.0, 1.0, 1.0, 1.0],
        GradientColor::Tint => [tint[0], tint[1], tint[2], 1.0],
        GradientColor::Transparent => [0.0, 0.0, 0.0, 0.0],
    }
}

/// Holds the first stop before its offset and the last after, and lerps
/// premultiplied color between neighbors, so a fade to transparent never
/// darkens.
fn radial_at(stops: &[(f32, GradientColor)], t: f32, tint: [f32; 3]) -> [f32; 4] {
    let Some(first) = stops.first().copied() else {
        return [0.0, 0.0, 0.0, 0.0];
    };
    if !t.is_finite() {
        return stop_color(first.1, tint);
    }
    let last = stops[stops.len() - 1];
    if t <= first.0 {
        return stop_color(first.1, tint);
    }
    if t >= last.0 {
        return stop_color(last.1, tint);
    }
    let upper = stops.iter().position(|(offset, _)| *offset > t).unwrap_or(stops.len() - 1);
    let (o1, c1) = stops[upper - 1];
    let (o2, c2) = stops[upper];
    let a = stop_color(c1, tint);
    let b = stop_color(c2, tint);
    let span = o2 - o1;
    let f = if span > 0.0 { (t - o1) / span } else { 1.0 };
    [
        a[0] + (b[0] - a[0]) * f,
        a[1] + (b[1] - a[1]) * f,
        a[2] + (b[2] - a[2]) * f,
        a[3] + (b[3] - a[3]) * f,
    ]
}

impl Paint {
    /// Premultiplied color at full coverage. `t` is the pixel's distance
    /// from the shape origin divided by the layer's radius, used only by
    /// `Radial`.
    pub fn color_at(&self, tint: [f32; 3], t: f32) -> [f32; 4] {
        match *self {
            Paint::Tint => [tint[0], tint[1], tint[2], 1.0],
            Paint::White(a) => [a, a, a, a],
            Paint::Radial { stops, .. } => radial_at(stops, t, tint),
        }
    }

    pub fn alpha_at(&self, t: f32) -> f32 {
        self.color_at([0.0, 0.0, 0.0], t)[3]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: [f32; 3] = [1.0, 0.0, 0.0];
    const GLOW: &[(f32, GradientColor)] =
        &[(0.0, GradientColor::White), (0.3, GradientColor::Tint), (1.0, GradientColor::Transparent)];

    fn close(a: [f32; 4], b: [f32; 4]) -> bool {
        a.iter().zip(b.iter()).all(|(x, y)| (x - y).abs() < 1e-5)
    }

    #[test]
    fn tint_is_the_particle_color_at_full_alpha() {
        assert_eq!(Paint::Tint.color_at(RED, 0.0), [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(Paint::Tint.alpha_at(0.7), 1.0);
    }

    #[test]
    fn white_is_premultiplied_by_its_alpha() {
        assert!(close(Paint::White(0.4).color_at(RED, 0.0), [0.4, 0.4, 0.4, 0.4]));
        assert_eq!(Paint::White(0.4).alpha_at(0.0), 0.4);
    }

    #[test]
    fn a_radial_gradient_hits_its_stops_exactly() {
        let p = Paint::Radial { radius: 1.5, stops: GLOW };
        assert!(close(p.color_at(RED, 0.0), [1.0, 1.0, 1.0, 1.0]));
        assert!(close(p.color_at(RED, 0.3), [1.0, 0.0, 0.0, 1.0]));
        assert!(close(p.color_at(RED, 1.0), [0.0, 0.0, 0.0, 0.0]));
    }

    #[test]
    fn a_radial_gradient_interpolates_premultiplied_between_stops() {
        let p = Paint::Radial { radius: 1.5, stops: GLOW };
        // Halfway from tint (0.3) to transparent (1.0): tint at half strength.
        assert!(close(p.color_at(RED, 0.65), [0.5, 0.0, 0.0, 0.5]));
        // Halfway from white to tint.
        assert!(close(p.color_at(RED, 0.15), [1.0, 0.5, 0.5, 1.0]));
    }

    #[test]
    fn a_radial_gradient_holds_its_end_stops_outside_the_range() {
        let p = Paint::Radial { radius: 1.0, stops: GLOW };
        assert!(close(p.color_at(RED, -0.5), [1.0, 1.0, 1.0, 1.0]));
        assert!(close(p.color_at(RED, 7.0), [0.0, 0.0, 0.0, 0.0]));
    }

    #[test]
    fn an_empty_radial_gradient_paints_nothing_instead_of_panicking() {
        let p = Paint::Radial { radius: 1.0, stops: &[] };
        assert_eq!(p.color_at(RED, 0.5), [0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn a_non_finite_t_resolves_to_the_first_stop() {
        // t = distance / radius; the only way to get NaN is 0 / 0, which is
        // the gradient's center, so the first stop is the right answer.
        let p = Paint::Radial { radius: 1.0, stops: GLOW };
        assert!(close(p.color_at(RED, f32::NAN), [1.0, 1.0, 1.0, 1.0]));
        let single = Paint::Radial { radius: 1.0, stops: &[(0.5, GradientColor::Tint)] };
        assert!(close(single.color_at(RED, f32::NAN), [1.0, 0.0, 0.0, 1.0]));
    }
}
