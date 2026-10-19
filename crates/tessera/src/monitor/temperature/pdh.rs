//! Native thermal-zone fallback, including localized Windows counter names.
use std::{
    ptr::{null, null_mut},
    time::{Duration, Instant},
};
use tessera_core::hardware::thermal::{ThermalUnit, ThermalZones};
use windows_sys::{Win32::System::Performance::*, core::w};

use super::super::pdh::values;

#[derive(Default)]
pub(super) struct Reader {
    query: Option<Query>,
    retry_at: Option<Instant>,
    zones: ThermalZones,
}

impl Reader {
    pub fn read(&mut self) -> Option<f64> {
        if self.query.is_none() && self.retry_at.is_some_and(|at| Instant::now() < at) {
            return None;
        }
        let result = (|| {
            if self.query.is_none() {
                self.query = Some(Query::new()?);
            }
            self.query
                .as_ref()
                .expect("query initialized")
                .read(&mut self.zones)
        })();
        match result {
            Ok(value) => value,
            Err(_) => {
                self.query = None;
                self.retry_at = Some(Instant::now() + Duration::from_secs(10));
                None
            }
        }
    }
}

struct Query {
    handle: PDH_HQUERY,
    counters: Vec<(PDH_HCOUNTER, ThermalUnit)>,
}

impl Drop for Query {
    fn drop(&mut self) {
        // SAFETY: the query owns every counter and stays on its creating thread.
        unsafe { PdhCloseQuery(self.handle) };
    }
}

impl Query {
    fn new() -> Result<Self, u32> {
        // SAFETY: PDH writes owned handles to initialized output slots. English
        // paths are resolved on localized Windows; wildcard arrays enumerate zones.
        unsafe {
            let mut handle = null_mut();
            let status = PdhOpenQueryW(null(), 0, &mut handle);
            if status != 0 {
                return Err(status);
            }
            let mut query = Self {
                handle,
                counters: Vec::new(),
            };
            for (path, unit) in [
                (
                    w!("\\Thermal Zone Information(*)\\High Precision Temperature"),
                    ThermalUnit::Decikelvin,
                ),
                (
                    w!("\\Thermal Zone Information(*)\\Temperature"),
                    ThermalUnit::Kelvin,
                ),
            ] {
                let mut counter = null_mut();
                if PdhAddEnglishCounterW(handle, path, 0, &mut counter) == 0 {
                    query.counters.push((counter, unit));
                }
            }
            if query.counters.is_empty() {
                return Err(PDH_CSTATUS_NO_COUNTER);
            }
            Ok(query)
        }
    }

    fn read(&self, zones: &mut ThermalZones) -> Result<Option<f64>, u32> {
        // SAFETY: all handles belong to this thread's live query.
        let status = unsafe { PdhCollectQueryData(self.handle) };
        if status != 0 {
            return Err(status);
        }
        let mut available = false;
        for &(counter, unit) in &self.counters {
            let Ok(values) = values(counter) else {
                continue;
            };
            available = true;
            let best = values
                .into_iter()
                .filter_map(|(name, raw)| zones.read(&name, raw, unit))
                .max_by(f64::total_cmp);
            // Prefer high precision; standard Temperature remains independently
            // readable when that counter is unsupported or only has placeholders.
            if best.is_some() {
                return Ok(best);
            }
        }
        if available {
            Ok(None)
        } else {
            Err(PDH_NO_DATA)
        }
    }
}
