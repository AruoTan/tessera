//! Windows GPU counter accounting, independent of counter enumeration order.
use serde::Serialize;
use std::collections::BTreeSet;

/// Detection failures and unavailable counters are not evidence of absent hardware.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub enum Presence {
    #[default]
    Unknown,
    Absent,
    Present,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Snapshot {
    pub presence: Presence,
    pub name: Option<String>,
    pub percent: Option<f64>,
    /// Adapter-wide dedicated + shared usage, never summed across processes.
    pub memory_used_bytes: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct AdapterId {
    pub luid: u64,
    pub physical: u32,
}

impl AdapterId {
    fn parse<'a>(parts: &mut impl Iterator<Item = &'a str>) -> Option<Self> {
        if parts.next()? != "luid" {
            return None;
        }
        let mut hex = || u32::from_str_radix(parts.next()?.strip_prefix("0x")?, 16).ok();
        let high = hex()?;
        let low = hex()?;
        if parts.next()? != "phys" {
            return None;
        }
        Some(Self {
            luid: (u64::from(high) << 32) | u64::from(low),
            physical: parts.next()?.parse().ok()?,
        })
    }
}

/// Compact records reused between collections; PDH instance names are never retained.
#[derive(Default)]
pub struct EngineSamples {
    processes: Vec<((AdapterId, u32, u32), f64)>,
}

impl EngineSamples {
    /// Start a new interval, retaining normal capacity but releasing large bursts.
    pub fn clear(&mut self) {
        if self.processes.capacity() > self.processes.len().max(256) * 4 {
            self.processes.shrink_to(self.processes.len().max(256));
        }
        self.processes.clear();
    }

    pub fn push(&mut self, name: &str, value: f64, hardware_luids: &BTreeSet<u64>) {
        if !value.is_finite() || value < 0.0 {
            return;
        }
        let parsed = (|| {
            let mut parts = name.split('_');
            if parts.next()? != "pid" {
                return None;
            }
            let pid: u32 = parts.next()?.parse().ok()?;
            let adapter = AdapterId::parse(&mut parts)?;
            if !hardware_luids.contains(&adapter.luid) || parts.next()? != "eng" {
                return None;
            }
            let engine: u32 = parts.next()?.parse().ok()?;
            // Engine names are informational, may be blank, and are not unique.
            Some((adapter, engine, pid))
        })();
        if let Some(key) = parsed {
            self.processes.push((key, value));
        }
    }

    /// Add processes on the same engine, then select the busiest engine/GPU.
    pub fn busiest(&mut self, previous: Option<AdapterId>) -> Option<(AdapterId, f64)> {
        self.processes.sort_unstable_by_key(|&(key, _)| key);
        self.processes.dedup_by(|a, b| {
            if a.0 != b.0 {
                return false;
            }
            b.1 = b.1.max(a.1);
            true
        });
        self.processes
            .chunk_by(|a, b| a.0.0 == b.0.0)
            .map(|adapter| {
                let percent = adapter
                    .chunk_by(|a, b| a.0.1 == b.0.1)
                    .map(|engine| {
                        engine
                            .iter()
                            .map(|(_, value)| value)
                            .sum::<f64>()
                            .min(100.0)
                    })
                    .fold(0.0_f64, f64::max);
                (adapter[0].0.0, percent)
            })
            .max_by(|a, b| {
                a.1.total_cmp(&b.1)
                    .then_with(|| (Some(a.0) == previous).cmp(&(Some(b.0) == previous)))
                    .then_with(|| b.0.cmp(&a.0))
            })
    }
}

