//! Acquired on the existing sampler thread; never block the taskbar message loop.
#[cfg(windows)]
mod windows {
    use std::{
        mem::size_of,
        ptr::null,
        ptr::null_mut,
        sync::{Arc, atomic::AtomicBool},
        time::{Duration, Instant},
    };
    use tessera_core::hardware::{HardwareSnapshot, memory_percent};
    use windows_sys::{
        Win32::System::{
            Performance::*,
            SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX},
        },
        core::w,
    };

    // Processor Information includes processor groups. English counter paths are
    // resolved by PDH on localized Windows installations as well.
    const CPU_COUNTER: windows_sys::core::PCWSTR =
        w!("\\Processor Information(_Total)\\% Processor Time");

    struct CpuQuery {
        query: PDH_HQUERY,
        counter: PDH_HCOUNTER,
        primed: bool,
    }

    impl Drop for CpuQuery {
        fn drop(&mut self) {
            // SAFETY: closing the owned query also releases its counter.
            unsafe { PdhCloseQuery(self.query) };
        }
    }

    impl CpuQuery {
        fn new() -> Result<Self, u32> {
            // SAFETY: PDH writes owned handles; all access stays on this thread.
            unsafe {
                let mut query = null_mut();
                let status = PdhOpenQueryW(null(), 0, &mut query);
                if status != 0 {
                    return Err(status);
                }
                let mut query = Self {
                    query,
                    counter: null_mut(),
                    primed: false,
                };
                let status = PdhAddEnglishCounterW(query.query, CPU_COUNTER, 0, &mut query.counter);
                if status != 0 {
                    return Err(status);
                }
                Ok(query)
            }
        }

        fn read(&mut self) -> Result<Option<f64>, u32> {
            // SAFETY: valid query/counter, writable output, and matching DOUBLE union member.
            unsafe {
                let status = PdhCollectQueryData(self.query);
                if status != 0 {
                    return Err(status);
                }
                if !self.primed {
                    // A rate needs two reads one sampling interval apart. Never
                    // publish an instantaneous zero as a measured CPU reading.
                    self.primed = true;
                    return Ok(None);
                }
                let mut value = PDH_FMT_COUNTERVALUE::default();
                let status = PdhGetFormattedCounterValue(
                    self.counter,
                    PDH_FMT_DOUBLE,
                    null_mut(),
                    &mut value,
                );
                if status != 0 {
                    return Err(status);
                }
                if !matches!(value.CStatus, PDH_CSTATUS_VALID_DATA | PDH_CSTATUS_NEW_DATA) {
                    return Err(value.CStatus);
                }
                let percent = value.Anonymous.doubleValue;
                Ok(percent.is_finite().then(|| percent.clamp(0.0, 100.0)))
            }
        }
    }

    pub struct Sampler {
        cpu: Option<CpuQuery>,
        next_cpu_attempt: Instant,
        cpu_failed: bool,
        temperature: Option<super::super::temperature::Reader>,
        gpu: Option<super::super::gpu::Reader>,
    }

    impl Sampler {
        pub fn new(shutdown: Arc<AtomicBool>) -> Self {
            Self {
                cpu: None,
                next_cpu_attempt: Instant::now(),
                cpu_failed: false,
                temperature: super::super::temperature::Reader::start(shutdown.clone()).ok(),
                gpu: super::super::gpu::Reader::start(shutdown).ok(),
            }
        }
    }

    impl Sampler {
        pub fn read(&mut self) -> HardwareSnapshot {
            let now = Instant::now();
            let cpu_percent = if self.cpu.is_none() && now < self.next_cpu_attempt {
                None
            } else {
                let result = (|| {
                    if self.cpu.is_none() {
                        self.cpu = Some(CpuQuery::new()?);
                    }
                    self.cpu.as_mut().expect("query initialized").read()
                })();
                match result {
                    Ok(value) => {
                        if value.is_some() {
                            self.cpu_failed = false;
                        }
                        value
                    }
                    Err(status) => {
                        if !self.cpu_failed {
                            eprintln!("Cannot read system CPU usage: PDH 0x{status:08x}");
                        }
                        self.cpu_failed = true;
                        // Recreate the query and baseline after failure. Back off
                        // when performance counters are disabled/unavailable.
                        self.cpu = None;
                        self.next_cpu_attempt = now + Duration::from_secs(10);
                        None
                    }
                }
            };
            let mut memory = MEMORYSTATUSEX {
                dwLength: size_of::<MEMORYSTATUSEX>() as u32,
                ..Default::default()
            };
            // SAFETY: memory is initialized with the required structure size.
            let memory_percent = if unsafe { GlobalMemoryStatusEx(&mut memory) } != 0 {
                memory_percent(memory.ullTotalPhys, memory.ullAvailPhys)
            } else {
                None
            };
            HardwareSnapshot {
                cpu_percent,
                cpu_temperature_c: self.temperature.as_ref().and_then(|reader| reader.latest()),
                gpu: self
                    .gpu
                    .as_ref()
                    .map(|reader| reader.latest())
                    .unwrap_or_default(),
                memory_percent,
                memory_used_bytes: memory_percent
                    .map(|_| memory.ullTotalPhys - memory.ullAvailPhys),
            }
        }
    }
}

#[cfg(windows)]
pub use windows::Sampler;

#[cfg(not(windows))]
#[derive(Default)]
pub struct Sampler {
    _private: (),
}

#[cfg(not(windows))]
impl Sampler {
    pub fn new(_shutdown: std::sync::Arc<std::sync::atomic::AtomicBool>) -> Self {
        Self::default()
    }

    pub fn read(&mut self) -> tessera_core::hardware::HardwareSnapshot {
        Default::default()
    }
}
