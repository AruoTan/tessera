//! DirectWrite layouts drawn by Direct2D into a native-size transparent surface.
use super::gauge;
use std::{collections::VecDeque, ptr::null};
use windows::{
    Win32::{
        Foundation::{E_FAIL, RPC_E_CHANGED_MODE},
        Graphics::{
            Direct2D::{Common::*, *},
            DirectWrite::*,
            Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
            Imaging::*,
        },
        System::Com::*,
    },
    core::{Error, PCWSTR, Result, w},
};
use windows_numerics::{Matrix3x2, Vector2};

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum Style {
    Network,
    Usage,
    Suffix,
    Detail,
    Compact,
    Small,
    Label,
    Tiny,
}

impl Style {
    // Slightly larger suffix/detail sizes keep small text legible; only numbers
    // shrink for three-digit usage or long binary-unit boundary values.
    const SIZES: [f32; 8] = [12.0, 13.0, 9.0, 10.0, 10.0, 8.0, 9.0, 7.0];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GaugeField {
    Usage,
    Temperature,
    Memory,
}

struct Apartment(bool);

impl Apartment {
    fn new() -> Result<Self> {
        // SAFETY: the renderer and every COM resource stay on this thread.
        let result = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if result == RPC_E_CHANGED_MODE {
            Ok(Self(false))
        } else {
            result.ok()?;
            Ok(Self(true))
        }
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: balances this thread's successful CoInitializeEx call.
            unsafe { CoUninitialize() };
        }
    }
}

#[derive(Clone)]
pub struct Layout {
    raw: IDWriteTextLayout,
    pub advance: f32,
    ink: [f32; 4],
    baseline: f32,
}

impl Layout {
    pub fn width(&self) -> f32 {
        self.ink[2] - self.ink[0]
    }

    pub fn centered(&self, [left, top, right, bottom]: [f32; 4], color: u32) -> Run {
        self.at(
            (left + right - self.width()) * 0.5,
            (top + bottom) * 0.5,
            color,
        )
    }

    pub fn at(&self, left: f32, center_y: f32, color: u32) -> Run {
        let baseline = (center_y - (self.ink[1] + self.ink[3]) * 0.5 + self.baseline).round();
        self.on_baseline(left, baseline, color)
    }

    fn on_baseline(&self, left: f32, baseline: f32, color: u32) -> Run {
        Run {
            layout: self.clone(),
            origin: Vector2 {
                X: (left - self.ink[0]).round(),
                Y: baseline - self.baseline,
            },
            transform: Matrix3x2::identity(),
            color,
            rotated: false,
        }
    }

    fn rotated(&self, center: [f32; 2], angle: f32, color: u32) -> Run {
        let (sin, cos) = angle.sin_cos();
        Run {
            layout: self.clone(),
            origin: Vector2 {
                X: -(self.ink[0] + self.ink[2]) * 0.5,
                Y: -(self.ink[1] + self.ink[3]) * 0.5,
            },
            transform: Matrix3x2 {
                M11: cos,
                M12: sin,
                M21: -sin,
                M22: cos,
                M31: center[0],
                M32: center[1],
            },
            color,
            rotated: true,
        }
    }
}

pub struct Run {
    layout: Layout,
    origin: Vector2,
    transform: Matrix3x2,
    color: u32,
    rotated: bool,
}

struct Target {
    size: (u32, u32),
    bitmap: IWICBitmap,
    render: ID2D1RenderTarget,
    brush: ID2D1SolidColorBrush,
}

// A frame needs fewer than 32 distinct strings, including units and labels.
// Retain recent readings without accumulating hundreds of native COM layouts.
const LAYOUT_CACHE_LIMIT: usize = 32;

pub struct Renderer {
    write: IDWriteFactory,
    draw: ID2D1Factory,
    imaging: IWICImagingFactory,
    rendering: IDWriteRenderingParams,
    formats: Vec<IDWriteTextFormat>,
    layouts: VecDeque<((Style, String), Layout)>,
    target: Option<Target>,
    pub scale: f32,
    // Must be dropped after all COM interfaces above.
    _apartment: Apartment,
}

