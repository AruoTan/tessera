//! Hollow, directional edge shadow. The menu surface never receives shadow ink.
//! A small nine-grid atlas keeps the same corner radius and falloff at any menu size.

// Visible-card spacing is independent of the reserved shadow gutter.
pub(super) const TASKBAR_GAP: i32 = 12;
const SHADOW_REACH: i32 = 12;
pub(super) const GUTTER: i32 = SHADOW_REACH + 2;
pub(super) const SIZE: usize = 64;
const RADIUS: f64 = 8.0;
// Keep the complete diagonal cap in the unstretched nine-grid corner tiles.
const BEVEL_SOFTNESS: f64 = 2.0;
pub(super) const CAP: f64 = GUTTER as f64 + SHADOW_REACH as f64 + BEVEL_SOFTNESS;
const BLUR_SIGMA: f64 = 4.5;
const OFFSET_Y: f64 = 4.5;
const INTENSITY: f64 = 0.8;

fn distance(x: f64, y: f64) -> f64 {
    let center = SIZE as f64 / 2.0;
    let half_inner = center - GUTTER as f64 - RADIUS;
    let qx = (x - center).abs() - half_inner;
    let qy = (y - center).abs() - half_inner;
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - RADIUS
}

// Normal CDF approximation, used for a soft Gaussian edge rather than a solid
// rectangle or a uniform halo around the menu.
fn normal_cdf(value: f64) -> f64 {
    let z = value.abs();
    let t = 1.0 / (1.0 + 0.231_641_9 * z);
    let tail = (-0.5 * z * z).exp() / (2.0 * std::f64::consts::PI).sqrt()
        * t
        * (0.319_381_530
            + t * (-0.356_563_782
                + t * (1.781_477_937 + t * (-1.821_255_978 + t * 1.330_274_429))));
    if value < 0.0 { tail } else { 1.0 - tail }
}

// Ease the last two DIPs of each tail to complete transparency.
fn fade_out(distance: f64, reach: f64) -> f64 {
    let tail = ((reach - distance) / 2.0).clamp(0.0, 1.0);
    tail * tail * (3.0 - 2.0 * tail)
}

