//! Small lock/unlock vectors for the native context menu; no font glyph dependency.
use tiny_skia::{FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Stroke, Transform};

/// An open shackle indicates the unlock action; the closed lock indicates locking.
fn rasterize(unlock: bool, size: u32) -> Option<Pixmap> {
    let mut image = Pixmap::new(size, size)?;
    let mut path = PathBuilder::new();
    // Rounded lock body, with enough transparent padding for the menu gutter.
    path.move_to(7.0, 11.0);
    path.line_to(17.0, 11.0);
    path.quad_to(19.0, 11.0, 19.0, 13.0);
    path.line_to(19.0, 19.0);
    path.quad_to(19.0, 21.0, 17.0, 21.0);
    path.line_to(7.0, 21.0);
    path.quad_to(5.0, 21.0, 5.0, 19.0);
    path.line_to(5.0, 13.0);
    path.quad_to(5.0, 11.0, 7.0, 11.0);
    path.close();
    path.move_to(7.5, 11.0);
    path.line_to(7.5, 7.5);
    path.cubic_to(7.5, 1.5, 16.5, 1.5, 16.5, 7.5);
    if !unlock {
        path.line_to(16.5, 11.0);
    }
    path.move_to(12.0, 15.0);
    path.line_to(12.0, 17.5);
    let mut paint = Paint::default();
    paint.set_color_rgba8(0, 0, 0, 255);
    let transform = Transform::from_scale(size as f32 / 24.0, size as f32 / 24.0);
    image.stroke_path(
        &path.finish()?,
        &paint,
        &Stroke {
            width: 1.8,
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Default::default()
        },
        transform,
        None,
    );
    image.fill_path(
        &PathBuilder::from_circle(12.0, 14.5, 1.0)?,
        &paint,
        FillRule::Winding,
        transform,
        None,
    );
    Some(image)
}

#[cfg(windows)]
pub(super) use native::MenuIcon;

#[cfg(windows)]
mod native {
    use super::rasterize;
    use std::{mem::size_of, ptr::null_mut};
    use windows_sys::Win32::Graphics::Gdi::*;

    pub struct MenuIcon(HBITMAP);

    impl MenuIcon {
        pub fn new(unlock: bool, size: u32, color: u32) -> Option<Self> {
            let image = rasterize(unlock, size)?;
            // SAFETY: the top-down 32-bit DIB allocation matches the pixmap. The
            // owned bitmap stays alive during menu tracking and is freed by Drop.
            unsafe {
                let info = BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: size as i32,
                        biHeight: -(size as i32),
                        biPlanes: 1,
                        biBitCount: 32,
                        biCompression: BI_RGB,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                let mut bits = null_mut();
                let bitmap =
                    CreateDIBSection(null_mut(), &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
                if bitmap.is_null() {
                    return None;
                }
                let icon = Self(bitmap);
                if bits.is_null() {
                    return None;
                }
                let pixels =
                    std::slice::from_raw_parts_mut(bits.cast::<u32>(), image.pixels().len());
                let (red, green, blue) = (color & 255, (color >> 8) & 255, (color >> 16) & 255);
                for (pixel, mask) in pixels.iter_mut().zip(image.pixels()) {
                    let alpha = u32::from(mask.alpha());
                    *pixel = (alpha << 24)
                        | ((red * alpha / 255) << 16)
                        | ((green * alpha / 255) << 8)
                        | (blue * alpha / 255);
                }
                Some(icon)
            }
        }

        pub fn handle(&self) -> HBITMAP {
            self.0
        }
    }

    impl Drop for MenuIcon {
        fn drop(&mut self) {
            // SAFETY: menu tracking has finished and this guard owns the bitmap.
            unsafe {
                DeleteObject(self.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_actions_have_distinct_unclipped_antialiased_icons_at_each_dpi() {
        for size in [16, 20, 24, 28, 32, 48, 64] {
            let locked = rasterize(false, size).unwrap();
            let unlocked = rasterize(true, size).unwrap();
            assert_ne!(locked.data(), unlocked.data());
            for image in [locked, unlocked] {
                assert!(
                    image
                        .pixels()
                        .iter()
                        .any(|p| p.alpha() > 0 && p.alpha() < 255)
                );
                assert!(image.pixels().iter().any(|p| p.alpha() >= 240));
                let width = size as usize;
                for index in 0..width {
                    for edge in [
                        index,
                        (width - 1) * width + index,
                        index * width,
                        index * width + width - 1,
                    ] {
                        assert_eq!(image.pixels()[edge].alpha(), 0, "clipped at size {size}");
                    }
                }
            }
        }
    }
}
