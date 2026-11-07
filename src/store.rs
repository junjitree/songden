//! Small JSON files in the state directory, written atomically.

use std::path::{Path, PathBuf};

use serde::{Serialize, de::DeserializeOwned};
use tracing::warn;

use crate::Result;

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
        std::fs::write(&tmp, serde_json::to_vec_pretty(&self.data)?)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::JsonStore;
    use std::collections::BTreeMap;

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
}
