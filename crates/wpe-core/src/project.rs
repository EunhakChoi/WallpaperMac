use crate::{
    CompatibilityIssue, Error, Result,
    vfs::{Resource, VirtualFileSystem},
};
use serde::Serialize;
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum WallpaperKind {
    Scene,
    Video,
    Web,
    Application,
    Unknown(String),
}

impl WallpaperKind {
    pub fn from_name(name: &str) -> Self {
        match name.to_ascii_lowercase().as_str() {
            "scene" => Self::Scene,
            "video" => Self::Video,
            "web" => Self::Web,
            "application" => Self::Application,
            _ => Self::Unknown(name.to_owned()),
        }
    }
}

#[derive(Debug)]
pub struct Project {
    pub root: PathBuf,
    /// Physical metadata-selected entry. For packages this is the .pkg file;
    /// scene resources inside it are resolved through read_resource.
    pub entry: PathBuf,
    pub title: String,
    pub kind: WallpaperKind,
    pub metadata: Value,
    pub issues: Vec<CompatibilityIssue>,
    pub vfs: VirtualFileSystem,
    pub(crate) scene_key: String,
    pub(crate) standalone_image: bool,
}

impl Project {
    pub fn read_resource(&self, name: &str) -> Result<Resource> {
        self.vfs.read(name)
    }
    /// Read the selected authored scene without starting wallpaper scripts.
    pub fn read_scene_document(&self) -> Result<Value> {
        if self.kind != WallpaperKind::Scene || self.standalone_image {
            return Err(Error::Unsupported(
                "authored scene document required; import a direct image into an editor project"
                    .into(),
            ));
        }
        let value = wpe_formats::jsonc::parse(&self.read_resource(&self.scene_key)?.bytes)?;
        if !value.is_object() {
            return Err(Error::invalid("scene", "object document required"));
        }
        Ok(value)
    }
    pub fn scene_resource_name(&self) -> Option<&str> {
        (self.kind == WallpaperKind::Scene && !self.standalone_image)
            .then_some(self.scene_key.as_str())
    }
    /// Select an editor working-copy scene, including an in-memory overlay.
    /// This changes the project object only; it never rewrites source metadata.
    pub fn override_scene_entry(&mut self, name: &str) -> Result<()> {
        let normalized = name.replace('\\', "/");
        let path = Path::new(&normalized);
        if self.kind != WallpaperKind::Scene
            || normalized.is_empty()
            || normalized.contains('\0')
            || path.is_absolute()
            || path.components().any(|c| {
                !matches!(
                    c,
                    std::path::Component::Normal(_) | std::path::Component::CurDir
                )
            })
        {
            return Err(Error::invalid(
                "scene entry",
                "expected a relative scene resource for a scene project",
            ));
        }
        self.entry = self.root.join(path);
        self.scene_key = normalized;
        self.standalone_image = false;
        Ok(())
    }
}

/// Open a folder/project.json, a loose scene JSON, or a scene package read-only.
/// PNG/JPEG, HTML and video direct paths are explicit convenience input profiles.
pub fn load_project(path: &Path, assets: Option<&Path>) -> Result<Project> {
    let selected = if path.is_dir() {
        path.join("project.json")
    } else {
        path.to_owned()
    };
    let selected = fs::canonicalize(&selected).map_err(|e| Error::io(&selected, e))?;
    let root = selected
        .parent()
        .ok_or_else(|| Error::invalid("project path", "has no parent"))?
        .to_owned();
    let extension = selected
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let is_metadata = selected
        .file_name()
        .is_some_and(|s| s.eq_ignore_ascii_case("project.json"));
    let mut issues = Vec::new();
    let (metadata, kind, entry_name, standalone_image) = if is_metadata {
        let bytes = fs::read(&selected).map_err(|e| Error::io(&selected, e))?;
        let metadata = wpe_formats::jsonc::parse(&bytes)?;
        if !metadata.is_object() {
            return Err(Error::invalid("project.json", "expected an object"));
        }
        let file = metadata
            .get("file")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::invalid("project.json", "missing string file"))?
            .replace('\\', "/");
        let kind = metadata
            .get("type")
            .and_then(Value::as_str)
            .map(WallpaperKind::from_name)
            .unwrap_or_else(|| {
                match Path::new(&file)
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase()
                    .as_str()
                {
                    "json" | "pkg" => WallpaperKind::Scene,
                    "html" | "htm" => WallpaperKind::Web,
                    "mp4" | "webm" | "mkv" | "mov" | "avi" | "m4v" => WallpaperKind::Video,
                    "exe" | "app" => WallpaperKind::Application,
                    _ => WallpaperKind::Unknown("unknown".into()),
                }
            });
        (metadata, kind, file, false)
    } else {
        let file = selected
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| Error::invalid("entry filename", "not UTF-8"))?
            .to_owned();
        let kind = match extension.as_str() {
            "json" | "pkg" | "png" | "jpg" | "jpeg" => WallpaperKind::Scene,
            "html" | "htm" => WallpaperKind::Web,
            "mp4" | "webm" | "mkv" | "mov" | "avi" | "m4v" => WallpaperKind::Video,
            _ => {
                return Err(Error::Unsupported(format!(
                    "direct entry extension {extension:?}; choose project.json"
                )));
            }
        };
        let image = matches!(extension.as_str(), "png" | "jpg" | "jpeg");
        issues.push(CompatibilityIssue::new(
            "direct-entry-profile",
            None,
            "Opened a direct file without original project metadata.",
        ));
        if image {
            issues.push(CompatibilityIssue::new("standalone-image-profile",None,"Direct PNG/JPEG is a convenience input; observed native scene image fields reference JSON descriptors."));
        }
        (serde_json::json!({"file":file}), kind, file, image)
    };
    let mut entry = root.join(&entry_name);
    let mut vfs = VirtualFileSystem::new();
    if let Some(assets) = assets {
        vfs.mount_directory_front(assets);
    }
    vfs.mount_directory_front(&root);
    let explicit_package = entry
        .extension()
        .is_some_and(|s| s.eq_ignore_ascii_case("pkg"));
    let implicit_package = is_metadata
        && kind == WallpaperKind::Scene
        && !explicit_package
        && Path::new(&entry_name)
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("json"))
        && root.join("scene.pkg").is_file();
    let implicit_package = implicit_package && !root.join(".wallpaper-mac-unpacked").is_file();
    let packed = explicit_package || implicit_package;
    if implicit_package {
        entry = root.join("scene.pkg");
    }
    if packed {
        vfs.mount_package(&entry)?;
    }
    let scene_key = if explicit_package {
        "scene.json".to_owned()
    } else {
        entry_name
    };
    let title = metadata
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            selected
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Wallpaper")
        })
        .to_owned();
    Ok(Project {
        root,
        entry,
        title,
        kind,
        metadata,
        issues,
        vfs,
        scene_key,
        standalone_image,
    })
}
