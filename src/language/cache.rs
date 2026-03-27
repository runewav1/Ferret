use std::collections::HashMap;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::detector::LanguageDetector;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanguageCache {
    entries: HashMap<String, CacheEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheEntry {
    languages: Vec<String>,
    scanned_at: DateTime<Utc>,
    directory_mtime: Option<i64>,
}

impl LanguageCache {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    pub fn get_or_detect(
        &mut self,
        dir: &Path,
        detector: &LanguageDetector,
        max_age_seconds: i64,
    ) -> crate::error::Result<Vec<String>> {
        let key = dir.to_string_lossy().to_string();

        if let Some(entry) = self.entries.get(&key) {
            let age = Utc::now().signed_duration_since(entry.scanned_at);
            if age.num_seconds() < max_age_seconds {
                return Ok(entry.languages.clone());
            }
        }

        let languages = detector.detect_language_names(dir)?;

        self.entries.insert(
            key,
            CacheEntry {
                languages: languages.clone(),
                scanned_at: Utc::now(),
                directory_mtime: None,
            },
        );

        Ok(languages)
    }

    pub fn refresh(
        &mut self,
        dir: &Path,
        detector: &LanguageDetector,
    ) -> crate::error::Result<Vec<String>> {
        let key = dir.to_string_lossy().to_string();
        let languages = detector.detect_language_names(dir)?;

        self.entries.insert(
            key,
            CacheEntry {
                languages: languages.clone(),
                scanned_at: Utc::now(),
                directory_mtime: None,
            },
        );

        Ok(languages)
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn remove(&mut self, dir: &Path) {
        let key = dir.to_string_lossy().to_string();
        self.entries.remove(&key);
    }

    pub fn save(&self, path: &Path) -> crate::error::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(crate::error::FerretError::IoError)?;
        }
        let content =
            serde_json::to_string_pretty(self).map_err(crate::error::FerretError::from)?;
        std::fs::write(path, content).map_err(crate::error::FerretError::IoError)?;
        Ok(())
    }

    pub fn load(path: &Path) -> crate::error::Result<Self> {
        if !path.exists() {
            return Ok(Self::new());
        }
        let content = std::fs::read_to_string(path).map_err(crate::error::FerretError::IoError)?;
        let cache: Self = serde_json::from_str(&content)?;
        Ok(cache)
    }
}

impl Default for LanguageCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_cache_hit() {
        let temp_dir = std::env::temp_dir().join("ferret_cache_test_hit");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();
        fs::write(temp_dir.join("main.rs"), "fn main() {}").unwrap();

        let detector = LanguageDetector::new();
        let mut cache = LanguageCache::new();

        let first = cache.get_or_detect(&temp_dir, &detector, 3600).unwrap();
        assert!(!first.is_empty());

        let second = cache.get_or_detect(&temp_dir, &detector, 3600).unwrap();
        assert_eq!(first, second);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_cache_expiration() {
        let temp_dir = std::env::temp_dir().join("ferret_cache_test_expire");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();
        fs::write(temp_dir.join("main.rs"), "fn main() {}").unwrap();

        let detector = LanguageDetector::new();
        let mut cache = LanguageCache::new();

        let _ = cache.get_or_detect(&temp_dir, &detector, 0).unwrap();
        let key = temp_dir.to_string_lossy().to_string();
        assert!(cache.entries.contains_key(&key));

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_cache_save_load() {
        let temp_dir = std::env::temp_dir().join("ferret_cache_test_persist");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();
        fs::write(temp_dir.join("app.py"), "print('hi')").unwrap();

        let cache_file = temp_dir.join("cache.json");
        let detector = LanguageDetector::new();
        let mut cache = LanguageCache::new();

        let _ = cache.get_or_detect(&temp_dir, &detector, 3600).unwrap();
        cache.save(&cache_file).unwrap();

        let loaded = LanguageCache::load(&cache_file).unwrap();
        let key = temp_dir.to_string_lossy().to_string();
        assert!(loaded.entries.contains_key(&key));

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_cache_refresh() {
        let temp_dir = std::env::temp_dir().join("ferret_cache_test_refresh");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();
        fs::write(temp_dir.join("main.rs"), "fn main() {}").unwrap();

        let detector = LanguageDetector::new();
        let mut cache = LanguageCache::new();

        let _ = cache.get_or_detect(&temp_dir, &detector, 3600).unwrap();
        let refreshed = cache.refresh(&temp_dir, &detector).unwrap();
        assert!(!refreshed.is_empty());

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_cache_clear_and_remove() {
        let temp_dir = std::env::temp_dir().join("ferret_cache_test_clear");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();
        fs::write(temp_dir.join("index.js"), "export default {}").unwrap();

        let detector = LanguageDetector::new();
        let mut cache = LanguageCache::new();

        let _ = cache.get_or_detect(&temp_dir, &detector, 3600).unwrap();

        cache.remove(&temp_dir);
        let key = temp_dir.to_string_lossy().to_string();
        assert!(!cache.entries.contains_key(&key));

        let _ = cache.get_or_detect(&temp_dir, &detector, 3600).unwrap();
        cache.clear();
        assert!(cache.entries.is_empty());

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
