//! A reusable top-down DIB used directly as the layered window's frame buffer.
use std::{
    mem::size_of,
    ptr::{NonNull, null_mut},
};
use windows_sys::Win32::Graphics::Gdi::*;

pub(super) struct Bitmap {
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    pixels: NonNull<u32>,
    size: (i32, i32),
}

impl Bitmap {
    pub fn new(width: i32, height: i32) -> std::io::Result<Self> {
        let len = usize::try_from(width)
            .ok()
            .zip(usize::try_from(height).ok())
            .and_then(|(width, height)| width.checked_mul(height))
            .filter(|&len| len > 0 && len <= isize::MAX as usize / 4);
        if len.is_none() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Invalid bitmap dimensions",
            ));
        }
        // SAFETY: DC and bitmap stay on their owning UI thread. Every failure
        // releases the resources already created; Drop restores the selected object.
        unsafe {
            let dc = CreateCompatibleDC(null_mut());
            if dc.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = null_mut();
            let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
            if bitmap.is_null() || bits.is_null() {
                let error = std::io::Error::last_os_error();
                if !bitmap.is_null() {
                    DeleteObject(bitmap);
                }
                DeleteDC(dc);
                return Err(error);
            }
            let previous = SelectObject(dc, bitmap);
            if previous.is_null() || previous as isize == -1 {
                let error = std::io::Error::last_os_error();
                DeleteObject(bitmap);
                DeleteDC(dc);
                return Err(error);
            }
            Ok(Self {
                dc,
                bitmap,
                previous,
                pixels: NonNull::new_unchecked(bits.cast()),
                size: (width, height),
            })
        }
    }

    pub fn size(&self) -> (i32, i32) {
        self.size
    }

    pub fn dc(&self) -> HDC {
        self.dc
    }

    pub fn pixels_mut(&mut self) -> &mut [u32] {
        // SAFETY: this exclusive borrow covers the validated DIB allocation.
        unsafe {
            std::slice::from_raw_parts_mut(
                self.pixels.as_ptr(),
                self.size.0 as usize * self.size.1 as usize,
            )
        }
    }
}

impl Drop for Bitmap {
    fn drop(&mut self) {
        // SAFETY: restore the DC's original bitmap before freeing owned GDI objects.
        unsafe {
            SelectObject(self.dc, self.previous);
            DeleteObject(self.bitmap);
            DeleteDC(self.dc);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_text_output_matches_the_previous_pixel_buffer_across_repaints_and_resize() {
        use crate::widget::text::{Renderer, Style};
        let mut text = Renderer::new(96).unwrap();
        for (width, height) in [(300, 48), (600, 96), (300, 48)] {
            let mut bitmap = Bitmap::new(width, height).unwrap();
            for label in ["42.1 GiB", "— °C", "0.00", "99 %"] {
                let run = text.layout(label, Style::Detail).unwrap().at(
                    10.0,
                    height as f32 / 2.0,
                    0x00ff_ffff,
                );
                let mut expected = vec![0; (width * height) as usize];
                let size = (width as u32, height as u32);
                text.render(size, std::slice::from_ref(&run), &mut expected)
                    .unwrap();
                bitmap.pixels_mut().fill(0xffff_ffff);
                text.render(size, &[run], bitmap.pixels_mut()).unwrap();
                assert_eq!(bitmap.pixels_mut(), expected);
                assert!(expected.contains(&0));
                assert!(expected.iter().any(|&pixel| pixel >> 24 != 0));
            }
        }
    }

    #[test]
    fn native_surface_can_be_rewritten_and_read_back_without_an_intermediate_frame() {
        for (width, height) in [(300, 48), (600, 96), (900, 144)] {
            let mut bitmap = Bitmap::new(width, height).unwrap();
            let address = bitmap.pixels_mut().as_ptr();
            for value in [0x8080_0000, 0, 0xffff_ffff] {
                bitmap.pixels_mut().fill(value);
                let mut info = DIBSECTION::default();
                // SAFETY: GetObject copies the live bitmap's description to this slot.
                let count = unsafe {
                    GetObjectW(
                        bitmap.bitmap,
                        size_of::<DIBSECTION>() as i32,
                        (&mut info as *mut DIBSECTION).cast(),
                    )
                };
                assert_eq!(count as usize, size_of::<DIBSECTION>());
                assert_eq!(info.dsBm.bmBits.cast::<u32>(), address.cast_mut());
                assert_eq!(bitmap.pixels_mut().len(), (width * height) as usize);
                assert!(bitmap.pixels_mut().iter().all(|&pixel| pixel == value));
            }
        }
        for size in [(0, 48), (300, 0), (-1, 48), (300, -1)] {
            assert!(Bitmap::new(size.0, size.1).is_err());
        }
    }
}
