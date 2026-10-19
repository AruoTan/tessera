use std::{
    path::PathBuf,
    sync::{Arc, Mutex, RwLock, atomic::AtomicBool},
};
use tessera_core::{
    config::Settings,
    network::{PauseState, Snapshot},
};

#[derive(Clone)]
pub struct AppState {
    pub settings: Arc<RwLock<Settings>>,
    pub settings_path: PathBuf,
    pub snapshot: Arc<RwLock<Snapshot>>,
    pub paused: Arc<Mutex<PauseState>>,
    pub shutdown: Arc<AtomicBool>,
}

impl AppState {
    /// Network publication must not overwrite newer, independently sampled hardware.
    pub fn publish_network(&self, mut snapshot: Snapshot) {
        if let Ok(mut latest) = self.snapshot.write() {
            snapshot.hardware = std::mem::take(&mut latest.hardware);
            *latest = snapshot;
        }
    }

    /// Form saves must not undo a position dragged while the form was open.
    pub fn save_settings(&self, mut settings: Settings) -> Result<Settings, String> {
        let mut current = self.settings.write().map_err(|e| e.to_string())?;
        settings.taskbar_position = current.taskbar_position;
        settings.offset_x = current.offset_x;
        tessera_core::config::save(&self.settings_path, &settings)?;
        *current = settings.clone();
        Ok(settings)
    }

    pub fn set_widget_position(&self, position: Option<u16>) -> Result<(), String> {
        let mut current = self.settings.write().map_err(|e| e.to_string())?;
        let mut settings = current.clone();
        settings.taskbar_position = position;
        settings.offset_x = 0;
        tessera_core::config::save(&self.settings_path, &settings)?;
        *current = settings;
        Ok(())
    }

    pub fn new(settings: Settings, settings_path: PathBuf) -> Self {
        Self {
            settings: Arc::new(RwLock::new(settings)),
            settings_path,
            snapshot: Arc::new(RwLock::new(Snapshot::default())),
            paused: Arc::new(Mutex::new(PauseState::default())),
            shutdown: Arc::new(AtomicBool::new(false)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_updates_preserve_independently_published_hardware() {
        let state = AppState::new(Settings::default(), PathBuf::new());
        state.snapshot.write().unwrap().hardware.cpu_percent = Some(37.0);
        state.publish_network(Snapshot {
            upload_bps: 128.0,
            ..Default::default()
        });
        let snapshot = state.snapshot.read().unwrap();
        assert_eq!(snapshot.hardware.cpu_percent, Some(37.0));
        assert_eq!(snapshot.upload_bps, 128.0);
        drop(snapshot);
        state.snapshot.write().unwrap().hardware.cpu_percent = Some(42.0);
        state.publish_network(Snapshot {
            ready: true,
            ..Default::default()
        });
        assert_eq!(
            state.snapshot.read().unwrap().hardware.cpu_percent,
            Some(42.0)
        );
    }

    #[test]
    fn stale_form_save_keeps_dragged_position_and_reset_is_persisted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let state = AppState::new(Settings::default(), path.clone());
        let mut stale_form = state.settings.read().unwrap().clone();
        state.set_widget_position(Some(3_456)).unwrap();
        stale_form.adapter = Some("Wi-Fi".into());
        let saved = state.save_settings(stale_form).unwrap();
        assert_eq!(saved.taskbar_position, Some(3_456));
        assert_eq!(saved.adapter.as_deref(), Some("Wi-Fi"));
        assert_eq!(tessera_core::config::load(&path).unwrap(), saved);
        state.set_widget_position(None).unwrap();
        let reset = tessera_core::config::load(&path).unwrap();
        assert_eq!(reset.taskbar_position, None);
        assert_eq!(reset.offset_x, 0);
        assert_eq!(reset.adapter.as_deref(), Some("Wi-Fi"));
    }

    #[test]
    fn failed_drag_save_does_not_publish_unsaved_position() {
        let dir = tempfile::tempdir().unwrap();
        // A directory cannot be atomically replaced by a settings file.
        let state = AppState::new(Settings::default(), dir.path().to_path_buf());
        assert!(state.set_widget_position(Some(5_000)).is_err());
        assert_eq!(*state.settings.read().unwrap(), Settings::default());
    }
}
