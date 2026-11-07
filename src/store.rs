//! Small JSON files in the state directory, written atomically.

use std::{
    fs::{OpenOptions, Permissions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use serde::{Serialize, de::DeserializeOwned};
use tracing::warn;

use crate::Result;

/// Owner only: `sessions.json` holds live session tokens.
const FILE_MODE: u32 = 0o600;

pub struct JsonStore<T> {
    path: PathBuf,
    pub data: T,
}

impl<T: Serialize + DeserializeOwned + Default> JsonStore<T> {
    /// Loads `dir/name`, or starts empty if it is missing or unreadable.
    pub fn load(dir: &Path, name: &str) -> Self {
        let path = dir.join(name);
        let data = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|err| {
                warn!("{}: unreadable, starting empty: {err}", path.display());
                T::default()
            }),
            Err(_) => T::default(),
        };
        Self { path, data }
    }

    /// Writes to a temporary file and renames it over the old one, so a crash
    /// mid-write never leaves a half-written file behind.
    pub fn save(&self) -> Result<()> {
        let tmp = self.path.with_extension("json.tmp");
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(FILE_MODE)
            .open(&tmp)?;
        // a leftover from a crash keeps its old mode through `mode` above
        file.set_permissions(Permissions::from_mode(FILE_MODE))?;
        file.write_all(&serde_json::to_vec_pretty(&self.data)?)?;
        drop(file);
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::JsonStore;
    use std::{collections::BTreeMap, os::unix::fs::PermissionsExt};

    #[test]
    fn saves_and_reloads() {
        let dir = std::env::temp_dir().join(format!("songden-store-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut store: JsonStore<BTreeMap<String, u32>> = JsonStore::load(&dir, "t.json");
        assert!(store.data.is_empty());
        store.data.insert("a".into(), 1);
        store.save().unwrap();
        let again: JsonStore<BTreeMap<String, u32>> = JsonStore::load(&dir, "t.json");
        assert_eq!(again.data.get("a"), Some(&1));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_its_owner_can_read_a_saved_file() {
        let dir = std::env::temp_dir().join(format!("songden-mode-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.json");
        let mode = || std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        // a file from before this, and a temporary one left by a crash
        std::fs::write(&path, "{}").unwrap();
        std::fs::write(path.with_extension("json.tmp"), "{}").unwrap();
        for file in [path.clone(), path.with_extension("json.tmp")] {
            std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        assert_eq!(mode(), 0o644);
        let store: JsonStore<BTreeMap<String, u32>> = JsonStore::load(&dir, "t.json");
        store.save().unwrap();
        assert_eq!(mode(), 0o600);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
