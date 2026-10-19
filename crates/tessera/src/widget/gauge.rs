//! Open taskbar gauges with curved CPU/GPU/RAM lettering replacing the final arc
//! segment. The wide bottom opening is reserved for temperature / used capacity.
use tessera_core::hardware::{HardwareSnapshot, format_memory, format_temperature, format_usage};
use tiny_skia::{LineCap, Paint, Path, PathBuilder, Pixmap, Stroke, Transform};

pub const SIZE: f32 = 40.0;
pub const COLUMN_WIDTH: f64 = 48.0;
pub const COUNT: usize = 3;

/// Metric indices stay stable for labels/colors; display slots stay contiguous.
pub fn visible(presence: tessera_core::hardware::gpu::Presence) -> &'static [usize] {
    if presence == tessera_core::hardware::gpu::Presence::Absent {
        &[0, 2]
    } else {
        &[0, 1, 2]
    }
}
// [left, top, right, bottom], relative to the 48 x 40-DIP column.
pub const TEXT_BOUNDS: [[f32; 4]; 2] = [[6.0, 7.0, 33.0, 31.0], [0.0, 28.0, 42.0, 40.0]];
const CENTER: (f32, f32) = (21.0, 18.5);
const RADIUS: f32 = 17.0;
const LABEL_ANGLES: [f32; 3] = [-36.0, -12.0, 12.0];
/// Centers and tangent rotations for the system-font labels after the solid arc.
pub fn labels(column: usize) -> impl Iterator<Item = (char, [f32; 2], f32)> {
    // CPU/GPU lettering needs a little more inset to match RAM visually.
    let radius = if column < 2 { 17.5 } else { 18.5 };
    ["CPU", "GPU", "RAM"][column]
        .chars()
        .zip(LABEL_ANGLES)
        .map(move |(character, angle)| {
            let angle = angle.to_radians();
            (
                character,
                [
                    CENTER.0 + radius * angle.cos(),
                    CENTER.1 + radius * angle.sin(),
                ],
                angle + std::f32::consts::FRAC_PI_2,
            )
        })
}

#[derive(Clone, Debug, PartialEq)]
pub struct Reading {
    pub percent: Option<f64>,
    pub value: String,
    pub detail: String,
}

impl Reading {
    fn new(percent: Option<f64>, detail: String, paused: bool) -> Self {
        let percent = percent.filter(|value| value.is_finite() && (0.0..=100.0).contains(value));
        Self {
            percent: if paused { None } else { percent },
            value: if paused {
                "Ⅱ".into()
            } else {
                format_usage(percent)
            },
            detail: if paused { "—".into() } else { detail },
        }
    }

    pub fn from_snapshot(snapshot: &HardwareSnapshot, paused: bool) -> [Self; COUNT] {
        [
            Self::new(
                snapshot.cpu_percent,
                format_temperature(snapshot.cpu_temperature_c),
                paused,
            ),
            Self::new(
                snapshot.gpu.percent,
                format_memory(snapshot.gpu.memory_used_bytes),
                paused,
            ),
            Self::new(
                snapshot.memory_percent,
                format_memory(snapshot.memory_used_bytes),
                paused,
            ),
        ]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub network: u32,
    pub text: u32,
    pub secondary: u32,
    track: [u8; 3],
    accents: [[u8; 3]; COUNT],
}

const fn colorref([red, green, blue]: [u8; 3]) -> u32 {
    red as u32 | (green as u32) << 8 | (blue as u32) << 16
}

impl Palette {
    pub fn label(self, column: usize, paused: bool) -> u32 {
        colorref(if paused {
            self.track
        } else {
            self.accents[column]
        })
    }