pub(super) fn pixels(dark: bool) -> Vec<u8> {
    let opacity = if dark { 0.36 } else { 0.24 };
    let mut pixels = vec![0; SIZE * SIZE * 4];
    // Leave the entire area above the card transparent. The side shadows start
    // lower as they extend outward, making mirrored 45-degree upper cutoffs.
    for y in GUTTER as usize..SIZE {
        let py = y as f64 + 0.5;
        let from_top = ((py - GUTTER as f64) / RADIUS).clamp(0.0, 1.0);
        let from_top = from_top * from_top * (3.0 - 2.0 * from_top);
        for x in 0..SIZE {
            let px = x as f64 + 0.5;
            let lateral_distance = (GUTTER as f64 - px)
                .max(px - (SIZE as f64 - GUTTER as f64))
                .max(0.0);
            // One DIP downward for each DIP outward gives a 45-degree slope.
            // A short smooth transition softens the cut while retaining its shape.
            let bevel = ((py - GUTTER as f64 - lateral_distance) / BEVEL_SOFTNESS).clamp(0.0, 1.0);
            let bevel = bevel * bevel * (3.0 - 2.0 * bevel);
            // Cut out the ORIGINAL card, not the downward-offset shadow shape.
            // Start at the card edge without expanding the cutout: extra clearance
            // exposes the backdrop as a bright seam. Limit corner antialiasing to
            // half a pixel so the first exterior pixel on each straight edge is full.
            let edge_distance = distance(px, py);
            let outside = (edge_distance * 2.0).clamp(0.0, 1.0);
            if outside == 0.0 {
                continue;
            }
            // Downward light direction: soft sides and a stronger bottom.
            let coverage = normal_cdf(-distance(px, py - OFFSET_Y) / BLUR_SIGMA);
            // The sides keep their wider falloff. The bottom tail ends at the
            // closer taskbar boundary; a separate fade avoids narrowing the sides.
            let below_card = py - (SIZE as f64 - GUTTER as f64);
            let tail = fade_out(edge_distance, SHADOW_REACH as f64)
                * fade_out(below_card, TASKBAR_GAP as f64);
            // Lower the amplitude after quantization so the faintest nonzero
            // tail pixels stay visible at the same positions instead of vanishing.
            let alpha = (coverage * outside * tail * from_top * bevel * opacity * 255.0).round();
            pixels[(y * SIZE + x) * 4 + 3] = (alpha * INTENSITY).round() as u8;
        }
    }
    // Black pixels in premultiplied BGRA; only their alpha channel is nonzero.
    pixels
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha(pixels: &[u8], x: usize, y: usize) -> u8 {
        pixels[(y * SIZE + x) * 4 + 3]
    }

    #[test]
    fn shadow_cannot_paint_or_darken_any_pixel_inside_the_rounded_menu() {
        for dark in [false, true] {
            let pixels = pixels(dark);
            assert_eq!(pixels.len(), SIZE * SIZE * 4);
            for y in 0..SIZE {
                for x in 0..SIZE {
                    let rgba = &pixels[(y * SIZE + x) * 4..][..4];
                    assert_eq!(&rgba[..3], &[0, 0, 0]);
                    if distance(x as f64 + 0.5, y as f64 + 0.5) <= 0.0 {
                        assert_eq!(rgba[3], 0, "shadow ink inside menu at ({x}, {y})");
                    }
                }
            }
        }
    }

    #[test]
    fn sides_are_symmetric_bottom_is_stronger_and_top_stays_clear() {
        for dark in [false, true] {
            let pixels = pixels(dark);
            let center = SIZE / 2;
            let near = GUTTER as usize - 3;
            let far = SIZE - near - 1;
            let top = alpha(&pixels, center, near);
            let side = alpha(&pixels, near, center);
            let bottom = alpha(&pixels, center, far);
            assert_eq!(side, alpha(&pixels, far, center));
            assert_eq!(top, 0);
            assert!(side > 0 && side < bottom);
            // The upper cut recedes down by the same distance it recedes outward,
            // while a visible shadow remains below it on both mirrored sides.
            for outward in [6, 10] {
                let left = GUTTER as usize - outward - 1;
                let right = SIZE - left - 1;
                let above_cut = GUTTER as usize + outward - 1;
                let below_cut = GUTTER as usize + outward + 3;
                for x in [left, right] {
                    assert_eq!(alpha(&pixels, x, above_cut), 0);
                    assert!(alpha(&pixels, x, below_cut) > 0);
                }
            }
            for y in 0..GUTTER as usize {
                for x in 0..SIZE {
                    assert_eq!(alpha(&pixels, x, y), 0, "shadow above menu at ({x}, {y})");
                }
            }
        }
    }

    #[test]
    fn shadow_meets_side_and_bottom_edges_without_a_bright_gap() {
        let near = GUTTER as usize - 1;
        let far = SIZE - GUTTER as usize;
        let center = SIZE / 2;
        for dark in [false, true] {
            let pixels = pixels(dark);
            let samples = |step| {
                [
                    alpha(&pixels, near - step, center),
                    alpha(&pixels, far + step, center),
                    alpha(&pixels, center, far + step),
                ]
            };
            let mut previous = samples(0);
            assert!(
                previous.iter().all(|value| *value > 0),
                "transparent seam next to menu"
            );
            for step in 1..GUTTER as usize {
                let current = samples(step);
                for (inner, outer) in previous.into_iter().zip(current) {
                    assert!(outer <= inner, "shadow grows away from card edge");
                }
                previous = current;
            }
        }
    }

    #[test]
    fn falloff_reaches_transparency_before_the_popup_boundary() {
        for dark in [false, true] {
            let pixels = pixels(dark);
            for index in 0..SIZE {
                assert_eq!(alpha(&pixels, index, 0), 0);
                assert_eq!(alpha(&pixels, index, SIZE - 1), 0);
                assert_eq!(alpha(&pixels, 0, index), 0);
                assert_eq!(alpha(&pixels, SIZE - 1, index), 0);
            }
            // The bottom fills the closer taskbar gap; both sides retain the
            // longer tail, independently of the menu's vertical placement.
            let edge = SIZE - GUTTER as usize;
            let bottom_tail = edge + TASKBAR_GAP as usize - 2;
            let bottom_end = edge + TASKBAR_GAP as usize;
            let side_tail = edge + SHADOW_REACH as usize - 2;
            let side_end = edge + SHADOW_REACH as usize;
            let center = SIZE / 2;
            assert!(alpha(&pixels, center, bottom_tail) > 0);
            assert!(alpha(&pixels, side_tail, center) > 0);
            assert!(alpha(&pixels, SIZE - side_tail - 1, center) > 0);
            assert_eq!(alpha(&pixels, center, bottom_end), 0);
            assert_eq!(alpha(&pixels, side_end, center), 0);
            assert_eq!(alpha(&pixels, SIZE - side_end - 1, center), 0);
            // Stretching the center tile must not create a filled background.
            for y in CAP as usize..SIZE - CAP as usize {
                for x in CAP as usize..SIZE - CAP as usize {
                    assert_eq!(alpha(&pixels, x, y), 0);
                }
            }
        }
    }
}
