//! The 13 particle shapes, ported layer for layer from Mouseflare's
//! `customFxRenderer.ts`. Coordinates are in units of the particle size `s`.

use super::paint::{GradientColor, Paint};
use super::path::{Geometry, PathCmd};
use crate::schema::ParticleShape;

pub(crate) enum Op {
    Fill,
    /// `width` in units of `s`; `min_width` in logical pixels, where canvas
    /// used `Math.max(min, s * width)`.
    Stroke { width: f32, min_width: f32, round_caps: bool },
}

pub(crate) struct Layer {
    pub geometry: Geometry,
    pub op: Op,
    pub paint: Paint,
}

pub(crate) struct Shape {
    pub layers: &'static [Layer],
    /// Radius in units of `s` that bounds every layer, stroke included.
    /// Sizes the raster tile and the glow sprite.
    pub extent: f32,
    /// Floor on the particle size in logical pixels, where canvas used
    /// `Math.max(min, s)` on the radius.
    pub min_size: f32,
    /// Every layer is a circle centered on the origin, so the silhouette,
    /// and with it the glow halo, is the same at every rotation. Such a
    /// halo is stamped unrotated, which is much cheaper.
    pub symmetric: bool,
}

#[cfg(test)]
pub(crate) const ALL: [ParticleShape; 13] = [
    ParticleShape::Circle,
    ParticleShape::SparkleStar,
    ParticleShape::GlowDisc,
    ParticleShape::Ring,
    ParticleShape::ShardCrystal,
    ParticleShape::PlasmaOrb,
    ParticleShape::SmokePuff,
    ParticleShape::LightningBolt,
    ParticleShape::Bubble,
    ParticleShape::Heart,
    ParticleShape::SakuraPetal,
    ParticleShape::Diamond,
    ParticleShape::Rune,
];

const fn fill(geometry: Geometry, paint: Paint) -> Layer {
    Layer { geometry, op: Op::Fill, paint }
}

const fn stroke(geometry: Geometry, width: f32, min_width: f32, round_caps: bool, paint: Paint) -> Layer {
    Layer { geometry, op: Op::Stroke { width, min_width, round_caps }, paint }
}

const UNIT_CIRCLE: Geometry = Geometry::Circle { cx: 0.0, cy: 0.0, r: 1.0 };

use GradientColor::{Tint as GTint, Transparent, White as GWhite};
use PathCmd::{Close, Cubic, Line, Move};

static CIRCLE: Shape = Shape {
    extent: 1.0,
    min_size: 0.5,
    symmetric: true,
    layers: &[fill(UNIT_CIRCLE, Paint::Tint)],
};

static GLOW_DISC: Shape = Shape {
    extent: 1.5,
    min_size: 1.0 / 1.5,
    symmetric: true,
    layers: &[fill(
        Geometry::Circle { cx: 0.0, cy: 0.0, r: 1.5 },
        Paint::Radial { radius: 1.5, stops: &[(0.0, GWhite), (0.3, GTint), (1.0, Transparent)] },
    )],
};

static SPARKLE_STAR: Shape = Shape {
    extent: 1.0,
    min_size: 0.0,
    symmetric: false,
    layers: &[fill(
        Geometry::Path(&[
            Move(0.0, -1.0),
            Line(0.25, -0.25),
            Line(1.0, 0.0),
            Line(0.25, 0.25),
            Line(0.0, 1.0),
            Line(-0.25, 0.25),
            Line(-1.0, 0.0),
            Line(-0.25, -0.25),
            Close,
        ]),
        Paint::Tint,
    )],
};

static RING: Shape = Shape {
    extent: 1.125,
    min_size: 0.0,
    symmetric: true,
    layers: &[stroke(UNIT_CIRCLE, 0.25, 1.0, false, Paint::Tint)],
};

static SHARD_CRYSTAL: Shape = Shape {
    extent: 1.2,
    min_size: 0.0,
    symmetric: false,
    layers: &[
        fill(
            Geometry::Path(&[
                Move(0.0, -1.2),
                Line(0.6, -0.2),
                Line(0.4, 1.0),
                Line(-0.4, 1.0),
                Line(-0.6, -0.2),
                Close,
            ]),
            Paint::Tint,
        ),
        // Inner facet highlight
        fill(
            Geometry::Path(&[Move(0.0, -1.2), Line(0.6, -0.2), Line(0.0, 1.0), Close]),
            Paint::White(0.4),
        ),
    ],
};

static PLASMA_ORB: Shape = Shape {
    extent: 1.0,
    min_size: 0.0,
    symmetric: false,
    layers: &[
        fill(UNIT_CIRCLE, Paint::Tint),
        // Core highlight
        fill(Geometry::Circle { cx: -0.25, cy: -0.25, r: 0.35 }, Paint::White(1.0)),
    ],
};

static SMOKE_PUFF: Shape = Shape {
    extent: 1.0,
    min_size: 0.0,
    symmetric: true,
    layers: &[fill(
        UNIT_CIRCLE,
        Paint::Radial { radius: 1.0, stops: &[(0.0, GTint), (0.6, GTint), (1.0, Transparent)] },
    )],
};

static LIGHTNING_BOLT: Shape = Shape {
    extent: 1.3,
    min_size: 0.0,
    symmetric: false,
    layers: &[stroke(
        Geometry::Path(&[Move(-0.6, -1.0), Line(0.0, -0.2), Line(-0.3, 0.0), Line(0.6, 1.0)]),
        0.2,
        1.2,
        true,
        Paint::Tint,
    )],
};

