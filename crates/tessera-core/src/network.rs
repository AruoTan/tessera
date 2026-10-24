use std::{collections::BTreeMap, time::Duration};

#[derive(Clone, Copy, Debug, Default)]
pub struct Counters {
    pub received: u64,
    pub transmitted: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdapterKind {
    Ethernet,
    Wifi,
    Other,
}

#[derive(Clone, Debug)]
pub struct Adapter {
    pub name: String,
    pub kind: AdapterKind,
    /// Hardware-backed, excluding filter and endpoint interfaces.
    pub physical: bool,
    pub counters: Counters,
}

/// Keep only physical Ethernet/Wi-Fi interfaces, once per stable OS interface ID.
/// Names and MAC addresses are not identities: names can change and virtual
/// interfaces can share a physical adapter's MAC address.
pub fn physical_adapters(
    adapters: impl IntoIterator<Item = (u64, Adapter)>,
) -> BTreeMap<u64, Adapter> {
    adapters
        .into_iter()
        .filter(|(_, adapter)| eligible(adapter))
        .collect()
}

fn eligible(adapter: &Adapter) -> bool {
    adapter.physical && matches!(adapter.kind, AdapterKind::Ethernet | AdapterKind::Wifi)
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PauseState {
    pub paused: bool,
    revision: u64,
}

impl PauseState {
    pub fn set(&mut self, paused: bool) {
        if self.paused != paused {
            self.paused = paused;
            self.revision = self.revision.wrapping_add(1);
        }
    }

    pub fn counts_interval_after(self, previous: Self) -> bool {
        !self.paused && !previous.paused && self.revision == previous.revision
    }
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub hardware: crate::hardware::HardwareSnapshot,
    pub download_bps: f64,
    pub upload_bps: f64,
    pub adapters: Vec<String>,
    pub ready: bool,
    pub paused: bool,
    pub adapter_missing: bool,
}

#[derive(Debug, Default, PartialEq)]
pub struct Sample {
    pub received: u64,
    pub transmitted: u64,
    pub download_bps: f64,
    pub upload_bps: f64,
    pub ready: bool,
}

#[derive(Default)]
pub struct Sampler {
    previous: BTreeMap<u64, Counters>,
    selection: Option<String>,
    initialized: bool,
}

impl Sampler {
    pub fn sample(
        &mut self,
        current: BTreeMap<u64, Adapter>,
        elapsed: Duration,
        adapter: Option<&str>,
    ) -> Sample {
        let baseline_ready =
            self.initialized && adapter == self.selection.as_deref() && !elapsed.is_zero();
        // Keep existing baseline nodes for unchanged interfaces. Remove missing
        // or newly ineligible identities before updating counters in place.
        self.previous
            .retain(|id, _| current.get(id).is_some_and(eligible));
        let mut result = Sample::default();
        for (id, value) in current {
            // Apply the same metadata policy to automatic and explicit selection,
            // including callers that supply unfiltered interfaces.
            if !eligible(&value) {
                continue;
            }
            let previous = self.previous.insert(id, value.counters);
            if !baseline_ready || adapter.is_some_and(|selected| selected != value.name) {
                continue;
            }
            if let Some(previous) = previous {
                // Reset/replaced interfaces must never cause unsigned wraparound spikes.
                if let (Some(received), Some(transmitted)) = (
                    value.counters.received.checked_sub(previous.received),
                    value.counters.transmitted.checked_sub(previous.transmitted),
                ) {
                    result.received = result.received.saturating_add(received);
                    result.transmitted = result.transmitted.saturating_add(transmitted);
                }
                result.ready = true;
            }
        }
        if baseline_ready {
            result.download_bps = result.received as f64 / elapsed.as_secs_f64();
            result.upload_bps = result.transmitted as f64 / elapsed.as_secs_f64();
        }
        if self.selection.as_deref() != adapter {
            self.selection = adapter.map(str::to_owned);
        }
        self.initialized = true;
        result
    }
}

pub const SPEED_UNITS: [&str; 7] = ["B/s", "KB/s", "MB/s", "GB/s", "TB/s", "PB/s", "EB/s"];
pub const SPEED_DECIMALS: usize = 2;

/// Separate fields let the native readout align numbers and units independently.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpeedDisplay {
    pub value: String,
    pub unit: &'static str,
}

pub fn format_speed(bytes_per_second: f64) -> SpeedDisplay {
    let mut value = if bytes_per_second.is_finite() {
        bytes_per_second.max(0.0)
    } else {
        0.0
    };
    let precision = SPEED_DECIMALS;
    let mut unit = 0;
    while value >= 1_000.0 && unit < SPEED_UNITS.len() - 1 {
        value /= 1_000.0;
        unit += 1;
    }
    let mut number = format!("{value:.precision$}");
    // Promote after formatting too: rounding must not produce a four-digit
    // mantissa (e.g. 999.996 KB/s -> 1.00 MB/s).
    if value >= 1_000.0 || number.starts_with("1000") {
        if unit < SPEED_UNITS.len() - 1 {
            value /= 1_000.0;
            unit += 1;
            number = format!("{value:.precision$}");
        } else {
            // Explicit overflow marker keeps even implausible finite inputs
            // inside the fixed number column instead of clipping their units.
            number = "999+".into();
        }
    }
    SpeedDisplay {
        value: number,
        unit: SPEED_UNITS[unit],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counters(entries: &[(&str, u64, u64)]) -> BTreeMap<u64, Adapter> {
        entries
            .iter()
            .map(|(name, received, transmitted)| {
                (
                    // Each fixture name starts with a distinct ASCII character.
                    u64::from(name.as_bytes()[0]),
                    Adapter {
                        name: name.to_string(),
                        kind: AdapterKind::Ethernet,
                        physical: true,
                        counters: Counters {
                            received: *received,
                            transmitted: *transmitted,
                        },
                    },
                )
            })
            .collect()
    }

    #[test]
    fn first_sample_is_a_baseline_and_rate_uses_actual_elapsed_time() {
        let mut sampler = Sampler::default();
        assert!(
            !sampler
                .sample(
                    counters(&[("Ethernet", 10_000, 4_000)]),
                    Duration::from_secs(1),
                    None
                )
                .ready
        );
        let sample = sampler.sample(
            counters(&[("Ethernet", 12_000, 5_000)]),
            Duration::from_millis(500),
            None,
        );
        assert_eq!(sample.download_bps, 4_000.0);
        assert_eq!(sample.upload_bps, 2_000.0);
    }

    #[test]
    fn counters_reset_and_new_or_removed_adapters_do_not_spike() {
        let mut sampler = Sampler::default();
        sampler.sample(
            counters(&[("A", 100, 100), ("Removed", 9_000, 9_000)]),
            Duration::from_secs(1),
            None,
        );
        let sample = sampler.sample(
            counters(&[("A", 10, 20), ("New", 999_999, 999_999)]),
            Duration::from_secs(1),
            None,
        );
        assert_eq!(sample.received, 0);
        assert_eq!(sample.transmitted, 0);
        let sample = sampler.sample(
            counters(&[("A", 30, 40), ("New", 1_000_000, 1_000_000)]),
            Duration::from_secs(1),
            None,
        );
        assert_eq!(sample.received, 21);
    }

    #[test]
    fn selection_change_rebaselines_and_missing_adapter_is_not_ready() {
        let mut sampler = Sampler::default();
        sampler.sample(
            counters(&[("A", 100, 0), ("B", 900, 0)]),
            Duration::from_secs(1),
            None,
        );
        assert!(
            !sampler
                .sample(
                    counters(&[("A", 200, 0)]),
                    Duration::from_secs(1),
                    Some("A")
                )
                .ready
        );
        assert_eq!(
            sampler
                .sample(
                    counters(&[("A", 250, 0), ("B", 1000, 0)]),
                    Duration::from_secs(1),
                    Some("A")
                )
                .received,
            50
        );
        assert!(
            !sampler
                .sample(counters(&[]), Duration::from_secs(1), Some("A"))
                .ready
        );
    }

    #[test]
    fn zero_elapsed_never_produces_infinity() {
        let mut sampler = Sampler::default();
        sampler.sample(counters(&[("A", 1, 0)]), Duration::ZERO, None);
        assert_eq!(
            sampler.sample(counters(&[("A", 2, 0)]), Duration::ZERO, None),
            Sample::default()
        );
    }

    fn interface(name: &str, kind: AdapterKind, physical: bool, bytes: u64) -> Adapter {
        Adapter {
            name: name.into(),
            kind,
            physical,
            counters: Counters {
                received: bytes,
                transmitted: bytes / 2,
            },
        }
    }

    #[test]
    fn physical_ethernet_and_wifi_are_counted_once_without_virtual_copies() {
        let interfaces = |bytes| {
            physical_adapters([
                (1, interface("Ethernet", AdapterKind::Ethernet, true, bytes)),
                // Duplicate enumeration of the same LUID must not double count.
                (1, interface("Ethernet", AdapterKind::Ethernet, true, bytes)),
                (2, interface("Wi-Fi", AdapterKind::Wifi, true, bytes)),
                (3, interface("VPN", AdapterKind::Ethernet, false, bytes)),
                (
                    4,
                    interface("ZeroTier", AdapterKind::Ethernet, false, bytes),
                ),
                (
                    5,
                    interface("vEthernet (WSL)", AdapterKind::Ethernet, false, bytes),
                ),
                (6, interface("VMware", AdapterKind::Ethernet, false, bytes)),
                (
                    7,
                    interface("Wi-Fi Direct", AdapterKind::Wifi, false, bytes),
                ),
                (8, interface("Loopback", AdapterKind::Other, false, bytes)),
                (9, interface("Bluetooth", AdapterKind::Other, true, bytes)),
                // Eligibility uses metadata even if a virtual adapter is renamed.
                (
                    10,
                    interface("Ethernet 2", AdapterKind::Ethernet, false, bytes),
                ),
            ])
        };
        assert_eq!(interfaces(0).len(), 2);
        let mut sampler = Sampler::default();
        sampler.sample(interfaces(1_000), Duration::from_millis(500), None);
        let sample = sampler.sample(interfaces(2_000), Duration::from_millis(500), None);
        assert_eq!(sample.received, 2_000);
        assert_eq!(sample.transmitted, 1_000);
        assert_eq!(sample.download_bps, 4_000.0);
        assert_eq!(sample.upload_bps, 2_000.0);
    }

    #[test]
    fn an_ineligible_interval_discards_the_old_identity_baseline() {
        let mut sampler = Sampler::default();
        for (physical, kind) in [(false, AdapterKind::Ethernet), (true, AdapterKind::Other)] {
            let interfaces = |physical, kind, bytes| {
                BTreeMap::from([(1, interface("Ethernet", kind, physical, bytes))])
            };
            sampler.sample(
                interfaces(true, AdapterKind::Ethernet, 100),
                Duration::from_secs(1),
                None,
            );
            assert_eq!(
                sampler.sample(
                    interfaces(physical, kind, 1_000),
                    Duration::from_secs(1),
                    None
                ),
                Sample::default()
            );
            // The same identity cannot inherit a baseline through an excluded interval.
            assert_eq!(
                sampler.sample(
                    interfaces(true, AdapterKind::Ethernet, 10_000),
                    Duration::from_secs(1),
                    None,
                ),
                Sample::default()
            );
            assert_eq!(
                sampler
                    .sample(
                        interfaces(true, AdapterKind::Ethernet, 10_100),
                        Duration::from_secs(1),
                        None,
                    )
                    .received,
                100
            );
        }
    }

    #[test]
    fn manual_selection_counts_one_physical_adapter_and_cannot_select_a_virtual_copy() {
        let interfaces = |bytes| {
            [
                (1, interface("Ethernet", AdapterKind::Ethernet, true, bytes)),
                (2, interface("Wi-Fi", AdapterKind::Wifi, true, bytes * 2)),
                (3, interface("VPN", AdapterKind::Ethernet, false, bytes)),
            ]
            .into_iter()
            .collect()
        };
        let mut sampler = Sampler::default();
        sampler.sample(interfaces(1_000), Duration::from_secs(1), Some("Wi-Fi"));
        let sample = sampler.sample(interfaces(1_500), Duration::from_secs(1), Some("Wi-Fi"));
        assert_eq!(sample.received, 1_000);
        assert_eq!(sample.transmitted, 500);
        sampler.sample(interfaces(2_000), Duration::from_secs(1), Some("VPN"));
        assert_eq!(
            sampler.sample(interfaces(3_000), Duration::from_secs(1), Some("VPN")),
            Sample::default()
        );
    }

    #[test]
    fn stable_identity_survives_rename_and_replacement_alias_does_not_inherit_baseline() {
        let mut sampler = Sampler::default();
        let interfaces = |id, name, bytes| {
            physical_adapters([(id, interface(name, AdapterKind::Ethernet, true, bytes))])
        };
        sampler.sample(
            interfaces(1, "Ethernet", 1_000),
            Duration::from_secs(1),
            None,
        );
        let sample = sampler.sample(
            interfaces(1, "Renamed", 1_200),
            Duration::from_secs(1),
            None,
        );
        assert_eq!(sample.received, 200);
        assert!(
            !sampler
                .sample(
                    interfaces(2, "Renamed", 900_000),
                    Duration::from_secs(1),
                    None
                )
                .ready
        );
        assert_eq!(
            sampler
                .sample(
                    interfaces(2, "Renamed", 900_100),
                    Duration::from_secs(1),
                    None
                )
                .received,
            100
        );
    }

    #[test]
    fn failed_read_or_disappearance_rebaselines_without_a_recovery_spike() {
        let mut sampler = Sampler::default();
        sampler.sample(
            counters(&[("Ethernet", 1_000, 500)]),
            Duration::from_secs(1),
            None,
        );
        assert!(
            !sampler
                .sample(BTreeMap::new(), Duration::from_secs(1), None)
                .ready
        );
        assert!(
            !sampler
                .sample(
                    counters(&[("Ethernet", 90_000, 80_000)]),
                    Duration::from_secs(1),
                    None
                )
                .ready
        );
        let sample = sampler.sample(
            counters(&[("Ethernet", 90_100, 80_050)]),
            Duration::from_secs(1),
            None,
        );
        assert_eq!(sample.received, 100);
        assert_eq!(sample.transmitted, 50);
    }

    #[test]
    fn partial_counter_reset_discards_the_uncertain_interval_in_both_directions() {
        let mut sampler = Sampler::default();
        sampler.sample(
            counters(&[("Ethernet", 1_000, 500)]),
            Duration::from_secs(1),
            None,
        );
        let sample = sampler.sample(
            counters(&[("Ethernet", 10, 900)]),
            Duration::from_secs(1),
            None,
        );
        assert_eq!(sample.received, 0);
        assert_eq!(sample.transmitted, 0);
        let sample = sampler.sample(
            counters(&[("Ethernet", 110, 950)]),
            Duration::from_secs(1),
            None,
        );
        assert_eq!(sample.received, 100);
        assert_eq!(sample.transmitted, 50);
    }

    #[test]
    fn pause_and_resume_between_samples_still_exclude_the_boundary_interval() {
        let previous = PauseState::default();
        let mut pause = previous;
        pause.set(true);
        pause.set(false);
        assert!(!pause.counts_interval_after(previous));
        assert!(pause.counts_interval_after(pause));
        let resumed = pause;
        pause.set(true);
        assert!(!pause.counts_interval_after(resumed));
        assert!(!pause.counts_interval_after(pause));
        let paused = pause;
        pause.set(false);
        assert!(!pause.counts_interval_after(paused));
    }

    #[test]
    fn display_has_consistent_decimal_units_and_sanitizes_invalid_values() {
        for (speed, value, unit) in [
            (125_000.0, "125.00", "KB/s"),
            (1_500.0, "1.50", "KB/s"),
            (f64::NAN, "0.00", "B/s"),
            (f64::INFINITY, "0.00", "B/s"),
            (-3.0, "0.00", "B/s"),
            (0.0, "0.00", "B/s"),
            (3.25, "3.25", "B/s"),
            (1e18, "1.00", "EB/s"),
            (f64::MAX, "999+", "EB/s"),
        ] {
            assert_eq!(
                format_speed(speed),
                SpeedDisplay {
                    value: value.into(),
                    unit
                }
            );
        }
    }

    #[test]
    fn rounding_promotes_units_without_growing_the_number_column() {
        let step = 0.01;
        for (index, unit) in SPEED_UNITS.iter().enumerate() {
            let multiplier = 1_000_f64.powi(index as i32);
            let below = format_speed((1_000.0 - step) * multiplier);
            assert_eq!(below.unit, *unit);
            assert!(below.value.starts_with("999"));
            let rounded = format_speed((1_000.0 - step / 4.0) * multiplier);
            if index < SPEED_UNITS.len() - 1 {
                assert_eq!(rounded.unit, SPEED_UNITS[index + 1]);
                assert_eq!(rounded.value, "1.00");
            } else {
                assert_eq!(rounded.value, "999+");
            }
            assert!(rounded.value.len() <= 6);
        }
    }
}