impl Renderer {
    pub fn new(dpi: u32) -> Result<Self> {
        let apartment = Apartment::new()?;
        // SAFETY: COM is initialized, output interfaces are owned by this thread.
        unsafe {
            let write: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let rendering = write.CreateRenderingParams()?;
            let draw = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let imaging = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
            let mut collection = None;
            write.GetSystemFontCollection(&mut collection, false)?;
            let collection = collection.ok_or_else(|| Error::from(E_FAIL))?;
            let mut selected = None;
            for name in ["Segoe UI Variable", "Segoe UI Variable Small", "Segoe UI"] {
                let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
                let mut index = 0;
                let mut found = Default::default();
                collection.FindFamilyName(PCWSTR(name.as_ptr()), &mut index, &mut found)?;
                if found.as_bool() {
                    selected = Some(name);
                    break;
                }
            }
            let name = selected.ok_or_else(|| Error::new(E_FAIL, "Windows UI font unavailable"))?;
            let scale = dpi.max(96) as f32 / 96.0;
            let mut formats = Vec::new();
            for size in Style::SIZES {
                let format = write.CreateTextFormat(
                    PCWSTR(name.as_ptr()),
                    &collection,
                    DWRITE_FONT_WEIGHT_NORMAL,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    size * scale,
                    w!("en-US"),
                )?;
                format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
                formats.push(format);
            }
            Ok(Self {
                write,
                draw,
                imaging,
                rendering,
                formats,
                layouts: VecDeque::new(),
                target: None,
                scale,
                _apartment: apartment,
            })
        }
    }

    pub fn layout(&mut self, text: &str, style: Style) -> Result<Layout> {
        if let Some(index) = self
            .layouts
            .iter()
            .position(|((cached_style, cached_text), _)| {
                *cached_style == style && cached_text == text
            })
        {
            let entry = self.layouts.remove(index).unwrap();
            let layout = entry.1.clone();
            self.layouts.push_front(entry);
            return Ok(layout);
        }
        // SAFETY: the UTF-16 buffer remains alive through synchronous layout creation.
        unsafe {
            let raw = self.write.CreateTextLayout(
                &text.encode_utf16().collect::<Vec<_>>(),
                &self.formats[style as usize],
                4_096.0,
                4_096.0,
            )?;
            let mut metrics = DWRITE_TEXT_METRICS::default();
            raw.GetMetrics(&mut metrics)?;
            raw.SetMaxWidth(metrics.widthIncludingTrailingWhitespace.max(1.0))?;
            raw.SetMaxHeight(metrics.height.max(1.0))?;
            let overhang = raw.GetOverhangMetrics()?;
            let mut lines = [DWRITE_LINE_METRICS::default()];
            let mut count = 0;
            raw.GetLineMetrics(Some(&mut lines), &mut count)?;
            let layout = Layout {
                raw,
                advance: metrics.widthIncludingTrailingWhitespace,
                ink: [
                    -overhang.left,
                    -overhang.top,
                    metrics.widthIncludingTrailingWhitespace.max(1.0) + overhang.right,
                    metrics.height.max(1.0) + overhang.bottom,
                ],
                baseline: lines[0].baseline,
            };
            // Evict only the least recently used native layout. Live runs hold
            // their own COM reference and remain valid after eviction.
            if self.layouts.len() == LAYOUT_CACHE_LIMIT {
                self.layouts.pop_back();
            }
            self.layouts
                .push_front(((style, text.to_owned()), layout.clone()));
            Ok(layout)
        }
    }

    pub fn refresh_rendering_params(&mut self) -> Result<()> {
        // SAFETY: the factory and target belong to this renderer's UI thread.
        unsafe {
            self.rendering = self.write.CreateRenderingParams()?;
            if let Some(target) = &self.target {
                target.render.SetTextRenderingParams(&self.rendering);
            }
        }
        Ok(())
    }

