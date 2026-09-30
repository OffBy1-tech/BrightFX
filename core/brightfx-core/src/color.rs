/// Parses a `#rgb` or `#rrggbb` hex color string into normalized 0..1 RGB.
/// Falls back to Mouseflare's existing fallback color (#f59e0b) on parse
/// failure, matching `hexToRgb`'s behavior in `customFxRenderer.ts`.
pub(crate) fn hex_to_rgb(hex: &str) -> [f32; 3] {
    let clean = hex.trim_start_matches('#');
    if !clean.is_ascii() {
        // Byte-index slicing below assumes 1 byte == 1 hex digit; a
        // multi-byte UTF-8 character would land on a non-char-boundary and
        // panic. Malformed config values must be tolerated, never panic.
        return [245.0 / 255.0, 158.0 / 255.0, 11.0 / 255.0];
    }
    let (r, g, b) = if clean.len() == 3 {
        let ch = |i: usize| -> Option<u8> {
            let c = &clean[i..i + 1];
            u8::from_str_radix(&c.repeat(2), 16).ok()
        };
        match (ch(0), ch(1), ch(2)) {
            (Some(r), Some(g), Some(b)) => (r, g, b),
            _ => (245, 158, 11),
        }
    } else if clean.len() == 6 {
        let byte = |i: usize| -> Option<u8> { u8::from_str_radix(&clean[i..i + 2], 16).ok() };
        match (byte(0), byte(2), byte(4)) {
            (Some(r), Some(g), Some(b)) => (r, g, b),
            _ => (245, 158, 11),
        }
    } else {
        (245, 158, 11)
    };
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0]
}

pub(crate) fn interpolate_hex(c1: &str, c2: &str, factor: f32) -> [f32; 3] {
    let f = factor.clamp(0.0, 1.0);
    let a = hex_to_rgb(c1);
    let b = hex_to_rgb(c2);
    [
        a[0] + (b[0] - a[0]) * f,
        a[1] + (b[1] - a[1]) * f,
        a[2] + (b[2] - a[2]) * f,
    ]
}

/// A parsed, sorted multi-stop gradient: `(offset, rgb)` pairs with offsets
/// ascending in [0, 1]. Built once per config load, sampled per particle.
pub(crate) type Palette = Vec<(f32, [f32; 3])>;

/// The stop a `palette_pick` in [0, 1) lands on in a palette of `len`
/// stops, each an equal share of the unit range. Clamped, so a pick of
/// exactly 1.0 (or rounding just below it) still lands on the last stop.
/// `len` must be non-zero.
pub(crate) fn palette_index(pick: f32, len: usize) -> usize {
    ((pick * len as f32) as usize).min(len - 1)
}

/// Samples a `Palette` at `t` in [0, 1]. Holds the first stop's color
/// before its offset and the last stop's color after it, and linearly
/// interpolates between neighbors. `stops` must be non-empty.
pub(crate) fn sample_palette(stops: &[(f32, [f32; 3])], t: f32) -> [f32; 3] {
    let (first, last) = (&stops[0], &stops[stops.len() - 1]);
    if t <= first.0 {
        return first.1;
    }
    if t >= last.0 {
        return last.1;
    }
    let upper = stops.iter().position(|(offset, _)| *offset > t).unwrap_or(stops.len() - 1);
    let (o1, a) = stops[upper - 1];
    let (o2, b) = stops[upper];
    let span = o2 - o1;
    let f = if span > 0.0 { (t - o1) / span } else { 1.0 };
    [
        a[0] + (b[0] - a[0]) * f,
        a[1] + (b[1] - a[1]) * f,
        a[2] + (b[2] - a[2]) * f,
    ]
}

