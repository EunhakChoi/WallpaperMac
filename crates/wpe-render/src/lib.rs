//! Portable render frames, shader translation and a CPU reference implementation.
mod array_arguments;
pub mod blend;
pub mod cpu;
mod dialect;
pub mod effect;
pub mod frame;
mod metal_layout;
pub mod shader;
pub mod text;

pub use frame::*;
pub use wpe_formats::{Error, ErrorKind, Result, RgbaImage};

/// Owned fixtures for CPU/physical Metal comparison, containing no original artwork.
pub mod smoke;
