use serde::{Deserialize, Serialize};
use std::{fs, io::Write, path::Path};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct Settings {
    pub schema_version: u32,
    /// None monitors all physical Ethernet/Wi-Fi adapters; virtual interfaces are excluded.
    pub adapter: Option<String>,
    /// Logical pixels from the default taskbar position; positive moves right.
    pub offset_x: i32,
    /// Position along the usable taskbar, in ten-thousandths (0 = start, 10000 = tray).
    /// None uses automatic placement and the horizontal offset.
    pub taskbar_position: Option<u16>,
    /// Prevent mouse dragging without changing the saved taskbar position.
    pub position_locked: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            adapter: None,
            offset_x: 0,
            taskbar_position: None,
            position_locked: false,
        }
    }
}

impl Settings {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != SCHEMA_VERSION {
            return Err("Unsupported settings version. Your settings were not changed.".into());
        }
        if !(-1_000..=1_000).contains(&self.offset_x) {
            return Err("Horizontal offset must be between -1000 and 1000 px.".into());
        }
        if self
            .taskbar_position
            .is_some_and(|position| position > 10_000)
        {
            return Err("Taskbar position must be between 0 and 10000.".into());
        }
        if self
            .adapter
            .as_ref()
            .is_some_and(|s| s.trim().is_empty() || s.len() > 512)
        {
            return Err("Select a valid network adapter.".into());
        }
        Ok(())
    }
}

pub fn load(path: &Path) -> Result<Settings, String> {
    let contents = match fs::read(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Settings::default());
        }
        Err(error) => return Err(format!("Cannot read settings: {error}")),
    };
    let settings: Settings =
        serde_json::from_slice(&contents).map_err(|e| format!("Invalid settings: {e}"))?;
    settings.validate()?;
    Ok(settings)
}

/// Replace the file atomically, including on Windows. Never truncate a valid file first.
pub fn save(path: &Path, settings: &Settings) -> Result<(), String> {
    settings.validate()?;
    let parent = path.parent().ok_or("Settings directory is missing.")?;
    fs::create_dir_all(parent).map_err(|e| format!("Cannot create settings directory: {e}"))?;
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    let contents = serde_json::to_vec_pretty(settings).map_err(|e| e.to_string())?;
    file.write_all(&contents)
        .and_then(|()| file.write_all(b"\n"))
        .map_err(|e| e.to_string())?;
    file.as_file().sync_all().map_err(|e| e.to_string())?;
    file.persist(path)
        .map_err(|e| format!("Cannot save settings: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_uses_defaults_and_partial_file_fills_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        assert_eq!(load(&path).unwrap(), Settings::default());
        fs::write(&path, r#"{"adapter":"Wi-Fi"}"#).unwrap();
        assert_eq!(load(&path).unwrap().adapter.as_deref(), Some("Wi-Fi"));
        assert_eq!(load(&path).unwrap().offset_x, 0);
        assert_eq!(load(&path).unwrap().taskbar_position, None);
        assert!(!load(&path).unwrap().position_locked);
    }

    #[test]
    fn replacement_round_trip_and_invalid_save_preserve_previous_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/settings.json");
        let mut settings = Settings::default();
        save(&path, &settings).unwrap();
        settings.adapter = Some("Ethernet".into());
        settings.taskbar_position = Some(4_321);
        settings.position_locked = true;
        save(&path, &settings).unwrap();
        assert_eq!(load(&path).unwrap(), settings);
        let previous = fs::read(&path).unwrap();
        settings.taskbar_position = Some(10_001);
        assert!(save(&path, &settings).is_err());
        assert_eq!(fs::read(&path).unwrap(), previous);
    }

    #[test]
    fn corrupt_or_future_settings_are_not_silently_reset() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        for contents in ["{", r#"{"schema_version":99}"#] {
            fs::write(&path, contents).unwrap();
            assert!(load(&path).is_err());
            assert_eq!(fs::read_to_string(&path).unwrap(), contents);
        }
    }

    #[test]
    fn retired_sampling_interval_is_ignored_and_removed_on_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(
            &path,
            r#"{"poll_interval_ms":5000,"adapter":"Wi-Fi","taskbar_position":4321}"#,
        )
        .unwrap();
        let settings = load(&path).unwrap();
        assert_eq!(settings.adapter.as_deref(), Some("Wi-Fi"));
        assert_eq!(settings.taskbar_position, Some(4321));
        save(&path, &settings).unwrap();
        let saved: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert!(saved.get("poll_interval_ms").is_none());
        assert_eq!(load(&path).unwrap(), settings);
    }

    #[test]
    fn retired_visibility_preference_is_ignored_and_removed_on_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(
            &path,
            r#"{"widget_visible":false,"adapter":"Ethernet","taskbar_position":4321,"offset_x":42}"#,
        )
        .unwrap();
        let settings = load(&path).unwrap();
        assert_eq!(settings.adapter.as_deref(), Some("Ethernet"));
        assert_eq!(settings.taskbar_position, Some(4321));
        assert_eq!(settings.offset_x, 42);
        save(&path, &settings).unwrap();
        let saved: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert!(saved.get("widget_visible").is_none());
        assert_eq!(load(&path).unwrap(), settings);
    }

    #[test]
    fn retired_bit_mode_is_ignored_and_removed_without_losing_position() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(
            &path,
            r#"{"unit":"bits","adapter":"Wi-Fi","taskbar_position":4321}"#,
        )
        .unwrap();
        let settings = load(&path).unwrap();
        assert_eq!(settings.adapter.as_deref(), Some("Wi-Fi"));
        assert_eq!(settings.taskbar_position, Some(4321));
        save(&path, &settings).unwrap();
        let saved: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert!(saved.get("unit").is_none());
        assert_eq!(load(&path).unwrap(), settings);
    }

    #[test]
    fn retired_appearance_preferences_are_ignored_and_removed_on_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        for appearance in [
            r##""decimals":0,"upload_color":"#123456","download_color":"#ABCDEF""##,
            r#""decimals":"invalid","upload_color":null,"download_color":false"#,
        ] {
            fs::write(
                &path,
                format!(
                    r#"{{"adapter":"Wi-Fi","offset_x":42,"taskbar_position":4321,{appearance}}}"#
                ),
            )
            .unwrap();
            let settings = load(&path).unwrap();
            assert_eq!(settings.adapter.as_deref(), Some("Wi-Fi"));
            assert_eq!(settings.offset_x, 42);
            assert_eq!(settings.taskbar_position, Some(4321));
            save(&path, &settings).unwrap();
            let saved: serde_json::Value =
                serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            for field in ["decimals", "upload_color", "download_color"] {
                assert!(saved.get(field).is_none());
            }
            assert_eq!(load(&path).unwrap(), settings);
        }
    }

    #[test]
    fn rejects_unbounded_values_and_invalid_adapters() {
        let mut settings = Settings {
            offset_x: 1_001,
            ..Settings::default()
        };
        assert!(settings.validate().is_err());
        settings.offset_x = 0;
        settings.taskbar_position = Some(10_001);
        assert!(settings.validate().is_err());
        settings.taskbar_position = None;
        for adapter in [" ".into(), "a".repeat(513)] {
            settings.adapter = Some(adapter);
            assert!(settings.validate().is_err());
        }
    }
}
