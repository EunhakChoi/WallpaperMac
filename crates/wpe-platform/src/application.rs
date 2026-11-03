//! Explicit external application runtime configuration. Never invokes a shell.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ApplicationRuntime {
    pub command: PathBuf,
    pub arguments: Vec<String>,
    pub window_title: Option<String>,
    pub window_id: Option<u32>,
}
impl Default for ApplicationRuntime {
    fn default() -> Self {
        Self {
            command: PathBuf::new(),
            arguments: vec!["{entry}".into()],
            window_title: None,
            window_id: None,
        }
    }
}
impl ApplicationRuntime {
    pub fn validate(&self) -> Result<()> {
        if !self.command.is_absolute() {
            return Err(Error(
                "Application runtime requires an absolute executable path".into(),
            ));
        }
        if self.arguments.len() > 128
            || self
                .arguments
                .iter()
                .any(|s| s.len() > 32768 || s.contains('\0'))
        {
            return Err(Error(
                "External runtime arguments exceed the128argument/32KiB limit or contain NUL"
                    .into(),
            ));
        }
        if self
            .window_title
            .as_ref()
            .is_some_and(|s| s.len() > 4096 || s.contains('\0'))
        {
            return Err(Error("Invalid capture window title".into()));
        }
        if self.window_id == Some(0) {
            return Err(Error("Capture window ID must be positive".into()));
        }
        Ok(())
    }
    pub fn arguments_for(&self, entry: &Path, root: &Path) -> Result<Vec<String>> {
        self.validate()?;
        let entry = entry
            .to_str()
            .ok_or_else(|| Error("Application entry is not UTF-8".into()))?;
        let root = root
            .to_str()
            .ok_or_else(|| Error("Application project path is not UTF-8".into()))?;
        Ok(self
            .arguments
            .iter()
            .map(|s| s.replace("{entry}", entry).replace("{root}", root))
            .collect())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn runtime_arguments_are_literal_and_configuration_is_explicit() {
        let config = ApplicationRuntime {
            command: "/usr/local/bin/wine".into(),
            arguments: vec![
                "--bottle".into(),
                "literal $(do-not-execute)".into(),
                "{entry}".into(),
            ],
            ..Default::default()
        };
        let args = config
            .arguments_for(
                Path::new("/owned/project/my wallpaper.exe"),
                Path::new("/owned/project"),
            )
            .unwrap();
        assert_eq!(args[1], "literal $(do-not-execute)");
        assert_eq!(args[2], "/owned/project/my wallpaper.exe");
        assert!(ApplicationRuntime::default().validate().is_err());
        let mut invalid = config;
        invalid.arguments.push("a\0b".into());
        assert!(invalid.validate().is_err());
    }
}
