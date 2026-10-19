//! One-time hardware warm-up. Never wait for network or optional temperature.
use std::time::Duration;
use tessera_core::hardware::{HardwareSnapshot, gpu::Presence};

const MAX_WAIT: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Default)]
pub(super) struct Startup {
    shown: bool,
}

impl Startup {
    pub fn can_present(&mut self, hardware: &HardwareSnapshot, elapsed: Duration) -> bool {
        let gpu_ready = match hardware.gpu.presence {
            Presence::Absent => true,
            Presence::Present => hardware.gpu.percent.is_some(),
            Presence::Unknown => false,
        };
        self.shown |= elapsed >= MAX_WAIT
            || (hardware.cpu_percent.is_some() && hardware.memory_percent.is_some() && gpu_ready);
        self.shown
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_core::hardware::gpu;

    #[test]
    fn first_frame_waits_for_hardware_rates_and_detection_but_not_temperature() {
        let mut startup = Startup::default();
        let mut hardware = HardwareSnapshot::default();
        assert!(!startup.can_present(&hardware, Duration::ZERO));
        hardware.cpu_percent = Some(0.0);
        hardware.memory_percent = Some(40.0);
        assert!(!startup.can_present(&hardware, Duration::from_secs(1)));
        hardware.gpu.presence = Presence::Present;
        assert!(!startup.can_present(&hardware, Duration::from_secs(1)));
        hardware.gpu.percent = Some(0.0);
        assert!(startup.can_present(&hardware, Duration::from_secs(1)));
        assert_eq!(hardware.cpu_temperature_c, None);
        // Later outages and Explorer restarts must not run startup warm-up again.
        assert!(startup.can_present(&HardwareSnapshot::default(), Duration::ZERO));
    }

    #[test]
    fn absent_gpu_does_not_delay_cpu_and_ram() {
        let hardware = HardwareSnapshot {
            cpu_percent: Some(12.0),
            memory_percent: Some(42.0),
            gpu: gpu::Snapshot {
                presence: Presence::Absent,
                ..Default::default()
            },
            ..Default::default()
        };
        let snapshot = tessera_core::network::Snapshot {
            hardware,
            ..Default::default()
        };
        assert!(!snapshot.ready); // No network sample or baseline has arrived.
        assert!(Startup::default().can_present(&snapshot.hardware, Duration::from_secs(1)));
    }

    #[test]
    fn unavailable_or_stalled_sources_cannot_block_startup_forever() {
        let mut startup = Startup::default();
        assert!(!startup.can_present(
            &HardwareSnapshot::default(),
            MAX_WAIT - Duration::from_millis(1)
        ));
        assert!(startup.can_present(&HardwareSnapshot::default(), MAX_WAIT));
    }
}
