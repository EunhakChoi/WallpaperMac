use crate::{Error, Result, config::Selection};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    #[default]
    Run,
    Mute,
    Pause,
    PauseAll,
    Stop,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaybackPolicy {
    pub fullscreen: Action,
    pub maximized: Action,
    pub focused: Action,
    pub audio: Action,
    pub battery: Action,
    pub sleep: Action,
}
impl Default for PlaybackPolicy {
    fn default() -> Self {
        Self {
            fullscreen: Action::Pause,
            maximized: Action::Pause,
            focused: Action::Run,
            audio: Action::Run,
            battery: Action::Run,
            sleep: Action::PauseAll,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Condition {
    Running,
    Focused,
    Maximized,
    Fullscreen,
    PlayingAudio,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "lowercase")]
pub enum RuleAction {
    Run,
    Mute,
    Pause,
    PauseAll,
    Stop,
    LoadWallpaper(PathBuf),
    LoadPlaylist(String),
    LoadProfile(String),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppRule {
    pub executable: String,
    pub condition: Condition,
    pub action: RuleAction,
}
#[derive(Clone, Debug, Default)]
pub struct ApplicationFact {
    pub executable: String,
    pub focused: u32,
    pub maximized: u32,
    pub fullscreen: u32,
    pub playing_audio: bool,
}
#[derive(Clone, Debug, Default)]
pub struct PlaybackFacts {
    pub active_displays: u32,
    pub fullscreen: u32,
    pub maximized: u32,
    pub focused: u32,
    pub audio_active: bool,
    pub audio_unattributed: bool,
    pub on_battery: bool,
    pub sleeping: bool,
    pub applications: Vec<ApplicationFact>,
    pub manual_pause: bool,
    pub manual_stop: bool,
    pub vram_stop: bool,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Decision {
    pub paused: u32,
    pub muted: bool,
    pub stopped: bool,
    pub load_rule: Option<usize>,
}
fn normalize(s: &str) -> String {
    let s = s.rsplit(['/', '\\']).next().unwrap_or(s);
    let mut s = s.to_lowercase();
    if !s.contains('.') {
        s.push_str(".exe");
    }
    s
}
fn condition_mask(rule: &AppRule, fact: &ApplicationFact, all: u32) -> u32 {
    match rule.condition {
        Condition::Running => all,
        Condition::Focused => fact.focused,
        Condition::Maximized => fact.maximized,
        Condition::Fullscreen => fact.fullscreen,
        Condition::PlayingAudio => {
            if fact.playing_audio {
                all
            } else {
                0
            }
        }
    }
}
fn apply(action: &Action, mask: u32, all: u32, result: &mut Decision) {
    if mask == 0 {
        return;
    }
    match action {
        Action::Run => {}
        Action::Mute => result.muted = true,
        Action::Pause => result.paused |= mask,
        Action::PauseAll => result.paused = all,
        Action::Stop => result.stopped = true,
    }
}
pub fn decide(policy: &PlaybackPolicy, rules: &[AppRule], facts: &PlaybackFacts) -> Decision {
    let all = facts.active_displays;
    let mut r = Decision::default();
    let mut fs = facts.fullscreen;
    let mut max = facts.maximized;
    let mut focus = facts.focused;
    // An exemption belongs to one application's contribution. Other audible
    // applications continue to contribute to the global playback condition.
    let known_audio = facts.applications.iter().any(|app| app.playing_audio);
    let mut audio = facts.audio_unattributed || (facts.audio_active && !known_audio);
    for app in &facts.applications {
        if app.playing_audio
            && !rules.iter().any(|rule| {
                normalize(&rule.executable) == normalize(&app.executable)
                    && rule.condition == Condition::PlayingAudio
                    && matches!(rule.action, RuleAction::Run)
            })
        {
            audio = true;
        }
    }
    for app in &facts.applications {
        for rule in rules
            .iter()
            .filter(|rule| normalize(&rule.executable) == normalize(&app.executable))
        {
            match rule.condition {
                Condition::Focused => focus &= !app.focused,
                Condition::Maximized => max &= !app.maximized,
                Condition::Fullscreen => fs &= !app.fullscreen,
                _ => {}
            }
        }
    }
    apply(&policy.fullscreen, fs, all, &mut r);
    apply(&policy.maximized, max, all, &mut r);
    apply(&policy.focused, focus, all, &mut r);
    apply(&policy.audio, if audio { all } else { 0 }, all, &mut r);
    apply(
        &policy.battery,
        if facts.on_battery { all } else { 0 },
        all,
        &mut r,
    );
    apply(
        &policy.sleep,
        if facts.sleeping { all } else { 0 },
        all,
        &mut r,
    );
    for (i, rule) in rules.iter().enumerate() {
        for app in facts
            .applications
            .iter()
            .filter(|a| normalize(&a.executable) == normalize(&rule.executable))
        {
            let mask = condition_mask(rule, app, all);
            if mask == 0 {
                continue;
            }
            match &rule.action {
                RuleAction::Run => {}
                RuleAction::Mute => r.muted = true,
                RuleAction::Pause => r.paused |= mask,
                RuleAction::PauseAll => r.paused = all,
                RuleAction::Stop => r.stopped = true,
                _ => r.load_rule = Some(i),
            }
        }
    }
    if facts.manual_pause {
        r.paused = all;
    }
    if facts.manual_stop || facts.vram_stop {
        r.stopped = true;
    }
    if r.stopped {
        r.load_rule = None;
    }
    r
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PlaylistMode {
    #[default]
    Timer,
    Logon,
    Daytime,
    DayOfWeek,
    Never,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Order {
    Random,
    #[default]
    Sorted,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaylistSettings {
    pub mode: PlaylistMode,
    pub order: Order,
    pub interval_seconds: f64,
    pub no_repeat: bool,
}
impl Default for PlaylistSettings {
    fn default() -> Self {
        Self {
            mode: PlaylistMode::Timer,
            order: Order::Sorted,
            interval_seconds: 900.0,
            no_repeat: true,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlaylistItem {
    pub selection: Selection,
    #[serde(default)]
    pub daytime_end: Option<u16>,
    #[serde(default)]
    pub weekdays: Vec<u8>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Playlist {
    pub name: String,
    #[serde(default)]
    pub settings: PlaylistSettings,
    pub items: Vec<PlaylistItem>,
}
impl Playlist {
    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty()
            || !self.settings.interval_seconds.is_finite()
            || self.settings.interval_seconds <= 0.0
        {
            return Err(Error("Invalid playlist name or interval".into()));
        }
        for i in &self.items {
            if i.daytime_end.is_some_and(|m| m >= 1440) || i.weekdays.iter().any(|d| *d > 6) {
                return Err(Error("Invalid playlist calendar range".into()));
            }
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Calendar {
    pub minute: u16,
    pub weekday: u8,
    pub day: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlaylistState {
    pub index: Option<usize>,
    pub elapsed: f64,
    pub last_day: Option<u64>,
    pub random: u64,
    pub initialized: bool,
    pub history: BTreeSet<usize>,
    #[serde(default)]
    pub playlist_name: Option<String>,
}
impl Default for PlaylistState {
    fn default() -> Self {
        Self {
            index: None,
            elapsed: 0.0,
            last_day: None,
            random: 0x4d595df4d0f33173,
            initialized: false,
            history: BTreeSet::new(),
            playlist_name: None,
        }
    }
}
impl PlaylistState {
    pub fn advance(
        &mut self,
        p: &Playlist,
        dt: f64,
        calendar: Calendar,
        force: bool,
    ) -> Option<Selection> {
        if p.items.is_empty() {
            return None;
        }
        if self.playlist_name.as_deref() != Some(&p.name) {
            self.playlist_name = Some(p.name.clone());
            self.index = None;
            self.elapsed = 0.;
            self.initialized = false;
            self.history.clear();
        }
        if self.index.is_some_and(|i| i >= p.items.len()) {
            self.index = None;
            self.initialized = false;
        }
        self.history.retain(|i| *i < p.items.len());
        self.elapsed += if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        let first = !self.initialized;
        self.initialized = true;
        let candidate = match p.settings.mode {
            PlaylistMode::Daytime => Some(
                p.items
                    .iter()
                    .position(|i| i.daytime_end.is_some_and(|end| calendar.minute < end))
                    .unwrap_or(0),
            ),
            PlaylistMode::DayOfWeek => p
                .items
                .iter()
                .position(|i| i.weekdays.contains(&calendar.weekday)),
            _ if force
                || first
                || (p.settings.mode == PlaylistMode::Timer
                    && self.elapsed >= p.settings.interval_seconds) =>
            {
                if p.settings.order == Order::Random {
                    if self.history.len() >= p.items.len() {
                        self.history.clear();
                    }
                    self.random ^= self.random << 13;
                    self.random ^= self.random >> 7;
                    self.random ^= self.random << 17;
                    let mut choices = (0..p.items.len())
                        .filter(|i| !p.settings.no_repeat || !self.history.contains(i))
                        .collect::<Vec<_>>();
                    if choices.len() > 1 {
                        choices.retain(|i| Some(*i) != self.index);
                    }
                    Some(choices[self.random as usize % choices.len()])
                } else {
                    Some(self.index.map(|i| (i + 1) % p.items.len()).unwrap_or(0))
                }
            }
            _ => self.index,
        }?;
        self.last_day = Some(calendar.day);
        if self.index == Some(candidate) && !force && !first {
            return None;
        }
        self.index = Some(candidate);
        self.history.insert(candidate);
        self.elapsed = 0.0;
        Some(p.items[candidate].selection.clone())
    }
}

#[derive(Default)]
pub struct PlaybackService {
    pub playlists: BTreeMap<String, PlaylistState>,
    saved: Option<BTreeMap<String, Selection>>,
    active_rule: Option<usize>,
}
impl PlaybackService {
    pub fn apply_rules(
        &mut self,
        decision: &Decision,
        rules: &[AppRule],
        profiles: &[crate::config::Profile],
        selections: &mut BTreeMap<String, Selection>,
    ) -> Result<bool> {
        if self.active_rule == decision.load_rule {
            return Ok(false);
        }
        let Some(index) = decision.load_rule else {
            self.active_rule = None;
            if let Some(saved) = self.saved.take() {
                *selections = saved;
                return Ok(true);
            }
            return Ok(false);
        };
        let rule = rules
            .get(index)
            .ok_or_else(|| Error("Rule index no longer exists".into()))?;
        let mut candidate = selections.clone();
        match &rule.action {
            RuleAction::LoadWallpaper(project) => {
                for s in candidate.values_mut() {
                    *s = Selection {
                        project: project.clone(),
                        ..Selection::default()
                    };
                }
            }
            RuleAction::LoadPlaylist(name) => {
                for s in candidate.values_mut() {
                    s.playlist = Some(name.clone());
                }
            }
            RuleAction::LoadProfile(name) => {
                candidate = profiles
                    .iter()
                    .find(|p| &p.name == name)
                    .ok_or_else(|| Error(format!("Profile not found: {name}")))?
                    .screens
                    .clone()
            }
            _ => return Ok(false),
        }
        if self.saved.is_none() {
            self.saved = Some(selections.clone());
        }
        *selections = candidate;
        self.active_rule = Some(index);
        Ok(true)
    }
}

/// Capture/session activity holds recovered from the native pause detector.
/// A capture backend supplies real peaks and process identity; this does not
/// infer process activity from silence-filled FFT arrays.
#[derive(Default)]
pub struct AudioActivity {
    processes: BTreeMap<String, f64>,
    global_last: Option<f64>,
}
impl AudioActivity {
    pub fn observe_global(&mut self, now: f64, peak: f32) {
        if now.is_finite() && peak.is_finite() && peak > f32::from_bits(0x3a83126f) {
            self.global_last = Some(now);
        }
    }
    pub fn observe_process(&mut self, now: f64, executable: &str, peak: f32) {
        if now.is_finite() && peak.is_finite() && peak > f32::from_bits(0x3a83126f) {
            self.processes.insert(normalize(executable), now);
        }
    }
    pub fn global_active(&self, now: f64) -> bool {
        self.global_last.is_some_and(|t| now >= t && now - t < 5.0)
    }
    pub fn process_active(&self, now: f64, executable: &str) -> bool {
        self.processes
            .get(&normalize(executable))
            .is_some_and(|t| now >= *t && now - *t < 3.0)
    }
    pub fn prune(&mut self, now: f64) {
        self.processes.retain(|_, t| now >= *t && now - *t < 3.0);
    }
}

pub fn executable_matches(left: &str, right: &str) -> bool {
    normalize(left) == normalize(right)
}