    pub const fn new(dark: bool) -> Self {
        if dark {
            Self {
                network: colorref([255, 255, 255]),
                text: colorref([226, 232, 240]),
                secondary: colorref([203, 213, 225]),
                track: [148, 163, 184],
                accents: [[125, 211, 252], [196, 181, 253], [110, 231, 183]],
            }
        } else {
            Self {
                network: colorref([0, 0, 0]),
                text: colorref([33, 47, 66]),
                secondary: colorref([51, 65, 85]),
                track: [80, 97, 120],
                accents: [[2, 132, 199], [124, 58, 237], [5, 150, 105]],
            }
        }
    }
}

fn arc(fraction: f32) -> Option<Path> {
    if fraction <= 0.0 {
        return None;
    }
    // Solid arc ends at -52 degrees. Curved lettering then continues toward
    // +20 degrees, leaving roughly 135 degrees open across the bottom.
    let start = 155_f32.to_radians();
    let sweep = (153.0 * fraction.min(1.0)).to_radians();
    let segments = (sweep / std::f32::consts::FRAC_PI_2).ceil() as usize;
    let step = sweep / segments as f32;
    let point = |angle: f32| {
        (
            CENTER.0 + RADIUS * angle.cos(),
            CENTER.1 + RADIUS * angle.sin(),
        )
    };
    let mut path = PathBuilder::new();
    let (x, y) = point(start);
    path.move_to(x, y);
    for segment in 0..segments {
        let a = start + step * segment as f32;
        let b = a + step;
        let k = 4.0 / 3.0 * (step / 4.0).tan() * RADIUS;
        let (ax, ay) = point(a);
        let (bx, by) = point(b);
        path.cubic_to(
            ax - k * a.sin(),
            ay + k * a.cos(),
            bx + k * b.sin(),
            by - k * b.cos(),
            bx,
            by,
        );
    }
    path.finish()
}

fn rings(size: u32, reading: &Reading, column: usize, palette: Palette) -> Option<Pixmap> {
    let mut image = Pixmap::new(
        (COLUMN_WIDTH * f64::from(size) / f64::from(SIZE)).round() as u32,
        size,
    )?;
    let transform = Transform::from_scale(size as f32 / SIZE, size as f32 / SIZE);
    let stroke = Stroke {
        width: 2.6,
        line_cap: LineCap::Round,
        ..Default::default()
    };
    let mut paint = Paint::default();
    let [r, g, b] = palette.track;
    paint.set_color_rgba8(r, g, b, 55);
    image.stroke_path(&arc(1.0)?, &paint, &stroke, transform, None);
    if let Some(path) = reading
        .percent
        .and_then(|percent| arc(percent as f32 / 100.0))
    {
        let [r, g, b] = palette.accents[column];
        paint.set_color_rgba8(r, g, b, 255);
        image.stroke_path(&path, &paint, &stroke, transform, None);
    }
    Some(image)
}

/// Composite premultiplied RGBA vector strokes onto the window's BGRA surface.
/// The transparent gap and interior retain the window's draggable alpha-1 pixels.
pub fn paint(
    pixels: &mut [u32],
    (width, height): (usize, usize),
    origin: (usize, usize),
    size: u32,
    reading: &Reading,
    column: usize,
    palette: Palette,
) {
    assert_eq!(pixels.len(), width * height);
    let Some(image) = rings(size, reading, column, palette) else {
        return;
    };
    let (left, top) = origin;
    for y in 0..(size as usize).min(height.saturating_sub(top)) {
        for x in 0..(image.width() as usize).min(width.saturating_sub(left)) {
            let source = image.pixels()[y * image.width() as usize + x];
            let alpha = u32::from(source.alpha());
            if alpha == 0 {
                continue;
            }
            let target = &mut pixels[(top + y) * width + left + x];
            let inverse = 255 - alpha;
            let over = |value: u8, shift: u32| {
                (u32::from(value) + (*target >> shift & 255_u32) * inverse / 255) << shift
            };
            *target = over(source.blue(), 0)
                | over(source.green(), 8)
                | over(source.red(), 16)
                | over(source.alpha(), 24);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readings_keep_usage_separate_from_temperature_capacity_and_pause() {
        let snapshot = HardwareSnapshot {
            cpu_percent: Some(34.2),
            cpu_temperature_c: Some(52.1),
            gpu: tessera_core::hardware::gpu::Snapshot {
                presence: tessera_core::hardware::gpu::Presence::Present,
                name: Some("Test GPU".into()),
                percent: Some(73.1),
                memory_used_bytes: Some(3 << 30),
            },
            memory_percent: Some(50.0),
            memory_used_bytes: Some(8 << 30),
        };
        let readings = Reading::from_snapshot(&snapshot, false);
        assert_eq!(
            (&*readings[0].value, &*readings[0].detail),
            ("34%", "52 °C")
        );
        assert_eq!(
            (&*readings[1].value, &*readings[1].detail),
            ("73%", "3.0 GiB")
        );
        assert_eq!(
            (&*readings[2].value, &*readings[2].detail),
            ("50%", "8.0 GiB")
        );
        assert_eq!(readings[0].percent, Some(34.2));
        for reading in Reading::from_snapshot(&snapshot, true) {
            assert_eq!(reading.percent, None);
            assert_eq!((&*reading.value, &*reading.detail), ("Ⅱ", "—"));
        }
        let readings = Reading::from_snapshot(&HardwareSnapshot::default(), false);
        assert_eq!((&*readings[0].value, &*readings[0].detail), ("—", "— °C"));
        assert_eq!((&*readings[1].value, &*readings[1].detail), ("—", "—"));
        assert_eq!((&*readings[2].value, &*readings[2].detail), ("—", "—"));
    }

    #[test]
    fn absent_gpu_is_removed_while_cpu_and_ram_keep_their_identity() {
        use tessera_core::hardware::gpu::Presence;
        assert_eq!(visible(Presence::Absent), &[0, 2]);
        for presence in [Presence::Present, Presence::Unknown] {
            assert_eq!(visible(presence), &[0, 1, 2]);
        }
        let labels: Vec<String> = visible(Presence::Absent)
            .iter()
            .map(|&metric| super::labels(metric).map(|(c, _, _)| c).collect())
            .collect();
        assert_eq!(labels, ["CPU", "RAM"]);
        assert_eq!(
            Palette::new(false).label(visible(Presence::Absent)[1], false),
            Palette::new(false).label(2, false)
        );
    }

    #[test]
    fn arcs_leave_a_wide_gap_and_progress_tracks_usage_at_each_dpi() {
        for ((_, cpu, cpu_angle), (_, ram, ram_angle)) in labels(0).zip(labels(2)) {
            let distance = |[x, y]: [f32; 2]| (x - CENTER.0).hypot(y - CENTER.1);
            assert!((distance(ram) - distance(cpu) - 1.0).abs() < 0.001);
            assert_eq!(cpu_angle, ram_angle);
        }
        for (column, expected) in ["CPU", "GPU", "RAM"].iter().enumerate() {
            let positions: Vec<_> = labels(column).collect();
            assert_eq!(
                positions.iter().map(|&(c, _, _)| c).collect::<String>(),
                *expected
            );
            assert!(
                positions.iter().all(|&(_, [x, y], _)| x > 30.0
                    && x < COLUMN_WIDTH as f32
                    && y > 0.0
                    && y < SIZE)
            );
            assert_ne!(
                Palette::new(false).label(column, false),
                Palette::new(true).label(column, false)
            );
        }
        for size in [40, 50, 60, 80, 120] {
            let scaled_column = (COLUMN_WIDTH * f64::from(size) / f64::from(SIZE)).round() as u32;
            assert!(scaled_column > size);
            for [left, top, right, bottom] in TEXT_BOUNDS {
                assert!(top >= 0.0 && top < bottom && bottom <= SIZE);
                assert!(left >= 0.0 && left < right && right <= COLUMN_WIDTH as f32);
            }
            for dark in [false, true] {
                let palette = Palette::new(dark);
                let image = |percent| {
                    rings(
                        size,
                        &Reading::new(Some(percent), String::new(), false),
                        0,
                        palette,
                    )
                    .unwrap()
                };
                let empty = image(0.0);
                let full = image(100.0);
                // Interior, bottom opening, and the old lower-right arc stay clear.
                for (x, y) in [(21, 19), (21, 36), (21, 39), (30, 34), (0, 0)] {
                    let pixel =
                        full.pixels()[(y * size / 40 * full.width() + x * size / 40) as usize];
                    assert_eq!(pixel.alpha(), 0);
                }
                let full_alpha: u32 = full
                    .pixels()
                    .iter()
                    .map(|pixel| u32::from(pixel.alpha()))
                    .sum();
                let partial = image(50.0);
                let added = |image: &Pixmap| {
                    image
                        .pixels()
                        .iter()
                        .zip(empty.pixels())
                        .map(|(a, b)| i64::from(a.alpha()) - i64::from(b.alpha()))
                        .sum::<i64>() as f64
                };
                let ratio = added(&partial) / added(&full);
                assert!((0.47..0.55).contains(&ratio), "sweep ratio {ratio}");
                assert!(full_alpha > 0);
                assert!(
                    full.pixels()
                        .iter()
                        .any(|pixel| pixel.alpha() > 0 && pixel.alpha() < 255)
                );
                // Tiny/clipped surfaces must not overrun the buffer.
                let mut clipped = [0x0100_0000; 6];
                paint(
                    &mut clipped,
                    (2, 3),
                    (0, 0),
                    size,
                    &Reading::new(Some(50.0), String::new(), false),
                    1,
                    palette,
                );
                paint(
                    &mut clipped,
                    (2, 3),
                    (5, 5),
                    size,
                    &Reading::new(None, String::new(), false),
                    0,
                    palette,
                );
            }
        }
    }
}
