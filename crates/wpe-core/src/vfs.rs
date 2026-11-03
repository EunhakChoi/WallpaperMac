//! Resource lookup: positive-size current package, original direct path, front mount.
use crate::{Error, Result};
use serde::Serialize;
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};
use wpe_formats::pkg::Package;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub enum ResourceOrigin {
    Package { archive: PathBuf, key: String },
    Direct(PathBuf),
    Directory { mount: PathBuf, path: PathBuf },
    Memory { mount: u64, key: String },
}

#[derive(Clone, Debug)]
pub struct Resource {
    pub bytes: Vec<u8>,
    pub origin: ResourceOrigin,
}

struct MountedPackage {
    path: PathBuf,
    package: Package,
    key_index: HashMap<String, usize>,
}

struct MemoryMount {
    id: u64,
    files: BTreeMap<String, Option<Arc<Vec<u8>>>>,
}

#[derive(Default)]
pub struct VirtualFileSystem {
    package: Option<MountedPackage>,
    directories: Vec<PathBuf>,
    memory: Vec<MemoryMount>,
    next_memory_id: u64,
}

impl std::fmt::Debug for VirtualFileSystem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VirtualFileSystem")
            .field("package", &self.package.as_ref().map(|p| &p.path))
            .field("directories", &self.directories)
            .field("memory_mounts", &self.memory.len())
            .finish()
    }
}

impl VirtualFileSystem {
    pub fn new() -> Self {
        Self::default()
    }

    /// A new mount replaces the current package, matching the observed native map.
    pub fn mount_package(&mut self, path: &Path) -> Result<()> {
        // Native mounting clears the old map before it reads the new table.
        self.package = None;
        let bytes = fs::read(path).map_err(|e| Error::io(path, e))?;
        let package = Package::parse(bytes)?;
        let mut key_index = HashMap::new();
        for (index, member) in package.members().iter().enumerate() {
            key_index.insert(member.name.replace('\\', "/").to_ascii_lowercase(), index);
        }
        self.package = Some(MountedPackage {
            path: path.to_owned(),
            package,
            key_index,
        });
        Ok(())
    }

    /// New directories are inserted at the front, rather than appended.
    pub fn mount_directory_front(&mut self, path: impl Into<PathBuf>) {
        self.directories.insert(0, path.into());
    }

    /// Editor working-copy overlays precede the original package and disk.
    /// A tombstone hides an older resource instead of falling through to disk.
    pub fn mount_memory_front(
        &mut self,
        files: BTreeMap<String, Option<Arc<Vec<u8>>>>,
    ) -> Result<()> {
        let mut normalized = BTreeMap::new();
        for (name, bytes) in files {
            if name.is_empty() || name.contains('\0') {
                return Err(Error::invalid(
                    "memory resource name",
                    "empty or contains NUL",
                ));
            }
            let key = memory_key(&name);
            if normalized.insert(key, bytes).is_some() {
                return Err(Error::invalid(
                    "memory resource name",
                    "duplicate normalized name",
                ));
            }
        }
        let id = self.next_memory_id;
        self.next_memory_id = self.next_memory_id.wrapping_add(1);
        self.memory.insert(
            0,
            MemoryMount {
                id,
                files: normalized,
            },
        );
        Ok(())
    }

    pub fn directories(&self) -> &[PathBuf] {
        &self.directories
    }
    /// Effective package table only, without traversing or reading asset mounts.
    /// Empty members are included so read() retains its native fallback rule.
    pub fn mounted_package_names(&self) -> Vec<String> {
        let Some(mount) = &self.package else {
            return Vec::new();
        };
        let mut names = mount
            .key_index
            .values()
            .map(|index| mount.package.members()[*index].name.replace('\\', "/"))
            .collect::<Vec<_>>();
        names.sort_by_key(|name| name.to_ascii_lowercase());
        names
    }