/// Read one adapter-memory record; callers take the maximum across duplicates.
pub fn memory_sample(name: &str, value: f64, selected: AdapterId) -> Option<u64> {
    let mut parts = name.split('_');
    let adapter = AdapterId::parse(&mut parts)?;
    (adapter == selected
        && parts.next().is_none()
        && value.is_finite()
        && value >= 0.0
        && value < u64::MAX as f64)
        .then_some(value as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn busiest(
        values: &[(String, f64)],
        hardware: &BTreeSet<u64>,
        previous: Option<AdapterId>,
    ) -> Option<(AdapterId, f64)> {
        let mut samples = EngineSamples::default();
        for (name, value) in values {
            samples.push(name, *value, hardware);
        }
        samples.busiest(previous)
    }

    fn memory(values: &[(String, f64)], selected: AdapterId) -> Option<u64> {
        values
            .iter()
            .filter_map(|(name, value)| memory_sample(name, *value, selected))
            .max()
    }

    #[test]
    fn reused_samples_exclude_previous_intervals_and_release_burst_capacity() {
        let hardware = BTreeSet::from([10]);
        let mut samples = EngineSamples::default();
        let (mut name, value) = engine(10, 0, 0, 1, "3D", 70.0);
        samples.push(&name, value, &hardware);
        // A native reader reuses the name buffer immediately after each callback.
        name.clear();
        assert_eq!(samples.busiest(None), Some((id(10, 0), 70.0)));
        let capacity = samples.processes.capacity();
        samples.clear();
        assert_eq!(samples.busiest(None), None);
        assert_eq!(samples.processes.capacity(), capacity);
        let (name, value) = engine(10, 0, 0, 2, "3D", 5.0);
        samples.push(&name, value, &hardware);
        assert_eq!(samples.busiest(None), Some((id(10, 0), 5.0)));
        samples.processes.reserve(16_384);
        samples.clear();
        assert!(samples.processes.capacity() <= 256);
    }

    fn engine(
        luid: u32,
        physical: u32,
        eng: u32,
        pid: u32,
        kind: &str,
        value: f64,
    ) -> (String, f64) {
        (
            format!(
                "pid_{pid}_luid_0x00000000_0x{luid:08X}_phys_{physical}_eng_{eng}_engtype_{kind}"
            ),
            value,
        )
    }

    fn id(luid: u64, physical: u32) -> AdapterId {
        AdapterId { luid, physical }
    }

    #[test]
    fn processes_share_an_engine_but_independent_engines_are_not_added() {
        let values = vec![
            engine(10, 0, 0, 1, "3D", 35.0),
            engine(10, 0, 0, 2, "3D", 45.0),
            engine(10, 0, 1, 1, "Copy", 30.0),
            engine(10, 0, 2, 3, "VideoDecode", 60.0),
        ];
        assert_eq!(
            busiest(&values, &BTreeSet::from([10]), None),
            Some((id(10, 0), 80.0))
        );
    }

    #[test]
    fn compute_video_and_blank_engine_names_count_and_duplicates_do_not() {
        let duplicate = engine(10, 0, 5, 1, "", 70.0);
        let values = vec![
            duplicate.clone(),
            duplicate,
            engine(10, 0, 5, 1, "Compute", 60.0),
            engine(10, 0, 0, 2, "VideoEncode", 90.0),
        ];
        assert_eq!(
            busiest(&values, &BTreeSet::from([10]), None),
            Some((id(10, 0), 90.0))
        );
    }

    #[test]
    fn many_processes_and_duplicate_engines_preserve_accounting_during_churn() {
        let mut values = Vec::new();
        for pid in 1..=256 {
            for eng in 0..8 {
                values.push(engine(10, 0, eng, pid, "3D", 0.125));
                values.push(engine(20, 0, eng, pid, "Compute", 0.25));
                values.push(engine(20, 0, eng, pid, "", 0.125));
            }
        }
        let hardware = BTreeSet::from([10, 20]);
        assert_eq!(busiest(&values, &hardware, None), Some((id(20, 0), 64.0)));
        values.reverse();
        assert_eq!(busiest(&values, &hardware, None), Some((id(20, 0), 64.0)));
        values.retain(|(name, _)| !name.starts_with("pid_1_"));
        assert_eq!(
            busiest(&values, &hardware, Some(id(20, 0))),
            Some((id(20, 0), 63.75))
        );
    }

    #[test]
    fn hybrid_and_linked_gpus_remain_separate_and_software_is_excluded() {
        let values = vec![
            engine(10, 0, 0, 1, "3D", 50.0),
            engine(20, 0, 0, 1, "3D", 70.0),
            engine(20, 1, 0, 1, "3D", 80.0),
            engine(30, 0, 0, 1, "3D", 99.0),
        ];
        assert_eq!(
            busiest(&values, &BTreeSet::from([10, 20]), None),
            Some((id(20, 1), 80.0))
        );
    }

    #[test]
    fn ties_keep_previous_gpu_and_idle_is_distinct_from_missing() {
        let values = vec![engine(10, 0, 0, 1, "", 0.0), engine(20, 0, 0, 1, "", 0.0)];
        let hardware = BTreeSet::from([10, 20]);
        assert_eq!(busiest(&values, &hardware, None), Some((id(10, 0), 0.0)));
        assert_eq!(
            busiest(&values, &hardware, Some(id(20, 0))),
            Some((id(20, 0), 0.0))
        );
        assert_eq!(busiest(&[], &hardware, Some(id(20, 0))), None);
        assert_eq!(busiest(&values, &BTreeSet::new(), None), None);
    }

    #[test]
    fn enumeration_order_process_churn_and_gpu_removal_cannot_reuse_previous_load() {
        let hardware = BTreeSet::from([10, 20]);
        let mut values = vec![
            engine(10, 0, 0, 1, "3D", 40.0),
            engine(10, 0, 0, 2, "3D", 45.0),
            engine(20, 0, 0, 3, "3D", 60.0),
        ];
        let selected = busiest(&values, &hardware, None);
        values.reverse();
        assert_eq!(busiest(&values, &hardware, None), selected);
        assert_eq!(selected, Some((id(10, 0), 85.0)));
        // A new process appears on the other GPU; no startup PID inventory exists.
        values.push(engine(20, 0, 0, 99, "Compute", 30.0));
        assert_eq!(
            busiest(&values, &hardware, Some(id(10, 0))),
            Some((id(20, 0), 90.0))
        );
        values.retain(|(name, _)| !name.starts_with("pid_99_"));
        assert_eq!(busiest(&values, &hardware, Some(id(20, 0))), selected);
        // Counters left by an unplugged device no longer qualify.
        assert_eq!(
            busiest(&values, &BTreeSet::from([20]), Some(id(10, 0))),
            Some((id(20, 0), 60.0))
        );
    }

    #[test]
    fn invalid_instances_are_ignored_and_usage_is_bounded() {
        let values = vec![
            engine(10, 0, 0, 1, "", f64::NAN),
            engine(10, 0, 1, 1, "", f64::INFINITY),
            engine(10, 0, 2, 1, "", -1.0),
            ("_Total".into(), 99.0),
            ("pid_bad_luid_0x0_0xA_phys_0_eng_0".into(), 99.0),
        ];
        assert_eq!(busiest(&values, &BTreeSet::from([10]), None), None);
        let values = vec![engine(10, 0, 0, 1, "", 70.0), engine(10, 0, 0, 2, "", 60.0)];
        assert_eq!(
            busiest(&values, &BTreeSet::from([10]), None),
            Some((id(10, 0), 100.0))
        );
    }

    #[test]
    fn memory_matches_the_same_gpu_without_process_or_adapter_double_counting() {
        let values = vec![
            ("luid_0x00000000_0x0000000A_phys_0".into(), 1024.0),
            ("luid_0x00000000_0x0000000A_phys_0".into(), 1024.0),
            ("luid_0x00000000_0x0000000A_phys_1".into(), 4096.0),
            ("luid_0x00000000_0x00000014_phys_0".into(), 8192.0),
            ("pid_1_luid_0x00000000_0x0000000A_phys_0".into(), 2048.0),
            ("luid_0x00000000_0x0000000A_phys_0".into(), f64::NAN),
        ];
        assert_eq!(memory(&values, id(10, 0)), Some(1024));
        assert_eq!(memory(&values, id(10, 1)), Some(4096));
        assert_eq!(memory(&values, id(30, 0)), None);
        assert_eq!(
            memory(
                &[("luid_0xffffffff_0x1234ABCD_phys_0".into(), 0.0)],
                id(0xffff_ffff_1234_abcd, 0)
            ),
            Some(0)
        );
    }
}
