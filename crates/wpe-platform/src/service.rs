//! Portable management commands and deterministic playback scheduling.
use crate::{
    Error, Result,
    config::{AppConfig, ConfigStore, Preset, Selection},
    library::{LibraryInventory, scan},
    playback::{self, Calendar, Decision, PlaybackFacts, PlaybackService, PlaylistState},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostAction {
    Assign {
        screen: String,
        selection: Option<Selection>,
    },
    Playback {
        paused: u32,
        muted: bool,
        stopped: bool,
    },
    AudioChanged,
    OpenUri {
        uri: String,
    },
    SettingsChanged,
}
#[derive(Clone, Debug, Serialize)]
pub struct Reply {
    pub value: Value,
    pub actions: Vec<HostAction>,
}

pub struct PlatformService {
    pub config: AppConfig,
    pub inventory: LibraryInventory,
    pub effective: BTreeMap<String, Selection>,
    store: ConfigStore,
    playback: PlaybackService,
    last: Decision,
    playlists: BTreeMap<String, PlaylistState>,
    pub paused: bool,
    pub stopped: bool,
}
impl PlatformService {
    pub fn new(path: PathBuf) -> Result<Self> {
        let store = ConfigStore::new(path);
        let config = store.load()?;
        let inventory = scan(&config.library_roots);
        let effective = config.screens.clone();
        let playlists = config.playlist_states.clone();
        Ok(Self {
            config,
            inventory,
            effective,
            store,
            playback: PlaybackService::default(),
            last: Decision::default(),
            playlists,
            paused: false,
            stopped: false,
        })
    }
    pub fn config_path(&self) -> &Path {
        self.store.path()
    }
    pub fn snapshot(&self) -> Value {
        json!({"config":self.config,"library":self.inventory,"effective":self.effective,"paused":self.paused,"stopped":self.stopped,"decision":{"paused":self.last.paused,"muted":self.last.muted,"stopped":self.last.stopped},"config_path":self.store.path()})
    }
    pub fn refresh(&mut self) {
        self.inventory = scan(&self.config.library_roots);
    }
    pub fn save(&mut self) -> Result<()> {
        self.config.playlist_states = self.playlists.clone();
        self.store.save(&self.config)
    }
    fn commit(&mut self, mut config: AppConfig) -> Result<Vec<HostAction>> {
        config.playlist_states = self.playlists.clone();
        config.validate()?;
        self.store.save(&config)?;
        let mut actions = diff(&self.effective, &config.screens);
        actions.push(HostAction::SettingsChanged);
        self.effective = config.screens.clone();
        self.config = config;
        self.playback = PlaybackService::default();
        self.playlists = self.config.playlist_states.clone();
        Ok(actions)
    }
    pub fn execute(&mut self, method: &str, args: Value) -> Result<Reply> {
        let mut actions = Vec::new();
        let mut value = Value::Null;
        match method {
            "state" => value = self.snapshot(),
            "library.refresh" => {
                self.refresh();
                value = serde_json::to_value(&self.inventory)?;
            }
            "library.add" => {
                let root = PathBuf::from(required(&args, "path")?).canonicalize()?;
                if !root.is_dir() {
                    return Err(Error("Library root must be a directory".into()));
                }
                let mut c = self.config.clone();
                if !c.library_roots.contains(&root) {
                    c.library_roots.push(root);
                }
                actions = self.commit(c)?;
                self.refresh();
                value = serde_json::to_value(&self.inventory)?;
            }
            "library.remove" => {
                let root = PathBuf::from(required(&args, "path")?);
                let mut c = self.config.clone();
                c.library_roots.retain(|p| p != &root);
                actions = self.commit(c)?;
                self.refresh();
            }
            "assign" => {
                let screen = required(&args, "screen")?.to_owned();
                let mut c = self.config.clone();
                if args.get("selection").is_none_or(Value::is_null) {
                    c.screens.remove(&screen);
                } else {
                    let s: Selection = serde_json::from_value(args["selection"].clone())?;
                    if !s.project.exists() {
                        return Err(Error(format!(
                            "Project does not exist: {}",
                            s.project.display()
                        )));
                    }
                    c.screens.insert(screen.clone(), s);
                }
                actions = self.commit(c)?;
                if !actions
                    .iter()
                    .any(|action| matches!(action,HostAction::Assign{screen:id,..}if id==&screen))
                {
                    actions.insert(
                        0,
                        HostAction::Assign {
                            screen: screen.clone(),
                            selection: self.effective.get(&screen).cloned(),
                        },
                    );
                }
            }
            "properties.set" => {
                let screen = required(&args, "screen")?;
                let mut c = self.config.clone();
                let s = c
                    .screens
                    .get_mut(screen)
                    .ok_or_else(|| Error(format!("No project assigned to {screen}")))?;
                let properties: BTreeMap<String, Value> = serde_json::from_value(
                    args.get("properties")
                        .cloned()
                        .ok_or_else(|| Error("Missing properties".into()))?,
                )?;
                s.properties.extend(properties);
                s.preset = None;
                actions = self.commit(c)?;
            }
            "preset.save" => {
                let screen = required(&args, "screen")?;
                let name = nonempty(&args, "name")?.to_owned();
                let mut c = self.config.clone();
                let selection = c
                    .screens
                    .get(screen)
                    .ok_or_else(|| Error("No project assigned".into()))?;
                let preset = Preset {
                    name: name.clone(),
                    properties: c.selection_properties(selection),
                };
                let list = c
                    .presets
                    .entry(selection.project.to_string_lossy().into_owned())
                    .or_default();
                if let Some(p) = list.iter_mut().find(|p| p.name == name) {
                    *p = preset;
                } else {
                    list.push(preset);
                }
                actions = self.commit(c)?;
            }
            "preset.apply" => {
                let screen = required(&args, "screen")?;
                let name = nonempty(&args, "name")?.to_owned();
                let mut c = self.config.clone();
                let s = c
                    .screens
                    .get_mut(screen)
                    .ok_or_else(|| Error("No project assigned".into()))?;
                if !c
                    .presets
                    .get(&s.project.to_string_lossy().into_owned())
                    .is_some_and(|ps| ps.iter().any(|p| p.name == name))
                {
                    return Err(Error("Preset not found".into()));
                }
                s.preset = Some(name);
                actions = self.commit(c)?;
            }
            "profile.save" => {
                let mut c = self.config.clone();
                c.save_profile(nonempty(&args, "name")?.to_owned())?;
                actions = self.commit(c)?;
            }
            "profile.apply" => {
                let mut c = self.config.clone();
                c.apply_profile(required(&args, "name")?)?;
                actions = self.commit(c)?;
            }
            "config.set" => {
                let c: AppConfig =
                    serde_json::from_value(args.get("config").cloned().unwrap_or(args))?;
                actions = self.commit(c)?;
                actions.push(HostAction::AudioChanged);
                self.refresh();
            }
            "audio.set" => {
                let mut c = self.config.clone();
                c.audio = serde_json::from_value(args)?;
                actions = self.commit(c)?;
                actions.push(HostAction::AudioChanged);
            }
            "playback.pause" => {
                self.paused = args
                    .get("paused")
                    .and_then(Value::as_bool)
                    .unwrap_or(!self.paused)
            }
            "playback.stop" => {
                self.stopped = args
                    .get("stopped")
                    .and_then(Value::as_bool)
                    .unwrap_or(!self.stopped)
            }
            "playlist.next" => {
                let screen = required(&args, "screen")?;
                let selection = self
                    .effective
                    .get(screen)
                    .ok_or_else(|| Error("No project assigned".into()))?;
                let name = selection
                    .playlist
                    .as_ref()
                    .ok_or_else(|| Error("No playlist assigned".into()))?;
                let p = self
                    .config
                    .playlists
                    .iter()
                    .find(|p| &p.name == name)
                    .ok_or_else(|| Error("Playlist no longer exists".into()))?;
                if let Some(mut s) = self.playlists.entry(screen.into()).or_default().advance(
                    p,
                    0.0,
                    Calendar {
                        minute: 0,
                        weekday: 0,
                        day: 0,
                    },
                    true,
                ) {
                    s.playlist = Some(name.clone());
                    actions.push(HostAction::Assign {
                        screen: screen.into(),
                        selection: Some(s.clone()),
                    });
                    self.effective.insert(screen.into(), s);
                }
            }
            "workshop.open" => {
                let id = args
                    .get("id")
                    .and_then(|v| {
                        v.as_u64()
                            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                    })
                    .ok_or_else(|| Error("Workshop ID must be a positive integer".into()))?;
                actions.push(HostAction::OpenUri {
                    uri: crate::workshop::item_uri(id)?,
                });
            }
            "workshop.browse" => actions.push(HostAction::OpenUri {
                uri: crate::workshop::browse_uri(),
            }),
            _ => return Err(Error(format!("Unknown platform command: {method}"))),
        }
        Ok(Reply { value, actions })
    }
    pub fn tick(
        &mut self,
        dt: f64,
        calendar: Calendar,
        mut facts: PlaybackFacts,
    ) -> Result<Vec<HostAction>> {
        facts.manual_pause |= self.paused;
        facts.manual_stop |= self.stopped;
        let decision = playback::decide(&self.config.policy, &self.config.rules, &facts);
        let before = self.effective.clone();
        self.playback.apply_rules(
            &decision,
            &self.config.rules,
            &self.config.profiles,
            &mut self.effective,
        )?;
        if !decision.stopped {
            for (screen, selection) in &mut self.effective {
                let Some(name) = &selection.playlist else {
                    continue;
                };
                let Some(playlist) = self.config.playlists.iter().find(|p| &p.name == name) else {
                    continue;
                };
                if let Some(mut next) = self
                    .playlists
                    .entry(screen.clone())
                    .or_default()
                    .advance(playlist, dt, calendar, false)
                {
                    next.playlist = Some(name.clone());
                    *selection = next;
                }
            }
        }
        let mut actions = diff(&before, &self.effective);
        if self.last != decision {
            let action = HostAction::Playback {
                paused: decision.paused,
                muted: decision.muted,
                stopped: decision.stopped,
            };
            if decision.stopped || decision.paused != 0 || decision.muted {
                actions.insert(0, action);
            } else {
                actions.push(action);
            }
            self.last = decision;
        }
        Ok(actions)
    }
}
fn required<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| Error(format!("Missing text field: {key}")))
}
fn nonempty<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    let s = required(v, key)?;
    if s.trim().is_empty() {
        Err(Error(format!("Empty {key}")))
    } else {
        Ok(s)
    }
}
fn diff(old: &BTreeMap<String, Selection>, new: &BTreeMap<String, Selection>) -> Vec<HostAction> {
    let mut out = Vec::new();
    for screen in old.keys() {
        if !new.contains_key(screen) {
            out.push(HostAction::Assign {
                screen: screen.clone(),
                selection: None,
            });
        }
    }
    for (screen, selection) in new {
        if old.get(screen) != Some(selection) {
            out.push(HostAction::Assign {
                screen: screen.clone(),
                selection: Some(selection.clone()),
            });
        }
    }
    out
}
