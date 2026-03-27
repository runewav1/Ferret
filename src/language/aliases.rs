use std::collections::HashMap;

/// Represents a known programming language with its aliases and file extensions
#[derive(Debug, Clone)]
pub struct LanguageInfo {
    pub canonical_name: String,
    pub extensions: Vec<String>,
}

/// Maps language aliases (names, abbreviations) to canonical language info
pub struct LanguageAliasMap {
    by_alias: HashMap<String, LanguageInfo>,
}

impl LanguageAliasMap {
    pub fn new() -> Self {
        let mut by_alias = HashMap::new();

        let languages = vec![
            ("rust", vec!["rust", "rs"], vec![".rs"]),
            ("python", vec!["python", "py"], vec![".py", ".pyw"]),
            (
                "javascript",
                vec!["javascript", "js"],
                vec![".js", ".mjs", ".cjs"],
            ),
            ("typescript", vec!["typescript", "ts"], vec![".ts", ".tsx"]),
            ("java", vec!["java"], vec![".java"]),
            ("c", vec!["c"], vec![".c", ".h"]),
            (
                "cpp",
                vec!["cpp", "c++", "cxx"],
                vec![".cpp", ".cc", ".cxx", ".hpp", ".hxx"],
            ),
            ("csharp", vec!["csharp", "c#", "cs"], vec![".cs"]),
            ("go", vec!["go", "golang"], vec![".go"]),
            ("ruby", vec!["ruby", "rb"], vec![".rb"]),
            ("php", vec!["php"], vec![".php"]),
            ("swift", vec!["swift"], vec![".swift"]),
            ("kotlin", vec!["kotlin", "kt"], vec![".kt", ".kts"]),
            ("scala", vec!["scala"], vec![".scala"]),
            ("haskell", vec!["haskell", "hs"], vec![".hs", ".lhs"]),
            ("elixir", vec!["elixir", "ex"], vec![".ex", ".exs"]),
            ("erlang", vec!["erlang", "erl"], vec![".erl", ".hrl"]),
            (
                "clojure",
                vec!["clojure", "clj"],
                vec![".clj", ".cljs", ".cljc"],
            ),
            ("lua", vec!["lua"], vec![".lua"]),
            ("perl", vec!["perl", "pl"], vec![".pl", ".pm"]),
            ("r", vec!["r"], vec![".r", ".R"]),
            ("dart", vec!["dart"], vec![".dart"]),
            ("zig", vec!["zig"], vec![".zig"]),
            ("nim", vec!["nim"], vec![".nim"]),
            (
                "shell",
                vec!["shell", "sh", "bash", "zsh"],
                vec![".sh", ".bash", ".zsh"],
            ),
            (
                "powershell",
                vec!["powershell", "ps1"],
                vec![".ps1", ".psm1"],
            ),
            ("html", vec!["html"], vec![".html", ".htm"]),
            ("css", vec!["css"], vec![".css"]),
            ("scss", vec!["scss", "sass"], vec![".scss", ".sass"]),
            ("sql", vec!["sql"], vec![".sql"]),
            ("yaml", vec!["yaml", "yml"], vec![".yaml", ".yml"]),
            ("json", vec!["json"], vec![".json"]),
            ("toml", vec!["toml"], vec![".toml"]),
            ("xml", vec!["xml"], vec![".xml"]),
            ("markdown", vec!["markdown", "md"], vec![".md", ".markdown"]),
            ("dockerfile", vec!["dockerfile", "docker"], vec![]),
        ];

        for (canonical, aliases, extensions) in languages {
            let info = LanguageInfo {
                canonical_name: canonical.to_string(),
                extensions: extensions.iter().map(|s| s.to_string()).collect(),
            };
            for alias in aliases {
                by_alias.insert(alias.to_lowercase(), info.clone());
            }
        }

        Self { by_alias }
    }

    /// Look up language info by any alias (case-insensitive)
    pub fn lookup(&self, alias: &str) -> Option<&LanguageInfo> {
        self.by_alias.get(&alias.to_lowercase())
    }

    /// Get the canonical name from an alias
    pub fn canonical_name(&self, alias: &str) -> Option<&str> {
        self.lookup(alias).map(|info| info.canonical_name.as_str())
    }

    /// Get file extensions for a language alias
    pub fn extensions(&self, alias: &str) -> Option<&[String]> {
        self.lookup(alias).map(|info| info.extensions.as_slice())
    }

