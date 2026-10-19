//! Vendor-neutral WDDM counters on an isolated thread. Never open a D3D device
//! or spawn a vendor CLI; driver/counter stalls cannot delay the other metrics.
use super::pdh::Array;
use std::{
    collections::{BTreeMap, BTreeSet},
    ptr::{null, null_mut},
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use tessera_core::hardware::gpu::{self, AdapterId, Presence, Snapshot};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, DXGI_ERROR_NOT_FOUND, IDXGIFactory1,
};
use windows_sys::{Win32::System::Performance::*, core::w};

struct Reading {
    at: Instant,
    snapshot: Snapshot,
}

pub struct Reader(Arc<RwLock<Reading>>);

impl Reader {
    pub fn start(shutdown: Arc<AtomicBool>) -> std::io::Result<Self> {
        let latest = Arc::new(RwLock::new(Reading {
            at: Instant::now(),
            snapshot: Snapshot::default(),
        }));
        let output = latest.clone();
        thread::Builder::new()
            .name("gpu-sampler".into())
            .spawn(move || {
                let mut sampler = Sampler::default();
                while !shutdown.load(Ordering::Relaxed) {
                    let next = Instant::now() + Duration::from_secs(1);
                    let snapshot = sampler.read();
                    if let Ok(mut reading) = output.write() {
                        *reading = Reading {
                            at: Instant::now(),
                            snapshot,
                        };
                    }
                    while !shutdown.load(Ordering::Relaxed) {
                        let remaining = next.saturating_duration_since(Instant::now());
                        if remaining.is_zero() {
                            break;
                        }
                        thread::park_timeout(remaining);
                    }
                }
            })?;
        Ok(Self(latest))
    }

    pub fn latest(&self) -> Snapshot {
        let Ok(reading) = self.0.read() else {
            return Snapshot::default();
        };
        if reading.at.elapsed() <= Duration::from_secs(5) {
            reading.snapshot.clone()
        } else {
            Snapshot {
                presence: reading.snapshot.presence,
                ..Default::default()
            }
        }
    }
}

#[derive(Default)]
struct Sampler {
    query: Option<Query>,
    inventory: Option<Inventory>,
    presence: Presence,
    retry_at: Option<Instant>,
    selected: Option<AdapterId>,
    failed: bool,
}

impl Sampler {
    fn unavailable(&self) -> Snapshot {
        Snapshot {
            presence: self.presence,
            ..Default::default()
        }
    }

    fn read(&mut self) -> Snapshot {
        if self.inventory.is_none() && self.retry_at.is_some_and(|at| Instant::now() < at) {
            return self.unavailable();
        }
        let result = (|| {
            // Detection is independent of counter support: a disabled PDH provider
            // must not make an installed GPU disappear. Refresh after device changes.
            // SAFETY: the inventory's DXGI factory belongs to this worker thread.
            if self
                .inventory
                .as_ref()
                .is_none_or(|inventory| !unsafe { inventory.factory.IsCurrent() }.as_bool())
            {
                self.query = None;
                self.selected = None;
                self.inventory = None;
                let inventory = Inventory::new()?;
                self.presence = if inventory.adapters.is_empty() {
                    Presence::Absent
                } else {
                    Presence::Present
                };
                self.inventory = Some(inventory);
                self.retry_at = None;
            }
            if self.presence == Presence::Absent
                || (self.query.is_none() && self.retry_at.is_some_and(|at| Instant::now() < at))
            {
                return Ok(self.unavailable());
            }
            if self.query.is_none() {
                self.query = Some(Query::new()?);
            }
            self.query.as_mut().expect("query initialized").read(
                self.inventory.as_ref().expect("inventory initialized"),
                &mut self.selected,
            )
        })();
        match result {
            Ok(mut snapshot) => {
                if snapshot.percent.is_some() || self.presence == Presence::Absent {
                    self.failed = false;
                }
                snapshot.presence = self.presence;
                snapshot
            }
            Err(status) => {
                if !self.failed {
                    eprintln!("Cannot read GPU counters: 0x{status:08x}");
                }
                self.failed = true;
                self.query = None;
                self.selected = None;
                self.retry_at = Some(Instant::now() + Duration::from_secs(10));
                self.unavailable()
            }
        }
    }
}

struct Query {
    handle: PDH_HQUERY,
    engines: PDH_HCOUNTER,
    dedicated: Option<PDH_HCOUNTER>,
    shared: Option<PDH_HCOUNTER>,
    primed: bool,
    array: Array,
    samples: gpu::EngineSamples,
}

struct Adapter {
    name: String,
    has_dedicated_memory: bool,
}

impl Drop for Query {
    fn drop(&mut self) {
        // SAFETY: this thread owns the query, which owns all counter handles.
        unsafe { PdhCloseQuery(self.handle) };
    }
}

struct Inventory {
    factory: IDXGIFactory1,
    adapters: BTreeMap<u64, Adapter>,
    luids: BTreeSet<u64>,
}

impl Inventory {
    fn new() -> Result<Self, u32> {
        // SAFETY: DXGI returns owned interfaces kept on this GPU worker thread.
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1().map_err(|e| e.code().0 as u32)?;
            let mut adapters = BTreeMap::new();
            for index in 0..256 {
                let adapter = match factory.EnumAdapters1(index) {
                    Ok(adapter) => adapter,
                    Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
                    Err(error) => return Err(error.code().0 as u32),
                };
                let desc = adapter.GetDesc1().map_err(|e| e.code().0 as u32)?;
                if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
                    continue;
                }
                let luid = ((desc.AdapterLuid.HighPart as u32 as u64) << 32)
                    | u64::from(desc.AdapterLuid.LowPart);
                let end = desc
                    .Description
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(desc.Description.len());
                adapters.insert(
                    luid,
                    Adapter {
                        name: String::from_utf16_lossy(&desc.Description[..end]),
                        has_dedicated_memory: desc.DedicatedVideoMemory != 0
                            || desc.DedicatedSystemMemory != 0,
                    },
                );
            }
            Ok(Self {
                factory,
                luids: adapters.keys().copied().collect(),
                adapters,
            })
        }
    }
}

