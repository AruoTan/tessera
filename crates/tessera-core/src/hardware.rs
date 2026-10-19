//! System-wide hardware readings. Missing data stays distinct from zero usage.
use serde::Serialize;

pub mod gpu;
pub mod thermal;

#[derive(Clone, Debug, Default, Serialize)]
pub struct HardwareSnapshot {
    pub cpu_percent: Option<f64>,
    pub cpu_temperature_c: Option<f64>,
    pub gpu: gpu::Snapshot,
    pub memory_percent: Option<f64>,
    pub memory_used_bytes: Option<u64>,
}

/// Physical memory in use; available memory includes reclaimable standby pages.
pub fn memory_percent(total: u64, available: u64) -> Option<f64> {
    (total != 0 && available <= total).then(|| (total - available) as f64 / total as f64 * 100.0)
}

pub fn format_usage(percent: Option<f64>) -> String {
    match percent.filter(|value| value.is_finite() && (0.0..=100.0).contains(value)) {
        Some(value) => format!("{value:.0}%"),
        None => "—".into(),
    }
}

pub fn format_temperature(celsius: Option<f64>) -> String {
    match celsius.filter(|value| value.is_finite() && *value > 0.0 && *value < 150.0) {
        Some(value) => format!("{value:.0} °C"),
        None => "— °C".into(),
    }
}

pub const MEMORY_UNITS: [&str; 7] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];

/// Keep at most three integer digits and one decimal place. Promote before
/// rounding would produce 1000.0, while retaining exact binary conversion.
pub fn format_memory(bytes: Option<u64>) -> String {
    let Some(bytes) = bytes else {
        return "—".into();
    };
    let scaled = u128::from(bytes) * 10;
    let mut divisor = 1_u128;
    for unit in MEMORY_UNITS {
        let tenths = (scaled + divisor / 2) / divisor;
        if tenths < 10_000 || unit == "EiB" {
            return format!("{}.{:01} {unit}", tenths / 10, tenths % 10);
        }
        divisor *= 1024;
    }
    unreachable!("u64 byte counts fit in EiB")
}

pub struct TemperatureSensor {
    pub identifier: String,
    pub name: String,
    pub celsius: Option<f64>,
}

