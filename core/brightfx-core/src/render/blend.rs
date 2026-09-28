//! The four canvas compositing modes on premultiplied RGBA, and the pixel
//! read-modify-write into the RGBA8 frame.

use crate::schema::BlendMode;

/// Blends premultiplied `src` onto premultiplied `dst`.
#[inline(always)]
pub(crate) fn blend(mode: BlendMode, s: [f32; 4], d: [f32; 4]) -> [f32; 4] {
    let (sa, da) = (s[3], d[3]);
    match mode {
        BlendMode::SourceOver => [
            s[0] + d[0] * (1.0 - sa),
            s[1] + d[1] * (1.0 - sa),
            s[2] + d[2] * (1.0 - sa),
            sa + da * (1.0 - sa),
        ],
        BlendMode::Lighter => [
            (s[0] + d[0]).min(1.0),
            (s[1] + d[1]).min(1.0),
            (s[2] + d[2]).min(1.0),
            (sa + da).min(1.0),
        ],
        BlendMode::Screen => [
            s[0] + d[0] - s[0] * d[0],
            s[1] + d[1] - s[1] * d[1],
            s[2] + d[2] - s[2] * d[2],
            sa + da - sa * da,
        ],
        BlendMode::ColorDodge => {
            // W3C compositing: Co = Cs'(1 - ab) + Cb'(1 - as) + as*ab*B(Cb, Cs)
            // with B evaluated on unpremultiplied channels.
            let mut out = [0.0; 4];
            for i in 0..3 {
                let cs = if sa > 0.0 { s[i] / sa } else { 0.0 };
                let cb = if da > 0.0 { d[i] / da } else { 0.0 };
                let b = if cb <= 0.0 {
                    0.0
                } else if cs >= 1.0 {
                    1.0
                } else {
                    (cb / (1.0 - cs)).min(1.0)
                };
                out[i] = s[i] * (1.0 - da) + d[i] * (1.0 - sa) + sa * da * b;
            }
            out[3] = sa + da * (1.0 - sa);
            out
        }
    }
}

/// A blend mode as a type, so a stamp loop instantiated for one mode
/// compiles to that mode's arithmetic alone instead of a per-pixel match.
/// `dispatch_blend!` picks the type from a runtime `BlendMode` once per
/// particle.
pub(crate) trait Blender {
    fn blend(s: [f32; 4], d: [f32; 4]) -> [f32; 4];
}

pub(crate) struct SourceOver;
pub(crate) struct Lighter;
pub(crate) struct Screen;
pub(crate) struct ColorDodge;

macro_rules! blender {
    ($name:ident, $mode:expr) => {
        impl Blender for $name {
            #[inline(always)]
            fn blend(s: [f32; 4], d: [f32; 4]) -> [f32; 4] {
                blend($mode, s, d)
            }
        }
    };
}
blender!(SourceOver, BlendMode::SourceOver);
blender!(Lighter, BlendMode::Lighter);
blender!(Screen, BlendMode::Screen);
blender!(ColorDodge, BlendMode::ColorDodge);

/// Runs `$body` with `$B` bound to the `Blender` type for `$mode`.
macro_rules! dispatch_blend {
    ($mode:expr, $B:ident => $body:expr) => {
        match $mode {
            $crate::schema::BlendMode::SourceOver => {
                type $B = $crate::render::blend::SourceOver;
                $body
            }
            $crate::schema::BlendMode::Lighter => {
                type $B = $crate::render::blend::Lighter;
                $body
            }
            $crate::schema::BlendMode::Screen => {
                type $B = $crate::render::blend::Screen;
                $body
            }
            $crate::schema::BlendMode::ColorDodge => {
                type $B = $crate::render::blend::ColorDodge;
                $body
            }
        }
    };
}
pub(crate) use dispatch_blend;

/// Blends `src` into the frame pixel `dst`, rounding to the nearest byte.
/// Clamps so a mode can never write out of range.
///
/// This is the innermost call of every stamp loop, a few million times per
/// frame at the benchmark's particle count, so it is written to inline and
/// takes the pixel as a fixed-size array so there is no slice range check.
#[inline(always)]
pub(crate) fn composite_pixel<B: Blender>(src: [f32; 4], dst: &mut [u8; 4]) {
    let d = [
        dst[0] as f32 / 255.0,
        dst[1] as f32 / 255.0,
        dst[2] as f32 / 255.0,
        dst[3] as f32 / 255.0,
    ];
    let o = B::blend(src, d);
    for i in 0..4 {
        dst[i] = (o[i].clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 4], b: [f32; 4]) -> bool {
        a.iter().zip(b.iter()).all(|(x, y)| (x - y).abs() < 1e-5)
    }

    const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
    const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];
    const HALF_RED: [f32; 4] = [0.5, 0.0, 0.0, 0.5];
    const CLEAR: [f32; 4] = [0.0; 4];

    #[test]
    fn every_mode_over_a_transparent_backdrop_is_the_source() {
        for mode in [BlendMode::SourceOver, BlendMode::Lighter, BlendMode::Screen, BlendMode::ColorDodge] {
            assert!(close(blend(mode, HALF_RED, CLEAR), HALF_RED), "{mode:?}");
        }
    }

    #[test]
    fn source_over_replaces_with_an_opaque_source_and_mixes_with_a_translucent_one() {
        assert!(close(blend(BlendMode::SourceOver, RED, GREEN), RED));
        assert!(close(blend(BlendMode::SourceOver, HALF_RED, GREEN), [0.5, 0.5, 0.0, 1.0]));
    }

    #[test]
    fn lighter_adds_and_clamps() {
        assert!(close(blend(BlendMode::Lighter, HALF_RED, HALF_RED), [1.0, 0.0, 0.0, 1.0]));
        assert!(close(blend(BlendMode::Lighter, RED, RED), RED));
        assert!(close(blend(BlendMode::Lighter, RED, GREEN), [1.0, 1.0, 0.0, 1.0]));
    }

    #[test]
    fn screen_of_two_halves_is_three_quarters() {
        let grey = [0.5, 0.5, 0.5, 1.0];
        assert!(close(blend(BlendMode::Screen, grey, grey), [0.75, 0.75, 0.75, 1.0]));
    }

    #[test]
    fn color_dodge_brightens_a_backdrop_by_the_source() {
        let backdrop = [0.25, 0.25, 0.25, 1.0];
        let source = [0.5, 0.5, 0.5, 1.0];
        // 0.25 / (1 - 0.5) = 0.5
        assert!(close(blend(BlendMode::ColorDodge, source, backdrop), [0.5, 0.5, 0.5, 1.0]));
        // A zero backdrop channel stays zero; a full source channel saturates.
        assert!(close(blend(BlendMode::ColorDodge, RED, backdrop), [1.0, 0.25, 0.25, 1.0]));
        assert!(close(blend(BlendMode::ColorDodge, source, [0.0, 0.0, 0.0, 1.0]), [0.0, 0.0, 0.0, 1.0]));
    }

    #[test]
    fn composite_pixel_rounds_to_bytes_and_reads_the_existing_pixel() {
        let mut px = [0u8, 255, 0, 255];
        composite_pixel::<SourceOver>(HALF_RED, &mut px);
        assert_eq!(px, [128, 128, 0, 255]);
    }

    #[test]
    fn composite_pixel_never_overflows() {
        let mut px = [250u8, 250, 250, 250];
        composite_pixel::<Lighter>([1.0, 1.0, 1.0, 1.0], &mut px);
        assert_eq!(px, [255, 255, 255, 255]);
    }
}
