//! A real portable ECMAScript module host. It exchanges JSON scene state and host
//! commands; it neither loads native SceneScript DLLs nor depends on wpe-core.
mod clock;
mod curve;
mod error;
mod runtime;
mod types;
pub use clock::advance_animation_clocks;
pub use curve::sample as sample_property_curve;
pub use error::{Error, Result};
pub use runtime::SceneRuntime;
pub use types::*;

mod storage;
pub use storage::{decode_storage_value, encode_storage_value};