    pub fn gauge(
        &mut self,
        text: &str,
        field: GaugeField,
        bounds: [f32; 4],
        color: u32,
    ) -> Result<Vec<Run>> {
        let [left, top, right, bottom] = bounds;
        // Keep the columns compact around the gauge center. Temperature needs
        // less room than memory, whose numbers include a fractional digit.
        let inset = match field {
            GaugeField::Usage => 1.0,
            GaugeField::Temperature => 5.0,
            GaugeField::Memory => 2.0,
        } * self.scale;
        let (left, right) = (left + inset, right - inset);
        let (number, unit) = match field {
            GaugeField::Usage => text
                .strip_suffix('%')
                .map_or((text, ""), |number| (number, "%")),
            _ => text.rsplit_once(' ').unwrap_or((text, "")),
        };
        let (value_style, unit_style, units): (_, _, &[&str]) = match field {
            GaugeField::Usage => (Style::Usage, Style::Suffix, &["%"]),
            GaugeField::Temperature => (Style::Detail, Style::Detail, &["°C"]),
            GaugeField::Memory => (
                Style::Detail,
                Style::Detail,
                &tessera_core::hardware::MEMORY_UNITS,
            ),
        };
        // Usage keeps its fixed columns. Bottom readouts use the actual unit
        // width so the complete number/gap/unit group can follow the arc center.
        let mut unit_width = 0.0_f32;
        if field == GaugeField::Usage {
            for unit in units {
                unit_width = unit_width.max(self.layout(unit, unit_style)?.width().ceil());
            }
        } else if !unit.is_empty() {
            unit_width = self.layout(unit, unit_style)?.width();
        }
        let gap = if field == GaugeField::Usage {
            self.scale.ceil()
        } else if unit.is_empty() {
            0.0
        } else {
            (4.0 * self.scale).ceil()
        };
        let number_right = right - unit_width - gap;
        let reference = self.layout("0123456789", value_style)?;
        let baseline = ((top + bottom - reference.ink[1] - reference.ink[3]) * 0.5
            + reference.baseline)
            .round();
        let mut value = self.layout(number, value_style)?;
        let fallbacks: &[Style] = if field == GaugeField::Usage {
            if number.chars().count() >= 3 {
                value = self.layout(number, Style::Compact)?;
            }
            &[Style::Compact, Style::Small]
        } else {
            &[Style::Suffix, Style::Small, Style::Tiny]
        };
        for &style in fallbacks {
            if value.width().ceil() <= number_right - left {
                break;
            }
            value = self.layout(number, style)?;
        }
        let x = if field == GaugeField::Usage {
            (left + number_right - value.width()) * 0.5
        } else {
            (left + right - value.width() - gap - unit_width) * 0.5
        };
        let mut runs = vec![value.on_baseline(x, baseline, color)];
        if !unit.is_empty() {
            let suffix = self.layout(unit, unit_style)?;
            let x = if field == GaugeField::Usage {
                right - suffix.width()
            } else {
                x + value.width() + gap
            };
            runs.push(suffix.on_baseline(x, baseline, color));
        }
        Ok(runs)
    }

    pub fn labels(
        &mut self,
        column: usize,
        origin: [f32; 2],
        scale: f32,
        color: u32,
    ) -> Result<Vec<Run>> {
        let mut runs = Vec::new();
        for (character, [x, y], angle) in gauge::labels(column) {
            let layout = self.layout(&character.to_string(), Style::Label)?;
            runs.push(layout.rotated([origin[0] + x * scale, origin[1] + y * scale], angle, color));
        }
        Ok(runs)
    }

