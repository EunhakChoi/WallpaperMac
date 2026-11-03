use super::{metal::SceneView, sound::SoundManager};
use objc2_foundation::{MainThreadMarker, NSRect};
use serde_json::{Value, json};
use std::{cell::RefCell, collections::BTreeMap};
use wpe_core::{PlayerInput, Project, ScenePlayer, ScenePlayerInitialState, ScenePlayerOptions};
pub(super) struct LiveScene {
    pub renderer: SceneView,
    pub player: RefCell<ScenePlayer>,
    pub sounds: RefCell<SoundManager>,
    pub videos: RefCell<super::video_texture::VideoTextures>,
    pub text: RefCell<super::text::NativeText>,
    storage_key: String,
    global_key: String,
    events: RefCell<Vec<wpe_script::HostCommand>>,
    last_input: RefCell<PlayerInput>,
}
impl LiveScene {
    pub fn new(
        project: Project,
        screen: &str,
        storage: &mut BTreeMap<String, Value>,
        rect: NSRect,
        mtm: MainThreadMarker,
        options: ScenePlayerOptions,
        initial_input: PlayerInput,
    ) -> Result<(Self, Vec<String>), String> {
        let storage_key = format!("{}::{screen}", project.root.display());
        let global_key = format!("{}::global", project.root.display());
        let initial = json!({"screen":storage.get(&storage_key).cloned().unwrap_or(json!({})),"global":storage.get(&global_key).cloned().unwrap_or(json!({}))});
        let is_editor = options.is_editor;
        let mut player = ScenePlayer::new_with_context(
            project,
            options,
            ScenePlayerInitialState {
                storage: initial,
                input: initial_input.clone(),
                user_properties: json!({}),
            },
        )
        .map_err(|e| e.to_string())?;
        player.set_native_text_provider(true);
        let mut text = super::text::NativeText::default();
        let mut frame = player
            .advance(0., &initial_input)
            .map_err(|e| e.to_string())?;
        text.update(&mut player, &mut frame)?;
        let issues = frame
            .issues
            .iter()
            .map(|i| format!("{}: {}", i.code, i.message))
            .collect();
        store(storage, &storage_key, &global_key, player.storage())?;
        let renderer = SceneView::from_frame(frame, rect, mtm)?;
        renderer.set_hdr(
            player
                .scene()
                .pointer("/general/hdr")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        );
        Ok((
            Self {
                renderer,
                player: RefCell::new(player),
                sounds: RefCell::new(SoundManager::for_editor(is_editor)),
                videos: RefCell::new(Default::default()),
                text: RefCell::new(text),
                storage_key,
                global_key,
                events: RefCell::new(vec![]),
                last_input: RefCell::new(initial_input),
            },
            issues,
        ))
    }
    pub fn advance(
        &self,
        dt: f32,
        input: &PlayerInput,
        storage: &mut BTreeMap<String, Value>,
    ) -> Result<Option<String>, String> {
        *self.last_input.borrow_mut() = input.clone();
        let mut player = self.player.borrow_mut();
        let mut cached = player.storage().clone();
        cached["global"] = storage.get(&self.global_key).cloned().unwrap_or(json!({}));
        player.set_storage(cached).map_err(|e| e.to_string())?;
        let mut frame = player.advance(dt, input).map_err(|e| e.to_string())?;
        let mut issue = frame
            .issues
            .first()
            .map(|i| format!("{}: {}", i.code, i.message));
        let commands = player.drain_host_commands();
        for command in commands {
            let result = match command.kind.as_str() {
                "sound.configure" => self.sounds.borrow_mut().configure(
                    player.project(),
                    &command.object_id,
                    &command.arguments,
                ),
                "layer.play" | "layer.pause" | "layer.stop" => self
                    .sounds
                    .borrow_mut()
                    .command(&command.kind, &command.object_id)
                    .map(|_| ()),
                kind if kind.starts_with("videoTexture.") => Ok(()),
                "engine.openUserShortcut" => {
                    self.events.borrow_mut().push(command);
                    Ok(())
                }
                _ => Err(format!("No native handler for {}", command.kind)),
            };
            if let Err(error) = result {
                issue.get_or_insert(error);
            }
        }
        self.text.borrow_mut().update(&mut player, &mut frame)?;
        for error in self.videos.borrow_mut().update(&mut player) {
            issue.get_or_insert(error);
        }
        let feedback = self.sounds.borrow_mut().sync(player.scene(), dt);
        for (id, properties) in feedback {
            player
                .apply_provider_state(&id, &properties)
                .map_err(|e| e.to_string())?;
        }
        if let Some(error) = self.sounds.borrow().failure() {
            issue.get_or_insert(error);
        }
        store(
            storage,
            &self.storage_key,
            &self.global_key,
            player.storage(),
        )?;
        self.renderer.set_hdr(
            player
                .scene()
                .pointer("/general/hdr")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        );
        self.renderer.submit(frame)?;
        Ok(issue.or_else(|| self.renderer.failure()))
    }
    pub fn render_current(&self, input: &PlayerInput) -> Result<(), String> {
        let mut player = self.player.borrow_mut();
        let mut input = input.clone();
        {
            let held = self.last_input.borrow();
            input.audio_left = held.audio_left.clone();
            input.audio_right = held.audio_right.clone();
        }
        let mut frame = player.render_current(&input).map_err(|e| e.to_string())?;
        self.renderer.set_hdr(
            player
                .scene()
                .pointer("/general/hdr")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        );
        self.text.borrow_mut().update(&mut player, &mut frame)?;
        self.renderer.submit(frame)
    }
    pub fn drain_events(&self) -> Vec<wpe_script::HostCommand> {
        std::mem::take(&mut *self.events.borrow_mut())
    }
}
fn store(
    storage: &mut BTreeMap<String, Value>,
    screen: &str,
    global: &str,
    value: &Value,
) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err("Scene storage exceeds4MiB per scene".into());
    }
    storage.insert(
        screen.into(),
        value.get("screen").cloned().unwrap_or(json!({})),
    );
    storage.insert(
        global.into(),
        value.get("global").cloned().unwrap_or(json!({})),
    );
    Ok(())
}
