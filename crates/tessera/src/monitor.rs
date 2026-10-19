use crate::state::AppState;
use std::{
    sync::atomic::Ordering,
    thread,
    time::{Duration, Instant},
};
use tessera_core::network::{PauseState, Sampler, Snapshot};

// Fixed cadence; rates still use the actual monotonic time between counter reads.
const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);

mod hardware;

#[cfg(windows)]
mod temperature;

#[cfg(windows)]
mod gpu;

#[cfg(windows)]
mod pdh;

#[cfg(windows)]
mod windows;

#[cfg(windows)]
use windows::read;

#[cfg(not(windows))]
fn read() -> std::io::Result<std::collections::BTreeMap<u64, tessera_core::network::Adapter>> {
    // Portable tests never read unfiltered host network counters.
    Ok(Default::default())
}

pub fn start(state: AppState) -> std::io::Result<()> {
    start_hardware(state.clone())?;
    thread::Builder::new()
        .name("network-sampler".into())
        .spawn(move || {
            let mut sampler = Sampler::default();
            let mut previous = Instant::now();
            let mut session_download: u64 = 0;
            let mut session_upload: u64 = 0;
            let mut previous_adapter = None;
            let mut previous_pause = PauseState::default();
            let mut read_failed = false;
            while !state.shutdown.load(Ordering::Relaxed) {
                let next_sample = Instant::now() + SAMPLE_INTERVAL;
                let Ok(settings) = state.settings.read().map(|s| s.clone()) else {
                    break;
                };
                let current = match read() {
                    Ok(current) => {
                        read_failed = false;
                        current
                    }
                    Err(error) => {
                        if !read_failed {
                            eprintln!("Cannot read physical network adapters: {error}");
                        }
                        read_failed = true;
                        // Clear baselines on failure. Do not fall back to virtual
                        // counters or divide several missed intervals by one interval.
                        Default::default()
                    }
                };
                let now = Instant::now();
                let mut adapters: Vec<_> = current.values().map(|a| a.name.clone()).collect();
                adapters.sort();
                adapters.dedup();
                if settings.adapter != previous_adapter {
                    session_download = 0;
                    session_upload = 0;
                    previous_adapter = settings.adapter.clone();
                }
                let sample = sampler.sample(
                    current,
                    now.duration_since(previous),
                    settings.adapter.as_deref(),
                );
                previous = now;
                // Refresh baselines even while paused; skip the resume boundary to exclude paused traffic.
                let Ok(pause) = state.paused.lock().map(|pause| *pause) else {
                    break;
                };
                let counting = pause.counts_interval_after(previous_pause);
                previous_pause = pause;
                if counting {
                    session_download = session_download.saturating_add(sample.received);
                    session_upload = session_upload.saturating_add(sample.transmitted);
                }
                let snapshot = Snapshot {
                    hardware: Default::default(),
                    download_bps: if counting { sample.download_bps } else { 0.0 },
                    upload_bps: if counting { sample.upload_bps } else { 0.0 },
                    session_download,
                    session_upload,
                    adapter_missing: settings
                        .adapter
                        .as_ref()
                        .is_some_and(|selected| !adapters.contains(selected)),
                    adapters,
                    ready: sample.ready,
                    paused: pause.paused,
                };
                state.publish_network(snapshot);
                // Include sampling/publication work in the one-second budget.
                // Recheck after early/spurious wakes; slow reads do not queue catch-up samples.
                while !state.shutdown.load(Ordering::Relaxed) {
                    let remaining = next_sample.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        break;
                    }
                    thread::park_timeout(remaining);
                }
            }
        })?;
    Ok(())
}

fn start_hardware(state: AppState) -> std::io::Result<()> {
    thread::Builder::new()
        .name("hardware-sampler".into())
        .spawn(move || {
            let mut sampler = hardware::Sampler::new(state.shutdown.clone());
            while !state.shutdown.load(Ordering::Relaxed) {
                let next = Instant::now() + SAMPLE_INTERVAL;
                let hardware = sampler.read();
                if let Ok(mut snapshot) = state.snapshot.write() {
                    snapshot.hardware = hardware;
                }
                while !state.shutdown.load(Ordering::Relaxed) {
                    let remaining = next.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        break;
                    }
                    thread::park_timeout(remaining);
                }
            }
        })?;
    Ok(())
}
