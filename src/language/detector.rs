use std::collections::HashMap;
use std::path::Path;

use super::aliases::LanguageAliasMap;

/// Detects programming languages in a directory by scanning file extensions
pub struct LanguageDetector {
    alias_map: LanguageAliasMap,
}

impl LanguageDetector {
    pub fn new() -> Self {
        Self {
            alias_map: LanguageAliasMap::new(),
        }
    }

    /// Scan a directory and return detected languages with their file counts
    pub fn detect(&self, dir: &Path) -> crate::error::Result<Vec<DetectedLanguage>> {
        if !dir.is_dir() {
            return Err(crate::error::FerretError::PathError(format!(
                "Not a directory: {}",
                dir.display()
            )));
        }

        let mut extension_counts: HashMap<String, usize> = HashMap::new();
        self.scan_directory(dir, &mut extension_counts)?;

        let mut language_counts: HashMap<String, usize> = HashMap::new();
        for (ext, count) in &extension_counts {
            for lang_name in self.alias_map.all_languages() {
                if let Some(lang_exts) = self.alias_map.extensions(lang_name) {
                    if lang_exts.iter().any(|le| {
                        le == ext.as_str()
                            || le.trim_start_matches('.') == ext.trim_start_matches('.')
                    }) {
                        *language_counts.entry(lang_name.to_string()).or_insert(0) += count;
                        break;
                    }
                }
            }
        }

        let mut detected: Vec<DetectedLanguage> = language_counts
            .into_iter()
            .map(|(name, count)| DetectedLanguage {
                language: name,
                file_count: count,
            })
            .collect();

        detected.sort_by(|a, b| b.file_count.cmp(&a.file_count));
        Ok(detected)
    }

    /// Recursively scan a directory, counting file extensions
    fn scan_directory(
        &self,
        dir: &Path,
        extension_counts: &mut HashMap<String, usize>,
    ) -> crate::error::Result<()> {
        let entries = std::fs::read_dir(dir).map_err(crate::error::FerretError::IoError)?;

        for entry in entries {
            let entry = entry.map_err(crate::error::FerretError::IoError)?;
            let path = entry.path();

            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with('.')
                    || name == "target"
                    || name == "node_modules"
                    || name == "vendor"
                    || name == "__pycache__"
                    || name == "dist"
                    || name == "build"
                {
                    continue;
                }
            }

            if path.is_dir() {
                self.scan_directory(&path, extension_counts)?;
            } else if path.is_file() {
                if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                    let ext_with_dot = format!(".{}", ext.to_lowercase());
                    *extension_counts.entry(ext_with_dot).or_insert(0) += 1;
                }
            }
        }

        Ok(())
    }

    /// Get the list of detected language names (canonical) from a directory
    pub fn detect_language_names(&self, dir: &Path) -> crate::error::Result<Vec<String>> {
        let detected = self.detect(dir)?;
        Ok(detected.into_iter().map(|d| d.language).collect())
    }
}

impl Default for LanguageDetector {
    fn default() -> Self {
        Self::new()
    }
}

/// Represents a detected language with its file count
#[derive(Debug, Clone)]
pub struct DetectedLanguage {
    pub language: String,
    pub file_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_detect_in_temp_dir() {
        let temp_dir = std::env::temp_dir().join("ferret_test_lang_detect");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        fs::write(temp_dir.join("main.rs"), "fn main() {}").unwrap();
        fs::write(temp_dir.join("lib.rs"), "pub mod foo;").unwrap();
        fs::write(temp_dir.join("readme.md"), "# Test").unwrap();

        let detector = LanguageDetector::new();
        let detected = detector.detect(&temp_dir).unwrap();

        assert!(!detected.is_empty());
        let rust_entry = detected.iter().find(|d| d.language == "rust");
        assert!(rust_entry.is_some());
        assert_eq!(rust_entry.unwrap().file_count, 2);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_skips_hidden_and_special_dirs() {
        let temp_dir = std::env::temp_dir().join("ferret_test_skip_dirs");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        fs::create_dir_all(temp_dir.join("node_modules")).unwrap();
        fs::write(
            temp_dir.join("node_modules/index.js"),
            "module.exports = {};",
        )
        .unwrap();

        fs::create_dir_all(temp_dir.join("target")).unwrap();
        fs::write(temp_dir.join("target/main.rs"), "fn main() {}").unwrap();

        fs::write(temp_dir.join("app.js"), "console.log('hi');").unwrap();

        let detector = LanguageDetector::new();
        let detected = detector.detect(&temp_dir).unwrap();

        let js_entry = detected.iter().find(|d| d.language == "javascript");
        assert!(js_entry.is_some());
        assert_eq!(js_entry.unwrap().file_count, 1);

        let rust_entry = detected.iter().find(|d| d.language == "rust");
        assert!(rust_entry.is_none());

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_error_on_non_directory() {
        let temp_file = std::env::temp_dir().join("ferret_test_not_a_dir.txt");
        fs::write(&temp_file, "test").unwrap();

        let detector = LanguageDetector::new();
        let result = detector.detect(&temp_file);

        assert!(result.is_err());

        let _ = fs::remove_file(&temp_file);
    }

    #[test]
    fn test_sorted_by_file_count() {
        let temp_dir = std::env::temp_dir().join("ferret_test_sorted");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        fs::write(temp_dir.join("a.rs"), "").unwrap();
        fs::write(temp_dir.join("b.py"), "").unwrap();
        fs::write(temp_dir.join("c.py"), "").unwrap();
        fs::write(temp_dir.join("d.py"), "").unwrap();

        let detector = LanguageDetector::new();
        let detected = detector.detect(&temp_dir).unwrap();

        assert_eq!(detected[0].language, "python");
        assert_eq!(detected[0].file_count, 3);
        assert_eq!(detected[1].language, "rust");
        assert_eq!(detected[1].file_count, 1);

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
