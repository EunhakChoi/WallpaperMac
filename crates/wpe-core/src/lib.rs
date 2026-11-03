//! Project loading, mutable SceneScript/simulation orchestration and explicit
//! render graphs. The initial static compositor remains available separately.

mod error;
mod player;
mod project;
mod render;
pub mod runtime;
mod scene;
pub mod vfs;

pub use error::{Error, Result};
pub use player::{PlayerInput, ScenePlayer, ScenePlayerInitialState, ScenePlayerOptions};
pub use project::{Project, WallpaperKind, load_project};
pub use render::render_software;
pub use scene::load_scene;
use serde::Serialize;
use std::sync::Arc;
pub use wpe_formats::RgbaImage;

/// A requested capability that the current compatibility profile cannot honor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CompatibilityIssue {
    pub code: String,
    pub object: Option<String>,
    pub message: String,
}

impl CompatibilityIssue {
    pub fn new(code: impl Into<String>, object: Option<&str>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            object: object.map(str::to_owned),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for CompatibilityIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(object) = &self.object {
            write!(f, "{} [{}]: {}", self.code, object, self.message)
        } else {
            write!(f, "{}: {}", self.code, self.message)
        }
    }
}

/// Canvas NDC position and top-down texture UV. Exactly 16 bytes, suitable for Metal.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vertex {
    pub position: [f32; 2],
    pub uv: [f32; 2],
}

/// An affine quad in triangle-strip order: bottom-left, bottom-right, top-left, top-right.
#[derive(Clone, Debug)]
pub struct Quad {
    pub name: String,
    pub vertices: [Vertex; 4],
    pub image: Arc<RgbaImage>,
    pub color: [f32; 4],
}

#[derive(Clone, Debug)]
pub struct RenderPlan {
    pub width: f32,
    pub height: f32,
    pub clear_color: [f32; 4],
    pub quads: Vec<Quad>,
    pub issues: Vec<CompatibilityIssue>,
}

#[cfg(test)]
mod tests {
    use super::Vertex;
    #[test]
    fn shared_metal_vertex_layout_is_sixteen_bytes() {
        assert_eq!(std::mem::size_of::<Vertex>(), 16);
        assert_eq!(std::mem::align_of::<Vertex>(), 4);
        assert_eq!(std::mem::offset_of!(Vertex, position), 0);
        assert_eq!(std::mem::offset_of!(Vertex, uv), 8);
    }
}
