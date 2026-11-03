use crate::{
    Error, Result,
    playback::{AppRule, PlaybackPolicy, Playlist},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioSettings {
    pub enabled: bool,
    pub input_device: Option<String>,
    pub volume: i32,
    pub threshold: i32,
    pub sample_rate: u32,
    pub exclude_self: bool,
}
impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            input_device: None,
            volume: 50,
            threshold: 0,
            sample_rate: 48000,
            exclude_self: true,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Selection {
    pub project: PathBuf,
    pub preset: Option<String>,
    pub properties: BTreeMap<String, Value>,
    pub playlist: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    pub screens: BTreeMap<String, Selection>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Preset {
    pub name: String,
    pub properties: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub version: u32,
    pub library_roots: Vec<PathBuf>,
    pub assets: Option<PathBuf>,
    pub screens: BTreeMap<String, Selection>,
    pub playlists: Vec<Playlist>,
    pub playlist_states: BTreeMap<String, crate::playback::PlaylistState>,
    pub profiles: Vec<Profile>,
    pub presets: BTreeMap<String, Vec<Preset>>,
    pub rules: Vec<AppRule>,
    pub policy: PlaybackPolicy,
    pub audio: AudioSettings,
    pub media: crate::media::MediaSettings,
    pub fps: u32,
    pub playback_volume: f32,
    pub desktop: bool,
    pub start_at_login: bool,
    pub steam: Option<crate::steam::SteamConfig>,
    pub applications: BTreeMap<String, crate::application::ApplicationRuntime>,
    pub external_tools: BTreeMap<String, PathBuf>,
    pub scene_storage: BTreeMap<String, Value>,
}
impl Default for AppConfig {
    fn default() -> Self {
        Self {
            version: 1,
            library_roots: vec![],
            assets: None,
            screens: BTreeMap::new(),
            playlists: vec![],
            playlist_states: BTreeMap::new(),
            profiles: vec![],
            presets: BTreeMap::new(),
            rules: vec![],
            policy: PlaybackPolicy::default(),
            audio: AudioSettings::default(),
            media: Default::default(),
            fps: 30,
            playback_volume: 1.,
            desktop: false,
            start_at_login: false,
            steam: None,
            applications: BTreeMap::new(),
            external_tools: BTreeMap::new(),
            scene_storage: BTreeMap::new(),
        }
    }
}
impl AppConfig {
    pub fn validate(&self) -> Result<()> {
        if self.version != 1 {
            return Err(Error(format!(
                "Unsupported configuration version {}",
                self.version
            )));
        }
        if self.fps == 0 || self.fps > 240 {
            return Err(Error("FPS must be between1 and240".into()));
        }
        if !self.playback_volume.is_finite() || !(0. ..=1.).contains(&self.playback_volume) {
            return Err(Error("Playback volume must be finite and0..1".into()));
        }
        if self.screens.len() > 32 {
            return Err(Error("At most32 display masks are supported".into()));
        }
        if !(8000..=384000).contains(&self.audio.sample_rate) {
            return Err(Error("Audio sample rate outside 8000..384000".into()));
        }
        if !(0..=200).contains(&self.audio.volume) || !(0..=1000).contains(&self.audio.threshold) {
            return Err(Error(
                "Audio volume/threshold outside supported range".into(),
            ));
        }
        if self
            .steam
            .as_ref()
            .is_some_and(|s| s.app_id == 0 || s.library.as_os_str().is_empty())
        {
            return Err(Error(
                "Steam SDK configuration requires a library path and nonzero AppID".into(),
            ));
        }
        self.media.validate()?;
        if self.external_tools.values().any(|path| !path.is_absolute()) {
            return Err(Error("External tool paths must be absolute".into()));
        }
        for runtime in self.applications.values() {
            runtime.validate()?;
        }
        for p in &self.playlists {
            p.validate()?;
        }
        Ok(())
    }
    pub fn apply_profile(&mut self, name: &str) -> Result<()> {
        self.screens = self
            .profiles
            .iter()
            .find(|p| p.name == name)
            .ok_or_else(|| Error(format!("Profile not found: {name}")))?
            .screens
            .clone();
        Ok(())
    }
    pub fn save_profile(&mut self, name: String) -> Result<()> {
        if name.trim().is_empty() {
            return Err(Error("Profile name is empty".into()));
        }
        let profile = Profile {
            name: name.clone(),
            screens: self.screens.clone(),
        };
        if let Some(existing) = self.profiles.iter_mut().find(|p| p.name == name) {
            *existing = profile;
        } else {
            self.profiles.push(profile);
        }
        Ok(())
    }
    pub fn selection_properties(&self, selection: &Selection) -> BTreeMap<String, Value> {
        let mut result = selection.properties.clone();
        if let Some(name) = &selection.preset
            && let Some(preset) = self
                .presets
                .get(&selection.project.to_string_lossy().into_owned())
                .and_then(|p| p.iter().find(|p| &p.name == name))
        {
            result.extend(preset.properties.clone());
        }
        result
    }
}

pub struct ConfigStore {
    path: PathBuf,
}
impl ConfigStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn load(&self) -> Result<AppConfig> {
        if !self.path.exists() {
            return Ok(AppConfig::default());
        }
        let data = fs::read(&self.path)?;
        if data.len() > 16 * 1024 * 1024 {
            return Err(Error("Configuration exceeds16MiB".into()));
        }
        let result: AppConfig = serde_json::from_slice(&data)?;
        result.validate()?;
        Ok(result)
    }
    pub fn save(&self, config: &AppConfig) -> Result<()> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        config.validate()?;
        let parent = self
            .path
            .parent()
            .ok_or_else(|| Error("Configuration path has no parent".into()))?;
        let bytes = serde_json::to_vec_pretty(config)?;
        if bytes.len() > 16 * 1024 * 1024 {
            return Err(Error("Configuration exceeds16MiB".into()));
        }
        fs::create_dir_all(parent)?;
        let temporary = parent.join(format!(
            ".state-{}-{}.tmp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let result = (|| {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            fs::rename(&temporary, &self.path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}
