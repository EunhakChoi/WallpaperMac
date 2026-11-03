use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LibraryItem {
    pub id: String,
    pub root: PathBuf,
    pub title: String,
    pub kind: String,
    pub requires_external_runtime: bool,
    pub entry: PathBuf,
    pub preview: Option<PathBuf>,
    pub workshop_id: Option<u64>,
    pub tags: Vec<String>,
}
#[derive(Clone, Debug, Default, Serialize)]
pub struct LibraryInventory {
    pub items: Vec<LibraryItem>,
    pub errors: Vec<String>,
}
pub fn scan(roots: &[PathBuf]) -> LibraryInventory {
    let mut result = LibraryInventory::default();
    let mut seen = BTreeSet::new();
    let mut todo = roots.iter().map(|p| (p.clone(), 0)).collect::<Vec<_>>();
    let mut visited = 0usize;
    while let Some((path, depth)) = todo.pop() {
        visited += 1;
        if visited > 100000 {
            result
                .errors
                .push("Library scan limit100000 reached".into());
            break;
        }
        if depth > 10 || path.is_symlink() {
            continue;
        }
        if path.is_file() {
            if path.file_name().is_some_and(|n| n == "project.json") {
                match item(&path) {
                    Ok(item) => {
                        if seen.insert(item.id.clone()) {
                            result.items.push(item);
                        }
                    }
                    Err(e) => result.errors.push(format!("{}: {e}", path.display())),
                }
            }
            continue;
        }
        let project = path.join("project.json");
        if project.is_file() {
            todo.push((project, depth + 1));
            continue;
        }
        match fs::read_dir(&path) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|s| matches!(s, "node_modules" | ".git" | "target" | "assets"))
                    {
                        continue;
                    }
                    todo.push((p, depth + 1));
                }
            }
            Err(e) => result.errors.push(format!("{}: {e}", path.display())),
        }
    }
    result.items.sort_by(|a, b| {
        a.title
            .to_lowercase()
            .cmp(&b.title.to_lowercase())
            .then(a.id.cmp(&b.id))
    });
    result
}
fn item(path: &Path) -> Result<LibraryItem> {
    let bytes = fs::read(path)?;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err(Error("project.json exceeds8MiB".into()));
    }
    let v = wpe_formats::jsonc::parse(&bytes).map_err(|e| Error(e.to_string()))?;
    let root = path
        .parent()
        .ok_or_else(|| Error("Project has no parent".into()))?
        .canonicalize()?;
    let file = v["file"].as_str().unwrap_or("scene.json");
    let entry = root.join(file);
    let workshop_id = root
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.parse().ok());
    let kind = v["type"]
        .as_str()
        .map(str::to_lowercase)
        .unwrap_or_else(|| {
            match Path::new(file)
                .extension()
                .and_then(|s| s.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref()
            {
                Some("json" | "pkg") => "scene",
                Some("html" | "htm") => "web",
                Some("mp4" | "webm" | "mkv" | "avi" | "mov" | "wmv" | "m4v") => "video",
                Some("exe" | "app") => "application",
                _ => "unknown",
            }
            .to_owned()
        });
    Ok(LibraryItem {
        id: root.to_string_lossy().into_owned(),
        title: v["title"].as_str().unwrap_or("Untitled").to_owned(),
        requires_external_runtime: kind == "application",
        kind,
        entry,
        preview: v["preview"].as_str().map(|s| root.join(s)),
        root,
        workshop_id,
        tags: v["tags"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
    })
}
pub fn steam_workshop_roots(home: &Path) -> Vec<PathBuf> {
    vec![
        home.join("Library/Application Support/Steam/steamapps/workshop/content/431960"),
        home.join(".local/share/Steam/steamapps/workshop/content/431960"),
        home.join(".steam/steam/steamapps/workshop/content/431960"),
    ]
}

/// Copy a local editable project into this app's workspace. Existing source files
/// are read only; symlinks and recursive destination paths are rejected.
pub fn working_copy(source: &Path, workspace: &Path) -> Result<PathBuf> {
    let source = source.canonicalize()?;
    if !source.is_dir() {
        return Err(Error("Project source must be a directory".into()));
    }
    fs::create_dir_all(workspace)?;
    let workspace = workspace.canonicalize()?;
    if workspace.starts_with(&source) {
        return Err(Error(
            "Editor workspace cannot be inside the imported project".into(),
        ));
    }
    let mut hash = 0xcbf29ce484222325u64;
    for byte in source.to_string_lossy().bytes() {
        hash = (hash ^ byte as u64).wrapping_mul(0x100000001b3);
    }
    let destination = workspace.join(format!("project-{hash:016x}"));
    if destination.join(".wallpaper-mac-workspace").is_file() {
        return Ok(destination);
    }
    let temporary = workspace.join(format!(".import-{}-{hash:016x}", std::process::id()));
    fs::create_dir(&temporary)?;
    let result = (|| -> Result<()> {
        let mut todo = vec![(source.clone(), temporary.clone())];
        let mut files = 0usize;
        let mut bytes = 0u64;
        while let Some((from, to)) = todo.pop() {
            for entry in fs::read_dir(&from)? {
                let entry = entry?;
                let ty = entry.file_type()?;
                let name = entry.file_name();
                if matches!(name.to_str(), Some(".git" | "target" | "node_modules")) {
                    continue;
                }
                if ty.is_symlink() {
                    return Err(Error(format!(
                        "Project symlink cannot be imported: {}",
                        entry.path().display()
                    )));
                }
                let target = to.join(name);
                if ty.is_dir() {
                    fs::create_dir(&target)?;
                    todo.push((entry.path(), target));
                } else if ty.is_file() {
                    files += 1;
                    bytes = bytes
                        .checked_add(entry.metadata()?.len())
                        .ok_or_else(|| Error("Import size overflow".into()))?;
                    if files > 100000 || bytes > 2 * 1024 * 1024 * 1024 {
                        return Err(Error("Editor import exceeds100000 files or2GiB".into()));
                    }
                    fs::copy(entry.path(), target)?;
                }
            }
        }
        fs::write(
            temporary.join(".wallpaper-mac-workspace"),
            source.to_string_lossy().as_bytes(),
        )?;
        fs::rename(&temporary, &destination)?;
        Ok(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_dir_all(&temporary);
        return Err(error);
    }
    Ok(destination)
}

/// Export a new project directory. The source is read only and an existing
/// destination is never intentionally replaced. Staging stays beside output.
pub fn export_copy(source: &Path, destination: &Path) -> Result<PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    if destination.exists() {
        return Err(Error(
            "Choose a new project directory; destination already exists".into(),
        ));
    }
    let parent = destination
        .parent()
        .ok_or_else(|| Error("Export directory has no parent".into()))?;
    fs::create_dir_all(parent)?;
    let staging = parent.join(format!(
        ".wallpaper-export-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&staging)?;
    let result = (|| {
        let copy = working_copy(source, &staging)?;
        fs::rename(copy, destination)?;
        Ok(destination.to_owned())
    })();
    let _ = fs::remove_dir_all(staging);
    result
}