    /// Check if an alias is recognized
    pub fn is_valid(&self, alias: &str) -> bool {
        self.by_alias.contains_key(&alias.to_lowercase())
    }

    /// Get all known canonical language names
    pub fn all_languages(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self
            .by_alias
            .values()
            .map(|info| info.canonical_name.as_str())
            .collect();
        names.sort();
        names.dedup();
        names
    }
}

impl Default for LanguageAliasMap {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rust_aliases() {
        let map = LanguageAliasMap::new();
        assert_eq!(map.canonical_name("rs"), Some("rust"));
        assert_eq!(map.canonical_name("rust"), Some("rust"));
        assert_eq!(map.canonical_name("RUST"), Some("rust"));
    }

    #[test]
    fn test_python_aliases() {
        let map = LanguageAliasMap::new();
        assert_eq!(map.canonical_name("py"), Some("python"));
        assert_eq!(map.canonical_name("python"), Some("python"));
    }

    #[test]
    fn test_extensions() {
        let map = LanguageAliasMap::new();
        let exts = map.extensions("rust").unwrap();
        assert!(exts.contains(&".rs".to_string()));
    }

    #[test]
    fn test_invalid_alias() {
        let map = LanguageAliasMap::new();
        assert!(map.lookup("unknown_lang").is_none());
    }

    #[test]
    fn test_javascript_aliases() {
        let map = LanguageAliasMap::new();
        assert_eq!(map.canonical_name("js"), Some("javascript"));
        assert_eq!(map.canonical_name("javascript"), Some("javascript"));
        let exts = map.extensions("js").unwrap();
        assert!(exts.contains(&".js".to_string()));
        assert!(exts.contains(&".mjs".to_string()));
        assert!(exts.contains(&".cjs".to_string()));
    }

    #[test]
    fn test_typescript_aliases() {
        let map = LanguageAliasMap::new();
        assert_eq!(map.canonical_name("ts"), Some("typescript"));
        let exts = map.extensions("typescript").unwrap();
        assert!(exts.contains(&".ts".to_string()));
        assert!(exts.contains(&".tsx".to_string()));
    }

    #[test]
    fn test_cpp_aliases() {
        let map = LanguageAliasMap::new();
        assert_eq!(map.canonical_name("cpp"), Some("cpp"));
        assert_eq!(map.canonical_name("c++"), Some("cpp"));
        assert_eq!(map.canonical_name("cxx"), Some("cpp"));
    }

    #[test]
    fn test_csharp_aliases() {
        let map = LanguageAliasMap::new();
        assert_eq!(map.canonical_name("cs"), Some("csharp"));
        assert_eq!(map.canonical_name("c#"), Some("csharp"));
        assert_eq!(map.canonical_name("csharp"), Some("csharp"));
    }

    #[test]
    fn test_go_aliases() {
        let map = LanguageAliasMap::new();
        assert_eq!(map.canonical_name("go"), Some("go"));
        assert_eq!(map.canonical_name("golang"), Some("go"));
    }

    #[test]
    fn test_shell_aliases() {
        let map = LanguageAliasMap::new();
        assert_eq!(map.canonical_name("sh"), Some("shell"));
        assert_eq!(map.canonical_name("bash"), Some("shell"));
        assert_eq!(map.canonical_name("zsh"), Some("shell"));
    }

    #[test]
    fn test_is_valid() {
        let map = LanguageAliasMap::new();
        assert!(map.is_valid("rust"));
        assert!(map.is_valid("rs"));
        assert!(map.is_valid("PYTHON"));
        assert!(!map.is_valid("fakeLang"));
    }

    #[test]
    fn test_all_languages() {
        let map = LanguageAliasMap::new();
        let langs = map.all_languages();
        assert!(langs.contains(&"rust"));
        assert!(langs.contains(&"python"));
        assert!(langs.contains(&"javascript"));
        assert!(langs.len() > 30);
    }

    #[test]
    fn test_case_insensitivity() {
        let map = LanguageAliasMap::new();
        assert_eq!(map.canonical_name("RUST"), map.canonical_name("rust"));
        assert_eq!(map.canonical_name("Python"), map.canonical_name("PYTHON"));
        assert_eq!(map.canonical_name("JS"), map.canonical_name("js"));
    }
}