/// h in degrees [0, 360), s and l in [0, 1].
pub(crate) fn hsl_to_rgb(h: f32, s: f32, l: f32) -> [f32; 3] {
    // Normalize once up front so the sector-selection branches below (which
    // use raw comparisons against 1..5) and `x`'s rem_euclid both agree on
    // the same [0, 360) hue -- otherwise a negative or >360 hue picks the
    // wrong RGB sector even though `x` wraps correctly.
    let h = h.rem_euclid(360.0);
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h / 60.0;
    let x = c * (1.0 - (hp.rem_euclid(2.0) - 1.0).abs());
    let (r1, g1, b1) = if hp < 1.0 {
        (c, x, 0.0)
    } else if hp < 2.0 {
        (x, c, 0.0)
    } else if hp < 3.0 {
        (0.0, c, x)
    } else if hp < 4.0 {
        (0.0, x, c)
    } else if hp < 5.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    let m = l - c / 2.0;
    [r1 + m, g1 + m, b1 + m]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_palette_pick_covers_every_stop_and_never_runs_past_the_last() {
        assert_eq!(palette_index(0.0, 3), 0);
        assert_eq!(palette_index(0.34, 3), 1);
        assert_eq!(palette_index(0.67, 3), 2);
        assert_eq!(palette_index(0.999_999_9, 3), 2);
        assert_eq!(palette_index(1.0, 3), 2, "a pick of 1.0 is clamped onto the last stop");
        assert_eq!(palette_index(0.5, 1), 0);
        let hits: std::collections::BTreeSet<usize> =
            (0..1000).map(|i| palette_index(i as f32 / 1000.0, 7)).collect();
        assert_eq!(hits, (0..7).collect(), "every stop is reachable");
    }

    #[test]
    fn hex_to_rgb_parses_six_digit_hex() {
        let rgb = hex_to_rgb("#ff0000");
        assert!((rgb[0] - 1.0).abs() < 1e-6);
        assert!((rgb[1] - 0.0).abs() < 1e-6);
        assert!((rgb[2] - 0.0).abs() < 1e-6);
    }

    #[test]
    fn hex_to_rgb_parses_three_digit_hex() {
        let rgb = hex_to_rgb("#0f0");
        assert!((rgb[0] - 0.0).abs() < 1e-6);
        assert!((rgb[1] - 1.0).abs() < 1e-6);
        assert!((rgb[2] - 0.0).abs() < 1e-6);
    }

    #[test]
    fn interpolate_hex_at_zero_and_one_matches_endpoints() {
        let start = interpolate_hex("#ff0000", "#00ff00", 0.0);
        let end = interpolate_hex("#ff0000", "#00ff00", 1.0);
        assert_eq!(start, hex_to_rgb("#ff0000"));
        assert_eq!(end, hex_to_rgb("#00ff00"));
    }

    #[test]
    fn hsl_red_matches_expected_rgb() {
        let rgb = hsl_to_rgb(0.0, 1.0, 0.5);
        assert!((rgb[0] - 1.0).abs() < 1e-5);
        assert!((rgb[1] - 0.0).abs() < 1e-5);
        assert!((rgb[2] - 0.0).abs() < 1e-5);
    }

    #[test]
    fn hex_to_rgb_falls_back_on_malformed_six_digit() {
        // #gggggg is malformed (g is not a valid hex digit)
        let rgb = hex_to_rgb("#gggggg");
        let fallback = hex_to_rgb("#f59e0b");
        assert_eq!(rgb, fallback, "malformed 6-digit hex should fall back to amber");
    }

    #[test]
    fn hex_to_rgb_falls_back_on_malformed_three_digit() {
        // #zzz is malformed (z is not a valid hex digit)
        let rgb = hex_to_rgb("#zzz");
        let fallback = hex_to_rgb("#f59e0b");
        assert_eq!(rgb, fallback, "malformed 3-digit hex should fall back to amber");
    }

    #[test]
    fn hex_to_rgb_falls_back_on_over_length() {
        // #1234567 is 7 digits (too long)
        let rgb = hex_to_rgb("#1234567");
        let fallback = hex_to_rgb("#f59e0b");
        assert_eq!(rgb, fallback, "hex string > 6 digits should fall back to amber");
    }

    #[test]
    fn hex_to_rgb_falls_back_instead_of_panicking_on_non_ascii() {
        // "é" is a 2-byte UTF-8 char, so byte-index slicing would land on a
        // non-char-boundary and panic if not guarded.
        let rgb = hex_to_rgb("#é0");
        let fallback = hex_to_rgb("#f59e0b");
        assert_eq!(rgb, fallback, "non-ASCII hex input should fall back to amber, not panic");
    }

    #[test]
    fn hex_to_rgb_falls_back_instead_of_panicking_on_non_ascii_at_six_digit_length() {
        // "é" is 2 bytes in UTF-8 landing at an odd byte offset here, so
        // "1é234" is 6 *bytes* total (matching the 6-digit branch's
        // byte-length check) while the multi-byte char straddles one of
        // the 2-byte slice boundaries the old code assumed -- exactly the
        // mismatch that made the byte-slicing panic.
        let rgb = hex_to_rgb("#1é234");
        let fallback = hex_to_rgb("#f59e0b");
        assert_eq!(rgb, fallback, "non-ASCII hex input at the 6-digit boundary should fall back, not panic");
    }

    #[test]
    fn hsl_to_rgb_normalizes_negative_hue_to_match_equivalent_positive_hue() {
        let negative = hsl_to_rgb(-10.0, 1.0, 0.5);
        let positive = hsl_to_rgb(350.0, 1.0, 0.5);
        assert_eq!(negative, positive, "a negative hue should wrap to its positive equivalent");
    }
}
