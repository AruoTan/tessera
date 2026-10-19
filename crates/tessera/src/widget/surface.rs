/// Give only fully transparent background pixels a hit-testable alpha of 1.
/// Preserve DirectWrite's premultiplied colors and antialiased coverage verbatim.
pub fn make_hit_testable(pixels: &mut [u32]) {
    for pixel in pixels {
        if *pixel >> 24 == 0 {
            *pixel = 0x0100_0000;
        }
    }
}

/// Composite a tinted coverage sample onto a premultiplied BGRA surface.
pub fn over_mask(target: &mut u32, color: u32, alpha: u8) {
    let alpha = u32::from(alpha);
    let inverse = 255 - alpha;
    let red = (color & 255) * alpha / 255 + (*target >> 16 & 255) * inverse / 255;
    let green = (color >> 8 & 255) * alpha / 255 + (*target >> 8 & 255) * inverse / 255;
    let blue = (color >> 16 & 255) * alpha / 255 + (*target & 255) * inverse / 255;
    let alpha = alpha + (*target >> 24) * inverse / 255;
    *target = (alpha << 24) | (red << 16) | (green << 8) | blue;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_space_remains_draggable_and_rendered_text_keeps_its_coverage_and_color() {
        let mut pixels = [0, 0x8000_0000, 0x8080_8080, 0xff00_0000, 0xffff_ffff];
        make_hit_testable(&mut pixels);
        assert_eq!(
            pixels,
            [
                0x0100_0000,
                0x8000_0000,
                0x8080_8080,
                0xff00_0000,
                0xffff_ffff
            ]
        );
    }

    #[test]
    fn tinted_icons_composite_without_recoloring_existing_text() {
        let mut pixel = 0;
        over_mask(&mut pixel, 0x0000_00ff, 128);
        assert_eq!(pixel, 0x8080_0000);
        over_mask(&mut pixel, 0x00ff_0000, 255);
        assert_eq!(pixel, 0xff00_00ff);
    }
}
