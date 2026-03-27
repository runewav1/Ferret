use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::entry::RegistryEntry;
use crate::pathutil;

/// The stored registry data
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RegistryStore {
    /// All registered entries
    pub entries: Vec<RegistryEntry>,
    /// Schema version for migration support
    pub version: u32,
}

impl RegistryStore {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            version: 1,
        }
    }
}

/// Handles loading and saving the registry
pub struct RegistryStorage {
    path: PathBuf,
}

impl RegistryStorage {
    /// Create a new storage handler using the default data directory
    pub fn new() -> crate::error::Result<Self> {
        let data_dir = pathutil::ferret_data_dir().ok_or_else(|| {
            crate::error::FerretError::RegistryError("Cannot determine data directory".to_string())
        })?;

        Ok(Self {
            path: data_dir.join("registry.json"),
        })
    }

    /// Create a storage handler with a custom path (useful for testing)
    pub fn with_path(path: PathBuf) -> Self {
        Self { path }
    }

    /// Load the registry from disk
    pub fn load(&self) -> crate::error::Result<RegistryStore> {
        if !self.path.exists() {
            return Ok(RegistryStore::new());
        }

        let content =
            std::fs::read_to_string(&self.path).map_err(crate::error::FerretError::IoError)?;

        let store: RegistryStore = serde_json::from_str(&content)?;
        Ok(store)
    }

    /// Save the registry to disk
    pub fn save(&self, store: &RegistryStore) -> crate::error::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(crate::error::FerretError::IoError)?;
        }

        let content =
            serde_json::to_string_pretty(store).map_err(crate::error::FerretError::from)?;

        std::fs::write(&self.path, content).map_err(crate::error::FerretError::IoError)?;

        Ok(())
    }

    /// Get the path to the registry file
    pub fn path(&self) -> &Path {
        &self.path
    }
}
