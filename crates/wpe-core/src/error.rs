use std::{fmt, io, path::PathBuf};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    Io { path: PathBuf, source: io::Error },
    Format(wpe_formats::Error),
    Invalid { context: String, message: String },
    ResourceNotFound(String),
    Unsupported(String),
}

impl Error {
    pub(crate) fn invalid(context: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Invalid {
            context: context.into(),
            message: message.into(),
        }
    }
    pub(crate) fn io(path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Format(source) => write!(f, "{source}"),
            Self::Invalid { context, message } => write!(f, "{context}: {message}"),
            Self::ResourceNotFound(path) => write!(f, "resource not found: {path}"),
            Self::Unsupported(message) => write!(f, "unsupported: {message}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Format(source) => Some(source),
            _ => None,
        }
    }
}

impl From<wpe_formats::Error> for Error {
    fn from(source: wpe_formats::Error) -> Self {
        Self::Format(source)
    }
}