    pub fn render(&mut self, size: (u32, u32), runs: &[Run], pixels: &mut [u32]) -> Result<()> {
        assert_eq!(pixels.len(), size.0 as usize * size.1 as usize);
        // SAFETY: all resources belong to this thread. WIC copies exactly the
        // requested native-size premultiplied BGRA rows into the supplied buffer.
        unsafe {
            if self
                .target
                .as_ref()
                .is_none_or(|target| target.size != size)
            {
                let bitmap = self.imaging.CreateBitmap(
                    size.0,
                    size.1,
                    &GUID_WICPixelFormat32bppPBGRA,
                    WICBitmapCacheOnLoad,
                )?;
                let render = self.draw.CreateWicBitmapRenderTarget(
                    &bitmap,
                    &D2D1_RENDER_TARGET_PROPERTIES {
                        r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
                        pixelFormat: D2D1_PIXEL_FORMAT {
                            format: DXGI_FORMAT_B8G8R8A8_UNORM,
                            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                        },
                        dpiX: 96.0,
                        dpiY: 96.0,
                        ..Default::default()
                    },
                )?;
                render.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
                render.SetTextRenderingParams(&self.rendering);
                let brush = render.CreateSolidColorBrush(&color(0), None)?;
                self.target = Some(Target {
                    size,
                    bitmap,
                    render,
                    brush,
                });
            }
            let target = self.target.as_ref().unwrap();
            target.render.BeginDraw();
            target.render.SetTransform(&Matrix3x2::identity());
            target.render.Clear(None);
            for run in runs {
                target.brush.SetColor(&color(run.color));
                target.render.SetTransform(&run.transform);
                target.render.DrawTextLayout(
                    run.origin,
                    &run.layout.raw,
                    &target.brush,
                    if run.rotated {
                        D2D1_DRAW_TEXT_OPTIONS_NO_SNAP
                    } else {
                        D2D1_DRAW_TEXT_OPTIONS_NONE
                    },
                );
            }
            if let Err(error) = target.render.EndDraw(None, None) {
                self.target = None;
                return Err(error);
            }
            let bytes =
                std::slice::from_raw_parts_mut(pixels.as_mut_ptr().cast::<u8>(), pixels.len() * 4);
            target.bitmap.CopyPixels(null(), size.0 * 4, bytes)?;
            Ok(())
        }
    }
}