    /// Enumerate logical resource names without changing the lookup priority.
    /// Symlink directories are not traversed, and package names are retained even
    /// for empty members so that read() can apply the native fallback rule.
    pub fn resource_names(&self, extensions: &[&str]) -> Result<Vec<String>> {
        fn collect(
            root: &Path,
            dir: &Path,
            extensions: &[&str],
            out: &mut std::collections::BTreeSet<String>,
        ) -> Result<()> {
            if !dir.is_dir() {
                return Ok(());
            }
            for entry in fs::read_dir(dir).map_err(|e| Error::io(dir, e))? {
                let entry = entry.map_err(|e| Error::io(dir, e))?;
                let path = entry.path();
                let ty = entry.file_type().map_err(|e| Error::io(&path, e))?;
                if ty.is_dir() {
                    collect(root, &path, extensions, out)?;
                } else if ty.is_file()
                    && (extensions.is_empty()
                        || path
                            .extension()
                            .and_then(|s| s.to_str())
                            .is_some_and(|s| extensions.iter().any(|e| s.eq_ignore_ascii_case(e))))
                    && let Ok(relative) = path.strip_prefix(root)
                {
                    out.insert(relative.to_string_lossy().replace('\\', "/"));
                }
            }
            Ok(())
        }
        let mut names = std::collections::BTreeSet::new();
        if let Some(package) = &self.package {
            for member in package.package.members() {
                let path = Path::new(&member.name);
                if extensions.is_empty()
                    || path
                        .extension()
                        .and_then(|s| s.to_str())
                        .is_some_and(|s| extensions.iter().any(|e| s.eq_ignore_ascii_case(e)))
                {
                    names.insert(member.name.replace('\\', "/"));
                }
            }
        }
        for mount in &self.directories {
            collect(mount, mount, extensions, &mut names)?;
        }
        for mount in self.memory.iter().rev() {
            for (key, bytes) in &mount.files {
                names.retain(|name| memory_key(name) != *key);
                if bytes.is_some()
                    && (extensions.is_empty()
                        || Path::new(key)
                            .extension()
                            .and_then(|s| s.to_str())
                            .is_some_and(|s| extensions.iter().any(|e| s.eq_ignore_ascii_case(e))))
                {
                    names.insert(key.clone());
                }
            }
        }
        Ok(names.into_iter().collect())
    }

    pub fn read(&self, name: &str) -> Result<Resource> {
        if name.contains('\0') {
            return Err(Error::invalid("resource name", "contains NUL"));
        }
        let normalized = name.replace('\\', "/");
        let memory_key = memory_key(&normalized);
        for mount in &self.memory {
            if let Some(bytes) = mount.files.get(&memory_key) {
                return match bytes {
                    Some(bytes) => Ok(Resource {
                        bytes: bytes.as_ref().clone(),
                        origin: ResourceOrigin::Memory {
                            mount: mount.id,
                            key: memory_key,
                        },
                    }),
                    None => Err(Error::ResourceNotFound(name.to_owned())),
                };
            }
        }
        // ASCII case folding is deliberately bounded; Windows locale/Unicode path
        // equivalence is not claimed for an arbitrary macOS/Linux filesystem.
        let key = normalized.to_ascii_lowercase();
        if let Some(mount) = &self.package
            && let Some(bytes) = mount
                .key_index
                .get(&key)
                .and_then(|index| mount.package.member_data(*index))
                .filter(|b| !b.is_empty())
        {
            return Ok(Resource {
                bytes: bytes.to_vec(),
                origin: ResourceOrigin::Package {
                    archive: mount.path.clone(),
                    key,
                },
            });
        }
        let direct = Path::new(&normalized);
        if let Ok(bytes) = fs::read(direct) {
            return Ok(Resource {
                bytes,
                origin: ResourceOrigin::Direct(direct.to_owned()),
            });
        }
        for mount in &self.directories {
            let path = mount.join(direct);
            if let Ok(bytes) = fs::read(&path) {
                return Ok(Resource {
                    bytes,
                    origin: ResourceOrigin::Directory {
                        mount: mount.clone(),
                        path,
                    },
                });
            }
        }
        Err(Error::ResourceNotFound(name.to_owned()))
    }
}

fn memory_key(name: &str) -> String {
    name.replace('\\', "/")
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect::<Vec<_>>()
        .join("/")
        .to_ascii_lowercase()
}

#[cfg(test)]
mod memory_tests {
    use super::*;

    #[test]
    fn working_copy_overrides_empty_files_and_tombstones_are_authoritative() {
        let mut fs = VirtualFileSystem::new();
        fs.mount_memory_front(BTreeMap::from([
            ("Scripts\\Pulse.JS".into(), Some(Arc::new(b"old".to_vec()))),
            ("scene.json".into(), Some(Arc::new(b"{}".to_vec()))),
        ]))
        .unwrap();
        fs.mount_memory_front(BTreeMap::from([
            ("./scripts/pulse.js".into(), Some(Arc::new(Vec::new()))),
            ("scene.json".into(), None),
        ]))
        .unwrap();
        let resource = fs.read("scripts\\PULSE.js").unwrap();
        assert!(resource.bytes.is_empty());
        assert!(matches!(
            resource.origin,
            ResourceOrigin::Memory { mount: 1, .. }
        ));
        assert!(fs.read("scene.json").is_err());
        assert_eq!(
            fs.resource_names(&["js", "json"]).unwrap(),
            ["scripts/pulse.js"]
        );
        assert!(
            fs.mount_memory_front(BTreeMap::from([
                ("a.JS".into(), None),
                ("A.js".into(), None)
            ]))
            .is_err()
        );
        assert!(fs.read("scripts/pulse.js").unwrap().bytes.is_empty());
    }
}
