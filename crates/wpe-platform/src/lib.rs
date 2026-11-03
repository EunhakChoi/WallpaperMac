//! Portable application services. Native handles and credentialed service SDKs stay outside.
pub mod application;
pub mod audio;
pub mod config;
pub mod decode;
pub mod export;
pub mod library;
pub mod media;
pub mod playback;
pub mod service;
pub mod steam;
mod steam_apps;
pub mod workshop;

pub type Result<T> = std::result::Result<T, Error>;
#[derive(Debug)]
pub struct Error(pub String);
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self(e.to_string())
    }
}
impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Self(e.to_string())
    }
}
