//! How the player behaves, set on the grown-ups' page and kept on the server, so
//! every signed-in device follows the same settings.

use std::{path::Path, sync::Mutex};

use serde::{Deserialize, Serialize};

use crate::{Error, Result, store::JsonStore};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Tapping a playlist shows its songs to pick from, instead of playing it.
    pub song_list: bool,
    /// Playlists play in a shuffled order, every song once before any repeats.
    pub shuffle: bool,
    /// Only the play button: no previous or next.
    pub simple_controls: bool,
}

pub struct SettingsStore {
    store: Mutex<JsonStore<Settings>>,
}

impl SettingsStore {
    pub fn new(state_dir: &Path) -> Self {
        Self {
            store: Mutex::new(JsonStore::load(state_dir, "settings.json")),
        }
    }

    pub fn get(&self) -> Settings {
        self.store
            .lock()
            .map(|s| s.data.clone())
            .unwrap_or_default()
    }

    pub fn set(&self, settings: Settings) -> Result<Settings> {
        let mut store = self
            .store
            .lock()
            .map_err(|_| Error::Internal("settings lock poisoned".into()))?;
        store.data = settings;
        store.save()?;
        Ok(store.data.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_persist_and_missing_ones_default_off() {
        let dir = std::env::temp_dir().join(format!("songden-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = SettingsStore::new(&dir);
        assert_eq!(store.get(), Settings::default());
        store
            .set(Settings {
                shuffle: true,
                ..Settings::default()
            })
            .unwrap();
        assert!(SettingsStore::new(&dir).get().shuffle);
        // an older file without a newer setting still loads, with it off
        std::fs::write(dir.join("settings.json"), r#"{"songList": true}"#).unwrap();
        let loaded = SettingsStore::new(&dir).get();
        assert!(loaded.song_list && !loaded.shuffle && !loaded.simple_controls);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
