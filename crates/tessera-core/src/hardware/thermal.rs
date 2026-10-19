//! Compatibility with Windows thermal zones. These are firmware readings and
//! may represent a board/ambient zone rather than the CPU die.
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ThermalUnit {
    /// The standard PDH Temperature counter is in whole kelvin.
    Kelvin,
    /// High Precision Temperature and MSAcpi CurrentTemperature use 0.1 kelvin.
    Decikelvin,
}

struct History {
    first: f64,
    moved: bool,
}

#[derive(Default)]
pub struct ThermalZones(BTreeMap<(String, ThermalUnit), History>);

impl ThermalZones {
    pub fn read(&mut self, zone: &str, raw: f64, unit: ThermalUnit) -> Option<f64> {
        if !raw.is_finite() {
            return None;
        }
        let kelvin = match unit {
            ThermalUnit::Kelvin => raw,
            ThermalUnit::Decikelvin => raw / 10.0,
        };
        let celsius = kelvin - 273.15;
        if celsius > 0.0 && celsius < 150.0 {
            let history = self.0.entry((zone.into(), unit)).or_insert(History {
                first: raw,
                moved: false,
            });
            history.moved |= raw != history.first;
            // Filter firmware placeholders: a constant 290 K
            // or 300 K must not masquerade as a measured 17 °C / 27 °C.
            if !history.moved && kelvin % 10.0 == 0.0 {
                return None;
            }
            return Some(celsius);
        }
        // Support ACPI / high precision OEM
        // providers that incorrectly supply direct Celsius instead of 0.1 K.
        (unit == ThermalUnit::Decikelvin && raw > 15.0 && raw < 110.0).then_some(raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(actual: Option<f64>, expected: f64) {
        assert!((actual.unwrap() - expected).abs() < 0.001);
    }

    #[test]
    fn converts_each_windows_source_in_its_own_units() {
        let mut zones = ThermalZones::default();
        close(zones.read("pdh", 328.0, ThermalUnit::Kelvin), 54.85);
        close(zones.read("pdh-hp", 3284.0, ThermalUnit::Decikelvin), 55.25);
        close(zones.read("wmi", 3291.0, ThermalUnit::Decikelvin), 55.95);
        close(zones.read("oem", 62.0, ThermalUnit::Decikelvin), 62.0);
        for raw in [0.0, -1.0, 255.0, 5000.0, f64::NAN, f64::INFINITY] {
            assert_eq!(zones.read("bad", raw, ThermalUnit::Decikelvin), None);
        }
    }

    #[test]
    fn placeholders_stay_hidden_until_that_zone_changes() {
        let mut zones = ThermalZones::default();
        for _ in 0..5 {
            assert_eq!(zones.read("pdh", 2900.0, ThermalUnit::Decikelvin), None);
            assert_eq!(zones.read("wmi", 3000.0, ThermalUnit::Decikelvin), None);
            assert_eq!(zones.read("standard", 300.0, ThermalUnit::Kelvin), None);
        }
        close(zones.read("wmi", 3010.0, ThermalUnit::Decikelvin), 27.85);
        close(zones.read("wmi", 3000.0, ThermalUnit::Decikelvin), 26.85);
        // Moving one sensor must not cause a different zone to become trusted.
        assert_eq!(zones.read("pdh", 2900.0, ThermalUnit::Decikelvin), None);
    }
}