fn color(value: u32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: (value & 255) as f32 / 255.0,
        g: (value >> 8 & 255) as f32 / 255.0,
        b: (value >> 16 & 255) as f32 / 255.0,
        a: 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ink(pixels: &[u32], width: usize) -> Vec<(usize, usize)> {
        pixels
            .iter()
            .enumerate()
            .filter_map(|(index, &pixel)| {
                (pixel >> 24 > 0).then_some((index % width, index / width))
            })
            .collect()
    }

    #[test]
    fn transparent_text_has_grayscale_coverage_and_premultiplied_theme_colors() {
        for dpi in [96, 120, 144, 192, 288] {
            let mut text = Renderer::new(dpi).unwrap();
            let width = (96.0 * text.scale).round() as u32;
            let height = (40.0 * text.scale).round() as u32;
            let layout = text.layout("12.34 KB/s", Style::Network).unwrap();
            for tint in [0, 0xff_ffff] {
                let run = layout.centered([0.0, 0.0, width as f32, height as f32], tint);
                let mut pixels = vec![0; (width * height) as usize];
                text.render((width, height), &[run], &mut pixels).unwrap();
                assert_eq!(pixels[0], 0);
                assert!(
                    pixels
                        .iter()
                        .any(|&pixel| (1..255).contains(&(pixel >> 24)))
                );
                for pixel in pixels {
                    let alpha = pixel >> 24;
                    let expected = if tint == 0 { 0 } else { alpha };
                    for shift in [0, 8, 16] {
                        assert_eq!(pixel >> shift & 255, expected, "color fringe at {dpi} DPI");
                    }
                }
            }
        }
    }

    #[test]
    fn gauge_ink_fits_columns_and_rows_with_separated_baseline_aligned_suffixes() {
        for dpi in [96, 120, 144, 192, 288] {
            let mut text = Renderer::new(dpi).unwrap();
            let scale = text.scale;
            let width = (72.0 * scale).round() as u32;
            let height = (64.0 * scale).round() as u32;
            let origin = (12.0 * scale).round();
            for values in [
                ["8%", "— °C"],
                ["92%", "14.4 GiB"],
                ["100%", "128.0 GiB"],
                ["99%", "999.9 MiB"],
            ] {
                let mut last_bottom = 0;
                for (row, value) in values.iter().enumerate() {
                    let bounds = gauge::TEXT_BOUNDS[row].map(|v| origin + v * scale);
                    let field = match row {
                        0 => GaugeField::Usage,
                        _ if value.ends_with("°C") => GaugeField::Temperature,
                        _ => GaugeField::Memory,
                    };
                    let runs = text.gauge(value, field, bounds, 0xff_ffff).unwrap();
                    assert_eq!(runs.len(), 2);
                    assert_eq!(
                        runs[0].origin.Y + runs[0].layout.baseline,
                        runs[1].origin.Y + runs[1].layout.baseline
                    );
                    let right = runs[0].origin.X + runs[0].layout.ink[2];
                    let left = runs[1].origin.X + runs[1].layout.ink[0];
                    assert!(left - right > 0.0, "number touches unit at {dpi} DPI");
                    let mut pixels = vec![0; (width * height) as usize];
                    text.render((width, height), &runs, &mut pixels).unwrap();
                    let ink = ink(&pixels, width as usize);
                    assert!(!ink.is_empty());
                    let top = ink.iter().map(|&(_, y)| y).min().unwrap();
                    let bottom = ink.iter().map(|&(_, y)| y).max().unwrap();
                    assert!(top >= last_bottom, "rows overlap at {dpi} DPI");
                    assert!((bottom as f32) < origin + gauge::SIZE * scale);
                    assert!(ink.iter().all(|&(x, _)| x as f32 >= origin
                        && (x as f32) < origin + gauge::COLUMN_WIDTH as f32 * scale));
                    last_bottom = bottom + 1;
                }
            }
        }
    }

    #[test]
    fn gauge_usage_anchors_stay_fixed_and_bottom_groups_stay_centered() {
        for dpi in [96, 120, 144, 192, 288] {
            let mut text = Renderer::new(dpi).unwrap();
            for (field, row, inset, values) in [
                (
                    GaugeField::Usage,
                    0,
                    1.0,
                    &["0%", "8%", "12%", "99%", "100%", "—", "Ⅱ"][..],
                ),
                (
                    GaugeField::Temperature,
                    1,
                    5.0,
                    &["9 °C", "52 °C", "100 °C", "150 °C", "— °C", "—"][..],
                ),
                (
                    GaugeField::Memory,
                    1,
                    2.0,
                    &[
                        "0.0 B",
                        "8.0 GiB",
                        "14.4 GiB",
                        "128.0 GiB",
                        "999.9 MiB",
                        "1.0 TiB",
                        "16.0 EiB",
                        "—",
                    ][..],
                ),
            ] {
                let bounds = gauge::TEXT_BOUNDS[row].map(|v| v * text.scale);
                let mut first = None;
                for value in values {
                    let runs = text.gauge(value, field, bounds, 0xff_ffff).unwrap();
                    let number = &runs[0];
                    let last = runs.last().unwrap();
                    let anchor = if field == GaugeField::Usage {
                        number.origin.X + (number.layout.ink[0] + number.layout.ink[2]) * 0.5
                    } else {
                        (number.origin.X
                            + number.layout.ink[0]
                            + last.origin.X
                            + last.layout.ink[2])
                            * 0.5
                    };
                    let baseline = number.origin.Y + number.layout.baseline;
                    let (initial_anchor, initial_baseline) =
                        *first.get_or_insert((anchor, baseline));
                    assert!(
                        (anchor - initial_anchor).abs() <= 1.001,
                        "alignment shifted for {value} at {dpi} DPI"
                    );
                    assert!(
                        (baseline - initial_baseline).abs() < 0.001,
                        "baseline shifted for {value}"
                    );
                    assert!(number.origin.X + number.layout.ink[0] >= bounds[0] - 0.51);
                    if field != GaugeField::Usage {
                        assert!(
                            (anchor - (bounds[0] + bounds[2]) * 0.5).abs() <= 0.51,
                            "bottom group is off-center for {value} at {dpi} DPI"
                        );
                    }
                    if let Some(unit) = runs.get(1) {
                        let left = unit.origin.X + unit.layout.ink[0];
                        let right = unit.origin.X + unit.layout.ink[2];
                        if field == GaugeField::Usage {
                            assert!(
                                (right - (bounds[2] - inset * text.scale)).abs() <= 0.51,
                                "unit shifted for {value} at {dpi} DPI"
                            );
                        } else {
                            assert!(right <= bounds[2] - inset * text.scale + 0.51);
                            assert!(
                                (left
                                    - number.origin.X
                                    - number.layout.ink[2]
                                    - (4.0 * text.scale).ceil())
                                .abs()
                                    <= 1.001,
                                "bottom gutter changed for {value} at {dpi} DPI"
                            );
                        }
                        assert!((unit.origin.Y + unit.layout.baseline - baseline).abs() < 0.001);
                        assert!(
                            unit.origin.X + unit.layout.ink[0]
                                - number.origin.X
                                - number.layout.ink[2]
                                > 0.0,
                            "number touches unit for {value} at {dpi} DPI"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn curved_system_labels_are_distinct_unclipped_and_clear_of_percentage_space() {
        for dpi in [96, 120, 144, 192, 288] {
            let mut text = Renderer::new(dpi).unwrap();
            let width = (gauge::COLUMN_WIDTH as f32 * text.scale).round() as u32;
            let height = (gauge::SIZE * text.scale).round() as u32;
            let mut masks = Vec::new();
            for column in 0..gauge::COUNT {
                let runs = text
                    .labels(column, [0.0, 0.0], text.scale, 0xff_ffff)
                    .unwrap();
                let mut pixels = vec![0; (width * height) as usize];
                text.render((width, height), &runs, &mut pixels).unwrap();
                assert!(!ink(&pixels, width as usize).is_empty());
                for (index, &pixel) in pixels.iter().enumerate() {
                    let x = index % width as usize;
                    let y = index / width as usize;
                    if x == 0 || y == 0 || x == width as usize - 1 || y == height as usize - 1 {
                        assert_eq!(pixel, 0, "label clipped at {dpi} DPI");
                    }
                    let x = x as f32 / text.scale;
                    let y = y as f32 / text.scale;
                    if (26.0..33.5).contains(&x) && (18.0..27.0).contains(&y) {
                        assert!(pixel >> 24 < 64, "label intersects percentage space");
                    }
                }
                masks.push(pixels);
            }
            assert_ne!(masks[0], masks[1]);
            assert_ne!(masks[1], masks[2]);
        }
    }

    #[test]
    fn layout_cache_is_bounded_and_live_runs_survive_eviction_and_surface_resize() {
        let mut text = Renderer::new(96).unwrap();
        let layout = text.layout("1.25", Style::Network).unwrap();
        let run = layout.centered([0.0, 0.0, 80.0, 40.0], 0xff_ffff);
        let unit = text.layout("KB/s", Style::Network).unwrap();
        for value in 0..512 {
            text.layout(&value.to_string(), Style::Network).unwrap();
            // Active units survive numeric churn; cold readings are evicted.
            assert_eq!(text.layout("KB/s", Style::Network).unwrap().raw, unit.raw);
            assert!(text.layouts.len() <= LAYOUT_CACHE_LIMIT);
        }
        assert!(!text.layouts.iter().any(|((_, key), _)| key == "1.25"));
        for size in [(80, 40), (120, 60), (80, 40)] {
            let mut pixels = vec![0; (size.0 * size.1) as usize];
            text.render(size, std::slice::from_ref(&run), &mut pixels)
                .unwrap();
            assert!(!ink(&pixels, size.0 as usize).is_empty());
            assert_eq!(text.target.as_ref().unwrap().size, size);
        }
    }
}
