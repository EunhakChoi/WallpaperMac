//! Bounded readers for the locally observed Wallpaper Engine resource formats.
//!
//! Parsing retains metadata that the first renderer does not execute. Decoding an
//! unsupported video, volume or GPU format returns an explicit error.

pub mod fbx;
pub mod jsonc;
pub mod mdl;
pub mod pkg;
mod reader;
pub mod tex;

use serde::Serialize;
use std::fmt;

/// Allocation policy for a single decoded resource (512 MiB).
pub const MAX_ALLOCATION: usize = 512 * 1024 * 1024;
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum ErrorKind {
    Parse,
    Unsupported,
    Limit,
    Decode,
}

#[derive(Clone, Debug, Serialize)]
pub struct Error {
    pub kind: ErrorKind,
    pub offset: Option<usize>,
    pub context: String,
}
impl Error {
    pub fn parse(offset: usize, context: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::Parse,
            offset: Some(offset),
            context: context.into(),
        }
    }
    pub fn unsupported(offset: usize, context: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::Unsupported,
            offset: Some(offset),
            context: context.into(),
        }
    }
    pub fn limit(offset: usize, context: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::Limit,
            offset: Some(offset),
            context: context.into(),
        }
    }
    pub fn decode(offset: usize, context: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::Decode,
            offset: Some(offset),
            context: context.into(),
        }
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.kind)?;
        if let Some(offset) = self.offset {
            write!(f, " at byte {offset:#x}")?;
        }
        write!(f, ": {}", self.context)
    }
}
impl std::error::Error for Error {}

/// Owned top-down, straight RGBA8 rows; no implicit color-space conversion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}
impl RgbaImage {
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Result<Self> {
        let expected = rgba_len(width, height, 0)?;
        if rgba.len() != expected {
            return Err(Error::parse(
                0,
                format!("RGBA buffer has {} bytes, expected {expected}", rgba.len()),
            ));
        }
        Ok(Self {
            width,
            height,
            rgba,
        })
    }
    pub fn validate(&self) -> Result<()> {
        let expected = rgba_len(self.width, self.height, 0)?;
        if self.rgba.len() != expected {
            return Err(Error::parse(
                0,
                "RGBA buffer size does not match dimensions",
            ));
        }
        Ok(())
    }
}

pub(crate) fn rgba_len(width: u32, height: u32, offset: usize) -> Result<usize> {
    if width == 0 || height == 0 {
        return Err(Error::parse(offset, "zero image dimensions"));
    }
    let n = (width as usize)
        .checked_mul(height as usize)
        .and_then(|v| v.checked_mul(4))
        .ok_or_else(|| Error::limit(offset, "image dimension overflow"))?;
    if n > MAX_ALLOCATION {
        return Err(Error::limit(
            offset,
            "decoded image exceeds 512 MiB allocation limit",
        ));
    }
    Ok(n)
}