static BUBBLE: Shape = Shape {
    extent: 1.1,
    min_size: 0.0,
    symmetric: false,
    layers: &[
        fill(UNIT_CIRCLE, Paint::White(0.08)),
        stroke(UNIT_CIRCLE, 0.15, 1.0, false, Paint::Tint),
        // Specular glint
        fill(Geometry::Circle { cx: -0.35, cy: -0.35, r: 0.2 }, Paint::White(0.7)),
    ],
};

static SAKURA_PETAL: Shape = Shape {
    extent: 1.0,
    min_size: 0.0,
    symmetric: false,
    layers: &[fill(
        Geometry::Path(&[
            Move(0.0, -1.0),
            Cubic(0.8, -0.8, 0.8, 0.6, 0.0, 1.0),
            Cubic(-0.8, 0.6, -0.8, -0.8, 0.0, -1.0),
            Close,
        ]),
        Paint::Tint,
    )],
};

static RUNE: Shape = Shape {
    extent: 1.1,
    min_size: 0.0,
    symmetric: false,
    layers: &[
        // Outer diamond
        stroke(
            Geometry::Path(&[Move(0.0, -1.0), Line(1.0, 0.0), Line(0.0, 1.0), Line(-1.0, 0.0), Close]),
            0.15,
            1.2,
            false,
            Paint::Tint,
        ),
        // Inner cross
        stroke(
            Geometry::Path(&[Move(0.0, -0.6), Line(0.0, 0.6), Move(-0.6, 0.0), Line(0.6, 0.0)]),
            0.15,
            1.2,
            false,
            Paint::Tint,
        ),
    ],
};

static HEART: Shape = Shape {
    extent: 1.0,
    min_size: 0.0,
    symmetric: false,
    layers: &[fill(
        Geometry::Path(&[
            Move(0.0, 0.4),
            Cubic(-0.8, -0.4, -0.8, -0.9, 0.0, -0.3),
            Cubic(0.8, -0.9, 0.8, -0.4, 0.0, 0.4),
            Close,
        ]),
        Paint::Tint,
    )],
};

static DIAMOND: Shape = Shape {
    extent: 1.0,
    min_size: 0.0,
    symmetric: false,
    layers: &[fill(
        Geometry::Path(&[Move(0.0, -1.0), Line(0.7, 0.0), Line(0.0, 1.0), Line(-0.7, 0.0), Close]),
        Paint::Tint,
    )],
};

pub(crate) fn shape_for(shape: ParticleShape) -> &'static Shape {
    match shape {
        ParticleShape::Circle => &CIRCLE,
        ParticleShape::SparkleStar => &SPARKLE_STAR,
        ParticleShape::GlowDisc => &GLOW_DISC,
        ParticleShape::Ring => &RING,
        ParticleShape::ShardCrystal => &SHARD_CRYSTAL,
        ParticleShape::PlasmaOrb => &PLASMA_ORB,
        ParticleShape::SmokePuff => &SMOKE_PUFF,
        ParticleShape::LightningBolt => &LIGHTNING_BOLT,
        ParticleShape::Bubble => &BUBBLE,
        ParticleShape::Heart => &HEART,
        ParticleShape::SakuraPetal => &SAKURA_PETAL,
        ParticleShape::Diamond => &DIAMOND,
        ParticleShape::Rune => &RUNE,
    }
}

#[cfg(test)]
mod tests {
    use super::super::path::{flatten, Transform};
    use super::*;

    #[test]
    fn every_shape_has_at_least_one_layer() {
        for shape in ALL {
            assert!(!shape_for(shape).layers.is_empty(), "{shape:?} has no layers");
        }
    }

    #[test]
    fn every_layer_fits_inside_its_shape_extent_including_stroke_width() {
        for shape in ALL {
            let def = shape_for(shape);
            for (i, layer) in def.layers.iter().enumerate() {
                let half = match layer.op {
                    Op::Fill => 0.0,
                    Op::Stroke { width, .. } => width * 0.5,
                };
                for contour in flatten(&layer.geometry, &Transform::identity(), 0.01) {
                    for [x, y] in contour.points {
                        let d = (x * x + y * y).sqrt() + half;
                        assert!(
                            d <= def.extent + 1e-3,
                            "{shape:?} layer {i} reaches {d} beyond extent {}",
                            def.extent
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn radial_layers_declare_the_radius_their_geometry_uses() {
        for shape in ALL {
            for layer in shape_for(shape).layers {
                if let Paint::Radial { radius, .. } = layer.paint {
                    match layer.geometry {
                        Geometry::Circle { r, .. } => assert_eq!(r, radius, "{shape:?}"),
                        Geometry::Path(_) => panic!("{shape:?}: radial paint on a path"),
                    }
                }
            }
        }
    }

    #[test]
    fn the_mouseflare_minimums_are_carried() {
        assert_eq!(shape_for(ParticleShape::Circle).min_size, 0.5);
        assert!((shape_for(ParticleShape::GlowDisc).min_size - 1.0 / 1.5).abs() < 1e-6);
        assert_eq!(shape_for(ParticleShape::Diamond).min_size, 0.0);
    }

    #[test]
    fn strokes_carry_their_canvas_line_widths() {
        let ring = &shape_for(ParticleShape::Ring).layers[0];
        assert!(matches!(ring.op, Op::Stroke { width, min_width, round_caps: false } if width == 0.25 && min_width == 1.0));
        let bolt = &shape_for(ParticleShape::LightningBolt).layers[0];
        assert!(matches!(bolt.op, Op::Stroke { width, min_width, round_caps: true } if width == 0.2 && min_width == 1.2));
    }

    #[test]
    fn every_radial_gradient_has_at_least_two_stops() {
        for shape in ALL {
            for layer in shape_for(shape).layers {
                if let Paint::Radial { stops, .. } = layer.paint {
                    assert!(stops.len() >= 2, "{shape:?} has a radial paint with {} stops", stops.len());
                }
            }
        }
    }
}
