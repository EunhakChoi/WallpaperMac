//! Scene sound layers decoded through AVAudioPlayer, with portable PCM fallback.
use objc2::rc::Retained;
use objc2::{AnyThread, extern_class, extern_methods};
use objc2_foundation::{NSData, NSError, NSObject};
use serde_json::Value;
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet},
};
use wpe_core::Project;
#[link(name = "AVFAudio", kind = "framework")]
unsafe extern "C" {}
extern_class!(
    #[unsafe(super(NSObject))]
    #[derive(Debug, PartialEq, Eq, Hash)]
    struct AVAudioPlayer;
);
impl AVAudioPlayer {
    extern_methods!(
        #[unsafe(method(initWithData:error:_))]
        #[unsafe(method_family=init)]
        fn with_data(
            this: objc2::rc::Allocated<Self>,
            data: &NSData,
        ) -> Result<Retained<Self>, Retained<NSError>>;
        #[unsafe(method(prepareToPlay))]
        #[unsafe(method_family=none)]
        fn prepare(&self) -> bool;
        #[unsafe(method(play))]
        #[unsafe(method_family=none)]
        fn play(&self) -> bool;
        #[unsafe(method(pause))]
        #[unsafe(method_family=none)]
        fn pause(&self);
        #[unsafe(method(stop))]
        #[unsafe(method_family=none)]
        fn stop(&self);
        #[unsafe(method(isPlaying))]
        #[unsafe(method_family=none)]
        fn is_playing(&self) -> bool;
        #[unsafe(method(setVolume:))]
        #[unsafe(method_family=none)]
        fn set_volume(&self, volume: f32);
        #[unsafe(method(setNumberOfLoops:))]
        #[unsafe(method_family=none)]
        fn set_loops(&self, count: isize);
        #[unsafe(method(duration))]
        #[unsafe(method_family=none)]
        fn duration(&self) -> f64;
        #[unsafe(method(currentTime))]
        #[unsafe(method_family=none)]
        fn current_time(&self) -> f64;
        #[unsafe(method(setCurrentTime:))]
        #[unsafe(method_family=none)]
        fn set_time(&self, seconds: f64);
    );
}
struct Track {
    clips: Vec<Retained<AVAudioPlayer>>,
    names: Vec<String>,
    index: usize,
    intent: bool,
    volume: f32,
    mode: String,
    last_playing: bool,
    delay: Option<f32>,
    minimum: f32,
    maximum: f32,
    mute_in_editor: bool,
}
pub struct SoundManager {
    tracks: BTreeMap<String, Track>,
    paused: Cell<bool>,
    muted: Cell<bool>,
    volume: Cell<f32>,
    random: u64,
    is_editor: bool,
    failure: RefCell<Option<String>>,
}
impl Default for SoundManager {
    fn default() -> Self {
        Self {
            tracks: BTreeMap::new(),
            paused: Cell::new(true),
            muted: Cell::new(false),
            volume: Cell::new(1.),
            random: 0x51eed5eed,
            is_editor: false,
            failure: RefCell::new(None),
        }
    }
}
impl SoundManager {
    pub fn for_editor(is_editor: bool) -> Self {
        {
            let mut value = Self::default();
            value.is_editor = is_editor;
            value
        }
    }
    pub fn failure(&self) -> Option<String> {
        self.failure.borrow_mut().take()
    }
    pub fn configure(
        &mut self,
        project: &Project,
        id: &Value,
        object: &Value,
    ) -> Result<(), String> {
        let names = object["sound"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(str::to_owned))
                    .collect::<Vec<_>>()
            })
            .or_else(|| object["sound"].as_str().map(|s| vec![s.into()]))
            .ok_or("Sound layer requires sound resource paths")?;
        if names.is_empty() {
            return Err("Sound layer has no clips".into());
        }
        if names.len() > 128 {
            return Err("Sound layer exceeds128 clips".into());
        }
        let key = id.to_string();
        if self.tracks.get(&key).is_some_and(|t| t.names == names) {
            return Ok(());
        }
        let mode = object["playbackmode"].as_str().unwrap_or("loop").to_owned();
        if !matches!(mode.as_str(), "loop" | "single" | "random") {
            return Err(format!("Unknown sound playback mode: {mode}"));
        }
        let minimum = number(object.get("mintime"), 1.);
        let maximum = number(object.get("maxtime"), 5.);
        if minimum < 0. || maximum < minimum || maximum > 86400. {
            return Err(
                "Sound random interval requires 0 <= mintime <= maxtime <= 86400 seconds".into(),
            );
        }
        let mut clips = vec![];
        for name in &names {
            let resource = project.read_resource(name).map_err(|e| e.to_string())?;
            if resource.bytes.len() > 128 * 1024 * 1024 {
                return Err("Sound source exceeds128MiB".into());
            }
            let native = AVAudioPlayer::with_data(
                AVAudioPlayer::alloc(),
                &NSData::with_bytes(&resource.bytes),
            );
            let player = match native {
                Ok(player) => player,
                Err(original) => {
                    let wav =
                        wpe_platform::decode::to_wav(&resource.bytes, Some(name)).map_err(|e| {
                            format!(
                                "{name}: native decoder {} / portable decoder {e}",
                                original.localizedDescription()
                            )
                        })?;
                    AVAudioPlayer::with_data(AVAudioPlayer::alloc(), &NSData::with_bytes(&wav))
                        .map_err(|e| {
                            format!("{name}: PCM playback decoder {}", e.localizedDescription())
                        })?
                }
            };
            player.set_loops(if mode == "loop" && names.len() == 1 {
                -1
            } else {
                0
            });
            if !player.prepare() {
                return Err(format!("{name}: audio device could not prepare playback"));
            }
            clips.push(player);
        }
        let mut track = Track {
            clips,
            names,
            index: 0,
            intent: object["__playing"]
                .as_bool()
                .unwrap_or(!object["startsilent"].as_bool().unwrap_or(false)),
            volume: number(object.get("volume"), 1.).clamp(0., 1.),
            mode,
            last_playing: false,
            delay: None,
            minimum,
            maximum,
            mute_in_editor: object["muteineditor"].as_bool().unwrap_or(false),
        };
        if track.mode == "random" && track.intent {
            track.delay = Some(interval(&mut self.random, track.minimum, track.maximum));
        }
        self.apply(&mut track)?;
        self.tracks.insert(key, track);
        Ok(())
    }
    fn apply(&self, track: &mut Track) -> Result<(), String> {
        for (i, clip) in track.clips.iter().enumerate() {
            clip.set_volume(
                if self.muted.get() || self.is_editor && track.mute_in_editor {
                    0.
                } else {
                    track.volume * self.volume.get()
                },
            );
            if i == track.index && track.intent && track.delay.is_none() && !self.paused.get() {
                if !clip.is_playing() {
                    if !clip.play() {
                        return Err(format!(
                            "{}: audio playback could not start",
                            track.names[i]
                        ));
                    }
                    track.last_playing = true;
                }
            } else {
                clip.pause();
            }
        }
        Ok(())
    }
    pub fn set_paused(&self, paused: bool) {
        if self.paused.replace(paused) == paused {
            return;
        }
        for track in self.tracks.values() {
            if paused {
                track.clips[track.index].pause();
            } else if track.intent
                && track.delay.is_none()
                && !track.clips[track.index].is_playing()
                && !track.clips[track.index].play()
            {
                *self.failure.borrow_mut() = Some(format!(
                    "{}: audio playback could not resume",
                    track.names[track.index]
                ));
            }
        }
    }
    pub fn set_volume(&self, volume: f32) {
        self.volume.set(volume.clamp(0., 1.));
        self.set_muted(self.muted.get());
    }
    pub fn set_muted(&self, muted: bool) {
        self.muted.set(muted);
        for track in self.tracks.values() {
            for clip in &track.clips {
                clip.set_volume(if muted || self.is_editor && track.mute_in_editor {
                    0.
                } else {
                    track.volume * self.volume.get()
                });
            }
        }
    }
    pub fn command(&mut self, kind: &str, id: &Value) -> Result<bool, String> {
        let Some(track) = self.tracks.get_mut(&id.to_string()) else {
            return Ok(false);
        };
        match kind {
            "layer.play" => {
                if !track.intent && track.mode == "random" {
                    self.random ^= self.random << 13;
                    self.random ^= self.random >> 7;
                    self.random ^= self.random << 17;
                    track.index = self.random as usize % track.clips.len();
                }
                track.intent = true;
                track.delay = None;
                if !self.paused.get() {
                    if !track.clips[track.index].play() {
                        return Err("Sound playback could not start".into());
                    }
                    track.last_playing = true;
                }
            }
            "layer.pause" => {
                track.intent = false;
                track.clips[track.index].pause();
            }
            "layer.stop" => {
                track.intent = false;
                track.delay = None;
                for clip in &track.clips {
                    clip.stop();
                    clip.set_time(0.);
                }
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
    pub fn media(&self) -> Option<Value> {
        let track = self.tracks.values().find(|t| t.intent)?;
        let clip = &track.clips[track.index];
        Some(
            serde_json::json!({"status":{"enabled":true},"playback":{"state":if clip.is_playing(){1}else{2}},"properties":{"title":track.names[track.index],"artist":"","contentType":"music"},"timeline":{"position":clip.current_time(),"duration":clip.duration()},"thumbnail":{"hasThumbnail":false,"thumbnail":""}}),
        )
    }
    pub fn sync(&mut self, scene: &Value, dt: f32) -> Vec<(Value, Value)> {
        let mut feedback = vec![];
        let objects = scene["objects"].as_array().cloned().unwrap_or_default();
        let active = objects
            .iter()
            .map(|o| o["id"].to_string())
            .collect::<BTreeSet<_>>();
        self.tracks.retain(|id, _| active.contains(id));
        for object in objects {
            let id = object.get("id").cloned().unwrap_or(Value::Null);
            let Some(track) = self.tracks.get_mut(&id.to_string()) else {
                continue;
            };
            track.volume = number(object.get("volume"), track.volume).clamp(0., 1.);
            track.mute_in_editor = object["muteineditor"]
                .as_bool()
                .unwrap_or(track.mute_in_editor);
            if !self.paused.get()
                && track.intent
                && let Some(delay) = track.delay.as_mut()
            {
                *delay -= dt.clamp(0., 0.25);
                if *delay <= 0. {
                    track.delay = None;
                    self.random ^= self.random << 13;
                    self.random ^= self.random >> 7;
                    self.random ^= self.random << 17;
                    track.index = self.random as usize % track.clips.len();
                    track.clips[track.index].set_time(0.);
                    if !track.clips[track.index].play() {
                        *self.failure.borrow_mut() =
                            Some("Scheduled sound playback could not start".into());
                        track.intent = false;
                    }
                }
            }
            let playing = track.clips[track.index].is_playing();
            if track.last_playing && !playing && track.intent && !self.paused.get() {
                if track.mode == "loop" {
                    track.index = (track.index + 1) % track.clips.len();
                    track.clips[track.index].set_time(0.);
                    if !track.clips[track.index].play() {
                        *self.failure.borrow_mut() =
                            Some("Looped sound playback could not start".into());
                        track.intent = false;
                    }
                } else if track.mode == "random" {
                    track.delay = Some(interval(&mut self.random, track.minimum, track.maximum));
                } else {
                    track.intent = false;
                }
            }
            for clip in &track.clips {
                clip.set_volume(
                    if self.muted.get() || self.is_editor && track.mute_in_editor {
                        0.
                    } else {
                        track.volume * self.volume.get()
                    },
                );
            }
            track.last_playing = track.clips[track.index].is_playing();
            feedback.push((id,serde_json::json!({"__playing":track.intent,"__time":track.clips[track.index].current_time()})));
        }
        feedback
    }
}
impl Drop for SoundManager {
    fn drop(&mut self) {
        for track in self.tracks.values() {
            for clip in &track.clips {
                clip.stop();
            }
        }
    }
}
fn interval(state: &mut u64, minimum: f32, maximum: f32) -> f32 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    minimum + (maximum - minimum) * ((*state >> 40) as f32 / 16777216.)
}
fn number(value: Option<&Value>, default: f32) -> f32 {
    value
        .and_then(|v| v.get("value").unwrap_or(v).as_f64())
        .map(|v| v as f32)
        .filter(|v| v.is_finite())
        .unwrap_or(default)
}
