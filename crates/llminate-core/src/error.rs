//! Error types and exit-code mapping shared by the library and the CLI.

use std::fmt;

/// Result alias used across the crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Every failure mode llminate can hit.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Configuration problem (unknown target, no target, bad TOML...).
    #[error("{0}")]
    Config(String),

    /// The resolved target has no API key but the command needs one.
    #[error("clé API manquante — définis LLMINATE_API_KEY, ou api_key_env dans [targets.<nom>]")]
    MissingApiKey,

    /// A non-2xx HTTP response.
    #[error("HTTP {status} sur {path}: {body}")]
    Http {
        status: u16,
        path: String,
        body: String,
    },

    /// Transport-level error (DNS, TCP, TLS, timeout...).
    #[error("réseau: {0}")]
    Network(#[from] reqwest::Error),

    /// Malformed JSON from the server.
    #[error("JSON invalide: {0}")]
    Json(#[from] serde_json::Error),

    /// Filesystem problem reading/writing config.
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),

    /// Free-form message for command-level failures.
    #[error("{0}")]
    Msg(String),
}

impl Error {
    /// Shell exit code: usage/config problems are `2`, everything else `1`.
    pub fn exit_code(&self) -> i32 {
        match self {
            Error::MissingApiKey | Error::Config(_) => 2,
            _ => 1,
        }
    }

    /// Convenience constructor for a free-form message error.
    pub fn msg(s: impl fmt::Display) -> Self {
        Error::Msg(s.to_string())
    }
}