impl Query {
    fn new() -> Result<Self, u32> {
        // SAFETY: PDH writes owned handles into initialized output slots. Every
        // handle stays on this thread and is closed with its containing query.
        unsafe {
            let mut handle = null_mut();
            let status = PdhOpenQueryW(null(), 0, &mut handle);
            if status != 0 {
                return Err(status);
            }
            let mut query = Self {
                handle,
                engines: null_mut(),
                dedicated: None,
                shared: None,
                primed: false,
                array: Array::default(),
                samples: gpu::EngineSamples::default(),
            };
            let status = PdhAddEnglishCounterW(
                handle,
                w!("\\GPU Engine(*)\\Utilization Percentage"),
                0,
                &mut query.engines,
            );
            if status != 0 {
                return Err(status);
            }
            for (path, destination) in [
                (
                    w!("\\GPU Adapter Memory(*)\\Dedicated Usage"),
                    &mut query.dedicated,
                ),
                (
                    w!("\\GPU Adapter Memory(*)\\Shared Usage"),
                    &mut query.shared,
                ),
            ] {
                let mut counter = null_mut();
                if PdhAddEnglishCounterW(handle, path, 0, &mut counter) == 0 {
                    *destination = Some(counter);
                }
            }
            Ok(query)
        }
    }

    fn read(
        &mut self,
        inventory: &Inventory,
        selected: &mut Option<AdapterId>,
    ) -> Result<Snapshot, u32> {
        // SAFETY: the live query belongs to this worker thread.
        let status = unsafe { PdhCollectQueryData(self.handle) };
        if status != 0 {
            return Err(status);
        }
        if !self.primed {
            self.primed = true;
            return Ok(Snapshot::default());
        }
        self.samples.clear();
        self.array.read(self.engines, |name, value| {
            self.samples.push(name, value, &inventory.luids);
        })?;
        let Some((adapter, percent)) = self.samples.busiest(*selected) else {
            // Empty/invalid arrays are unavailable, never a fabricated idle zero.
            return Ok(Snapshot::default());
        };
        let mut memory = |counter: Option<PDH_HCOUNTER>| {
            let mut used = None;
            self.array
                .read(counter?, |name, value| {
                    if let Some(value) = gpu::memory_sample(name, value, adapter) {
                        used = Some(used.map_or(value, |previous: u64| previous.max(value)));
                    }
                })
                .ok()?;
            used
        };
        // Adapter counters avoid shared-resource duplication in process counters.
        // Shared memory matters on integrated GPUs; missing is distinct from 0.
        let info = inventory
            .adapters
            .get(&adapter.luid)
            .expect("hardware LUID filtered");
        let dedicated = if info.has_dedicated_memory {
            memory(self.dedicated)
        } else {
            Some(0)
        };
        let memory_used_bytes = dedicated
            .zip(memory(self.shared))
            .and_then(|(dedicated, shared)| dedicated.checked_add(shared));
        let name = Some(info.name.clone());
        if *selected != Some(adapter) {
            eprintln!(
                "GPU monitor: {} (physical {}, LUID {:016x})",
                name.as_deref().unwrap_or("GPU"),
                adapter.physical,
                adapter.luid
            );
        }
        *selected = Some(adapter);
        Ok(Snapshot {
            presence: Presence::Present,
            name,
            percent: Some(percent),
            memory_used_bytes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counter_backoff_preserves_detection_instead_of_guessing_gpu_absence() {
        for presence in [Presence::Present, Presence::Absent, Presence::Unknown] {
            let mut sampler = Sampler {
                presence,
                retry_at: Some(Instant::now() + Duration::from_secs(10)),
                ..Default::default()
            };
            let snapshot = sampler.read();
            assert_eq!(snapshot.presence, presence);
            assert_eq!(snapshot.percent, None);
            assert_eq!(snapshot.memory_used_bytes, None);
        }
    }

    #[test]
    fn stale_or_failed_gpu_readings_do_not_retain_old_usage_or_memory() {
        let reader = Reader(Arc::new(RwLock::new(Reading {
            at: Instant::now(),
            snapshot: Snapshot {
                presence: Presence::Present,
                name: Some("GPU".into()),
                percent: Some(80.0),
                memory_used_bytes: Some(1024),
            },
        })));
        assert_eq!(reader.latest().percent, Some(80.0));
        reader.0.write().unwrap().at = Instant::now() - Duration::from_secs(6);
        assert_eq!(reader.latest().percent, None);
        assert_eq!(reader.latest().memory_used_bytes, None);
        assert_eq!(reader.latest().presence, Presence::Present);
        *reader.0.write().unwrap() = Reading {
            at: Instant::now(),
            snapshot: Snapshot::default(),
        };
        assert_eq!(reader.latest().percent, None);
        assert_eq!(reader.latest().memory_used_bytes, None);
        assert_eq!(reader.latest().presence, Presence::Unknown);
        reader.0.write().unwrap().snapshot.presence = Presence::Absent;
        reader.0.write().unwrap().at = Instant::now() - Duration::from_secs(6);
        assert_eq!(reader.latest().presence, Presence::Absent);
    }
}
