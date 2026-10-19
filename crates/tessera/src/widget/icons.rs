//! Build-time SVG paths, rasterized only when the taskbar DPI changes.
use tiny_skia::{FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Stroke, Transform};

include!(concat!(env!("OUT_DIR"), "/icons.rs"));

pub fn icon_size(dpi: u32) -> u32 {
    (16.0 * f64::from(dpi.max(96)) / 96.0).round() as u32
}

pub struct Icons {
    size: u32,
    masks: [Vec<u8>; 2],
}

impl Icons {
    pub fn new(dpi: u32) -> Option<Self> {
        let size = icon_size(dpi);
        Some(Self {
            size,
            masks: [rasterize_upload(size)?, rasterize_download(size)?],
        })
    }

    /// Composite SVG coverage directly into the premultiplied text surface.
    pub fn paint(&self, pixels: &mut [u32], width: usize, height: usize, left: usize, color: u32) {
        assert_eq!(pixels.len(), width * height);
        let size = self.size as usize;
        for (row, mask) in self.masks.iter().enumerate() {
            let row_top = row * (height / 2);
            let row_bottom = if row == 0 { height / 2 } else { height };
            let top = row_top + (row_bottom - row_top).saturating_sub(size) / 2;
            for y in 0..size.min(row_bottom - top) {
                for x in 0..size.min(width.saturating_sub(left)) {
                    let coverage = mask[y * size + x];
                    if coverage != 0 {
                        super::surface::over_mask(
                            &mut pixels[(top + y) * width + left + x],
                            color,
                            coverage,
                        );
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widget::surface::make_hit_testable;

    #[test]
    fn compiled_paths_match_the_original_svgs_at_each_dpi() {
        for dpi in [96, 120, 144, 168, 192, 216, 288, 384] {
            let icons = Icons::new(dpi).unwrap();
            for (svg, mask) in [
                include_str!("../../assets/icons/cloud-upload.svg"),
                include_str!("../../assets/icons/cloud-download.svg"),
            ]
            .into_iter()
            .zip(&icons.masks)
            {
                let tree = resvg::usvg::Tree::from_str(svg, &Default::default()).unwrap();
                let mut image = Pixmap::new(icons.size, icons.size).unwrap();
                resvg::render(
                    &tree,
                    Transform::from_scale(
                        icons.size as f32 / tree.size().width(),
                        icons.size as f32 / tree.size().height(),
                    ),
                    &mut image.as_mut(),
                );
                let expected: Vec<_> = image.pixels().iter().map(|p| p.alpha()).collect();
                assert_eq!(*mask, expected, "icon changed at {dpi} DPI");
            }
        }
    }

    #[test]
    fn embedded_vectors_are_distinct_antialiased_and_unclipped_at_taskbar_dpis() {
        for dpi in [96, 120, 144, 192, 288] {
            let icons = Icons::new(dpi).expect("embedded SVGs must render");
            let size = icons.size as usize;
            assert_ne!(icons.masks[0], icons.masks[1]);
            for mask in &icons.masks {
                assert_eq!(mask.len(), size * size);
                assert!(mask.iter().any(|&alpha| alpha > 0 && alpha < 255));
                assert!(mask.iter().any(|&alpha| alpha >= 240));
                for index in 0..size {
                    assert_eq!(mask[index], 0, "top edge clipped at {dpi} DPI");
                    assert_eq!(mask[(size - 1) * size + index], 0, "bottom edge clipped");
                    assert_eq!(mask[index * size], 0, "left edge clipped");
                    assert_eq!(mask[index * size + size - 1], 0, "right edge clipped");
                }
            }
        }
    }

    #[test]
    fn icons_use_row_tints_and_leave_blank_space_draggable() {
        let icons = Icons::new(96).unwrap();
        let (width, height) = (80, 41);
        let mut pixels = vec![0; width * height];
        // Already-colored DirectWrite text must survive icon composition.
        pixels[30] = 0x8080_8080;
        icons.paint(&mut pixels, width, height, 4, 0x0000_00ff);
        make_hit_testable(&mut pixels);
        assert_eq!(pixels[30], 0x8080_8080);
        assert_eq!(pixels[0], 0x0100_0000);
        for (row, tint) in [(0, 0x00ff_0000), (1, 0x00ff_0000)] {
            let start = row * (height / 2) * width;
            let end = if row == 0 {
                height / 2 * width
            } else {
                pixels.len()
            };
            assert!(pixels[start..end].iter().any(|&pixel| pixel & tint != 0));
            assert!(
                pixels[start..end]
                    .iter()
                    .filter(|&&pixel| pixel != 0x8080_8080)
                    .all(|&pixel| pixel & !(0xff00_0000 | tint) == 0)
            );
        }
        // A clipped surface must not write outside its row or pixel buffer.
        icons.paint(&mut [0; 6], 2, 3, 0, 0);
        icons.paint(&mut [0; 6], 2, 3, 3, 0);
    }
}