/// Prefer die/package readings, then CPU cores, then an explicitly labelled CPU
/// sensor on a motherboard's Super I/O chip. Other devices never qualify by name.
pub fn cpu_temperature(sensors: impl IntoIterator<Item = TemperatureSensor>) -> Option<f64> {
    sensors
        .into_iter()
        .filter_map(|sensor| {
            let id = sensor.identifier.to_ascii_lowercase();
            let name = sensor.name.to_ascii_lowercase();
            let cpu_die = ["/intelcpu/", "/amdcpu/"]
                .iter()
                .any(|prefix| id.starts_with(prefix));
            let board_cpu = id.starts_with("/lpc/")
                && name
                    .split(|c: char| !c.is_ascii_alphanumeric())
                    .any(|word| word == "cpu");
            if !cpu_die && !board_cpu {
                return None;
            }
            // Distance to TjMax is a margin, not a temperature reading.
            if name.contains("tjmax") || name.contains("distance") {
                return None;
            }
            let value = sensor
                .celsius
                .filter(|value| value.is_finite() && *value > 0.0 && *value < 150.0)?;
            let priority = if !cpu_die {
                0
            } else if name.contains("package") || name.contains("tdie") {
                3
            } else if name.contains("tctl") {
                2
            } else {
                1
            };
            Some((priority, value))
        })
        .max_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.total_cmp(&b.1)))
        .map(|(_, value)| value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_memory_uses_available_pages_and_rejects_invalid_reads() {
        let gib = 1_u64 << 30;
        assert_eq!(memory_percent(16 * gib, 10 * gib), Some(37.5));
        assert_eq!(memory_percent(16 * gib, 0), Some(100.0));
        assert_eq!(memory_percent(16 * gib, 16 * gib), Some(0.0));
        assert_eq!(memory_percent(0, 0), None);
        assert_eq!(memory_percent(16 * gib, 17 * gib), None);
    }

    #[test]
    fn unavailable_usage_is_not_presented_as_idle() {
        for value in [
            None,
            Some(f64::NAN),
            Some(f64::INFINITY),
            Some(-1.0),
            Some(101.0),
        ] {
            assert_eq!(format_usage(value), "—");
        }
        assert_eq!(format_usage(Some(0.0)), "0%");
        assert_eq!(format_usage(Some(37.56)), "38%");
        assert_eq!(format_usage(Some(99.96)), "100%");
    }

    #[test]
    fn gauge_details_fit_compact_units_and_promote_rounding_boundaries() {
        let gib = 1_u64 << 30;
        assert_eq!(format_memory(None), "—");
        assert_eq!(format_memory(Some(0)), "0.0 B");
        assert_eq!(format_memory(Some(gib * 5 / 2)), "2.5 GiB");
        assert_eq!(format_memory(Some(gib * 16)), "16.0 GiB");
        assert_eq!(format_memory(Some(gib * 10 - 1)), "10.0 GiB");
        assert_eq!(format_memory(Some(gib * 144 / 10)), "14.4 GiB");
        let mib = 1_u64 << 20;
        assert_eq!(format_memory(Some(1023 * mib + 6 * mib / 10)), "1.0 GiB");
        assert_eq!(format_memory(Some(gib - 1)), "1.0 GiB");
        assert_eq!(format_memory(Some(u64::MAX)), "16.0 EiB");
        assert_eq!(format_temperature(Some(52.4)), "52 °C");
        for value in [None, Some(0.0), Some(f64::NAN), Some(150.0)] {
            assert_eq!(format_temperature(value), "— °C");
        }
    }

    #[test]
    fn compact_memory_promotes_each_binary_unit_before_four_digit_rounding() {
        let mut divisor = 1024_u64;
        for pair in MEMORY_UNITS[1..].windows(2) {
            let below = divisor * 999 + divisor * 94 / 100;
            let boundary = (u128::from(divisor) * 99_995).div_ceil(100) as u64;
            assert_eq!(format_memory(Some(below)), format!("999.9 {}", pair[0]));
            assert_eq!(
                format_memory(Some(boundary - 1)),
                format!("999.9 {}", pair[0])
            );
            assert_eq!(format_memory(Some(boundary)), format!("1.0 {}", pair[1]));
            assert_eq!(
                format_memory(Some(divisor * 1021 + divisor / 5)),
                format!("1.0 {}", pair[1])
            );
            divisor *= 1024;
        }
        assert_eq!(format_memory(Some(999)), "999.0 B");
        assert_eq!(format_memory(Some(1000)), "1.0 KiB");
    }

    #[test]
    fn cpu_sensor_selection_excludes_other_devices_and_temperature_margins() {
        let sensor = |id: &str, name: &str, celsius| TemperatureSensor {
            identifier: id.into(),
            name: name.into(),
            celsius,
        };
        assert_eq!(
            cpu_temperature([
                sensor("/nvidiagpu/0/temperature/0", "CPU Package", Some(90.0)),
                sensor("/lpc/0/temperature/0", "CPU", Some(70.0)),
                sensor(
                    "/intelcpu/0/temperature/2",
                    "CPU Core #1 Distance to TjMax",
                    Some(99.0)
                ),
                sensor("/intelcpu/0/temperature/0", "CPU Core #1", Some(60.0)),
                sensor("/intelcpu/0/temperature/1", "CPU Package", Some(52.0)),
            ]),
            Some(52.0)
        );
        assert_eq!(
            cpu_temperature([
                sensor("/amdcpu/0/temperature/0", "Core (Tctl)", Some(70.0)),
                sensor("/amdcpu/0/temperature/1", "Core (Tctl/Tdie)", Some(62.0)),
                sensor("/amdcpu/0/temperature/2", "Core (Tdie)", Some(f64::NAN)),
            ]),
            Some(62.0)
        );
        assert_eq!(
            cpu_temperature([
                sensor("/intelcpu/0/temperature/0", "CPU Core #1", Some(50.0)),
                sensor("/intelcpu/0/temperature/1", "CPU Core #2", Some(55.0)),
            ]),
            Some(55.0)
        );
        assert_eq!(
            cpu_temperature([sensor("/gpu/0", "Core", Some(60.0))]),
            None
        );
    }

    #[test]
    fn motherboard_cpu_sensor_is_a_fallback_but_other_board_sensors_are_not() {
        let sensor = |id: &str, name: &str, celsius| TemperatureSensor {
            identifier: id.into(),
            name: name.into(),
            celsius: Some(celsius),
        };
        assert_eq!(
            cpu_temperature([
                sensor("/lpc/nct6797d/temperature/0", "CPU", 61.0),
                sensor("/lpc/nct6797d/temperature/1", "System", 80.0),
                sensor("/lpc/nct6797d/temperature/2", "Core", 85.0),
                sensor("/nvidiagpu/0/temperature/0", "CPU Package", 90.0),
            ]),
            Some(61.0)
        );
        assert_eq!(
            cpu_temperature([
                sensor("/lpc/it8728f/temperature/0", "CPU Core", 70.0),
                sensor("/intelcpu/0/temperature/0", "CPU Core #1", 55.0),
            ]),
            Some(55.0)
        );
        assert_eq!(
            cpu_temperature([
                sensor("/lpc/0/temperature/0", "CPU", 255.0),
                sensor("/lpc/0/temperature/1", "GPU", 60.0),
                sensor("/lpc/0/temperature/2", "CPU Distance to TjMax", 30.0),
            ]),
            None
        );
    }
}
