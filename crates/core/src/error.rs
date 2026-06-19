//! Crate-wide error type.

use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialization error: {0}")]
    Serde(#[from] postcard::Error),

    #[error("TLS error: {0}")]
    Tls(#[from] rustls::Error),

    #[error("certificate generation error: {0}")]
    Rcgen(String),

    #[error("realtime channel crypto error: {0}")]
    Crypto(String),

    #[error("peer authentication failed: {0}")]
    Auth(String),

    #[error("protocol error: {0}")]
    Protocol(String),

    #[error("pairing error: {0}")]
    Pairing(String),

    #[error("frame too large: {0} bytes (limit {1})")]
    FrameTooLarge(usize, usize),

    #[error("configuration error: {0}")]
    Config(String),

    #[error("{0}")]
    Other(String),
}

impl From<rcgen::Error> for Error {
    fn from(e: rcgen::Error) -> Self {
        Error::Rcgen(e.to_string())
    }
}

impl From<anyhow::Error> for Error {
    fn from(e: anyhow::Error) -> Self {
        Error::Other(e.to_string())
    }
}
