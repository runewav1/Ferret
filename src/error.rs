use thiserror::Error;

#[derive(Error, Debug)]
pub enum FerretError {
    #[error("Registry error: {0}")]
    RegistryError(String),

    #[error("Git error: {0}")]
    GitError(String),

    #[error("Config error: {0}")]
    ConfigError(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Not a git repository: {0}")]
    NotAGitRepo(String),

    #[error("Repository not found: {0}")]
    NotFound(String),

    #[error("Invalid path: {0}")]
    PathError(String),

    #[error("Duplicate entry: {0} already exists in registry")]
    DuplicateEntry(String),

    #[error("Remote error: {0}")]
    RemoteError(String),

    #[error("Config parse error: {0}")]
    ConfigParseError(String),

    #[error("Parse error: {0}")]
    ParseError(String),
}

impl From<git2::Error> for FerretError {
    fn from(err: git2::Error) -> Self {
        FerretError::GitError(err.to_string())
    }
}

impl From<serde_json::Error> for FerretError {
    fn from(err: serde_json::Error) -> Self {
        FerretError::ParseError(format!("JSON: {}", err))
    }
}

impl From<toml::de::Error> for FerretError {
    fn from(err: toml::de::Error) -> Self {
        FerretError::ConfigParseError(format!("TOML parse: {}", err))
    }
}

impl FerretError {
    pub fn context(self, msg: &str) -> Self {
        match self {
            FerretError::RegistryError(e) => FerretError::RegistryError(format!("{}: {}", msg, e)),
            FerretError::GitError(e) => FerretError::GitError(format!("{}: {}", msg, e)),
            FerretError::ConfigError(e) => FerretError::ConfigError(format!("{}: {}", msg, e)),
            FerretError::ConfigParseError(e) => {
                FerretError::ConfigParseError(format!("{}: {}", msg, e))
            }
            FerretError::IoError(e) => {
                FerretError::IoError(std::io::Error::new(e.kind(), format!("{}: {}", msg, e)))
            }
            other => other,
        }
    }
}

pub type Result<T> = std::result::Result<T, FerretError>;
