use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("could not determine the current directory: {0}")]
    CurrentDirectory(#[source] std::io::Error),

    #[error("no .forest.toml found; pass --config or set FOREST_CONFIG")]
    ConfigNotFound,

    #[error("could not resolve configuration path {path}: {source}")]
    ResolveConfig {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("could not read configuration {path}: {source}")]
    ReadConfig {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("could not parse configuration {path}: {source}")]
    ParseConfig {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error("invalid configuration: {0}")]
    InvalidConfig(String),

    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("git command failed to start: {0}")]
    StartGit(#[source] std::io::Error),

    #[error("{context}: {message}")]
    Git { context: String, message: String },

    #[error("{0}")]
    Operational(String),

    #[error("{context}: {source}")]
    Filesystem {
        context: String,
        #[source]
        source: std::io::Error,
    },

    #[error("could not write output: {0}")]
    WriteOutput(#[source] std::io::Error),

    #[error("could not serialize JSON output: {0}")]
    SerializeJson(#[source] serde_json::Error),
}

impl AppError {
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::ConfigNotFound
            | Self::ResolveConfig { .. }
            | Self::ReadConfig { .. }
            | Self::ParseConfig { .. }
            | Self::InvalidConfig(_)
            | Self::InvalidInput(_) => 2,
            _ => 1,
        }
    }
}

pub type Result<T> = std::result::Result<T, AppError>;
