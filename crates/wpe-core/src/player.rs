mod assets;
mod camera_path;
mod draw;
mod environment;
mod host;
mod interaction;
mod lighting;
mod puppet;
mod reflection;
use crate::{CompatibilityIssue, Error, Project, Result, WallpaperKind};
use assets::AssetStore;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use wpe_render::{DrawCall, FilterMode, GpuVertex, Material, RenderFrame, Texture, TextureBinding};
use wpe_sim::{
    math::{Mat4, Quat, Vec3},
    model::ModelAnimator,
    particle::ParticleSystem,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct PlayerInput {
    /// Display-relative normalized pointer, origin at its top-left.
    pub cursor: [f32; 2],
    pub pointer_active: bool,
    pub buttons: u32,
    pub keys: BTreeMap<String, bool>,
    pub screen_resolution: [f32; 2],
    pub audio_left: Vec<f32>,
    pub audio_right: Vec<f32>,
    pub time_of_day: f32,
    pub media: Value,
    pub general_settings: Value,
}
impl Default for PlayerInput {
    fn default() -> Self {
        Self {
            cursor: [0.5; 2],
            pointer_active: true,
            buttons: 0,
            keys: BTreeMap::new(),
            screen_resolution: [1920., 1080.],
            audio_left: vec![0.; 64],
            audio_right: vec![0.; 64],
            time_of_day: 0.,
            media: json!({}),
            general_settings: json!({}),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ScenePlayerOptions {
    pub is_editor: bool,
    pub is_wallpaper: bool,
    pub is_desktop: bool,
}
impl Default for ScenePlayerOptions {
    fn default() -> Self {
        Self {
            is_editor: false,
            is_wallpaper: true,
            is_desktop: true,
        }
    }
}
#[derive(Clone, Debug)]
pub struct ScenePlayerInitialState {
    pub storage: Value,
    pub user_properties: Value,
    pub input: PlayerInput,
}
impl Default for ScenePlayerInitialState {
    fn default() -> Self {
        Self {
            storage: json!({"global":{},"screen":{}}),
            user_properties: json!({}),
            input: PlayerInput::default(),
        }
    }
}
#[derive(Clone)]
struct HitRegion {
    id: Value,
    inverse: Mat4,
    size: [f32; 2],
    anchor: [f32; 2],
    screen_space: bool,
    triangles: Option<std::sync::Arc<Vec<[Vec3; 3]>>>,
}

/// Owns mutable state for one scene. All platform output is returned as typed
/// frames/commands; scripts never receive filesystem or native process access.
pub struct ScenePlayer {
    project: Project,
    scene: Value,
    user_properties: Value,
    storage: Value,
    time: f64,
    frame_delta: f32,
    preview_camera: [f32; 3],
    preview_view: Option<Value>,
    camera_paths: camera_path::CameraPaths,
    render_only: bool,
    paused: bool,
    options: ScenePlayerOptions,
    particles: BTreeMap<String, ParticleSystem>,
    children: BTreeMap<String, wpe_sim::children::ParticleChildren>,
    models: BTreeMap<String, ModelAnimator>,
    entity_sources: BTreeMap<String, String>,
    vm: Option<wpe_script::SceneRuntime>,
    issues: Vec<CompatibilityIssue>,
    assets: AssetStore,
    lighting: lighting::LightingState,
    host_commands: Vec<wpe_script::HostCommand>,
    native_text: bool,
    text_rasters: BTreeMap<String, (String, std::sync::Arc<crate::RgbaImage>)>,
    hits: Vec<HitRegion>,
    hover: Vec<Value>,
    pressed: Vec<Value>,
    last_input: PlayerInput,
    render_input: PlayerInput,
}

impl ScenePlayer {
    pub fn new(project: Project) -> Result<Self> {
        Self::new_with_options(project, ScenePlayerOptions::default())
    }
    pub fn new_with_options(project: Project, options: ScenePlayerOptions) -> Result<Self> {
        Self::new_with_context(project, options, ScenePlayerInitialState::default())
    }
    pub fn new_with_state(
        project: Project,
        options: ScenePlayerOptions,
        storage: Value,
    ) -> Result<Self> {
        Self::new_with_context(
            project,
            options,
            ScenePlayerInitialState {
                storage,
                ..Default::default()
            },
        )
    }
    pub fn new_with_context(
        project: Project,
        options: ScenePlayerOptions,
        initial: ScenePlayerInitialState,
    ) -> Result<Self> {
        if !initial.storage.is_object() || !initial.user_properties.is_object() {
            return Err(Error::invalid(
                "initial state",
                "storage and user_properties must be objects",
            ));
        }
        if project.kind != WallpaperKind::Scene {
            return Err(Error::Unsupported(
                "ScenePlayer requires a scene project".into(),
            ));
        }
        let scene = if project.standalone_image {
            json!({"objects":[],"general":{}})
        } else {
            wpe_formats::jsonc::parse(&project.read_resource(&project.scene_key)?.bytes)?
        };
        let mut user_properties = project
            .metadata
            .pointer("/general/properties")
            .and_then(Value::as_object)
            .map(|values| {
                Value::Object(
                    values
                        .iter()
                        .map(|(k, v)| (k.clone(), v.get("value").unwrap_or(v).clone()))
                        .collect(),
                )
            })
            .unwrap_or_else(|| json!({}));
        for (k, v) in initial.user_properties.as_object().unwrap() {
            user_properties[k] = v.get("value").unwrap_or(v).clone();
        }
        let mut this = Self {
            project,
            scene,
            user_properties,
            storage: initial.storage,
            time: 0.,
            frame_delta: 0.,
            preview_camera: [1., 0., 0.],
            preview_view: None,
            camera_paths: camera_path::CameraPaths::default(),
            render_only: false,
            paused: false,
            options,
            particles: BTreeMap::new(),
            children: BTreeMap::new(),
            models: BTreeMap::new(),
            entity_sources: BTreeMap::new(),
            vm: None,
            issues: Vec::new(),
            assets: AssetStore::default(),
            lighting: lighting::LightingState::default(),
            host_commands: Vec::new(),
            native_text: false,
            text_rasters: BTreeMap::new(),
            hits: Vec::new(),
            hover: Vec::new(),
            pressed: Vec::new(),
            render_input: initial.input.clone(),
            last_input: initial.input,
        };
        resolve_user_bindings(&mut this.scene, &this.user_properties);
        this.refresh_camera_paths()?;
        this.initialize_entities()?;
        this.initialize_scripts()?;
        Ok(this)
    }
    pub fn project(&self) -> &Project {
        &self.project
    }
    pub fn scene(&self) -> &Value {
        &self.scene
    }
    pub fn time(&self) -> f64 {
        self.time
    }
    pub fn issues(&self) -> &[CompatibilityIssue] {
        &self.issues
    }
    pub fn texture_metadata(&self) -> Value {
        json!(self.assets.textures.iter().map(|(name,a)|json!({"name":name,"logical":a.logical,"storage":a.storage,"flags":a.flags,"frames":a.frames.len(),"images":a.images.len()})).collect::<Vec<_>>())
    }
    pub fn storage(&self) -> &Value {
        &self.storage
    }
    pub fn set_storage(&mut self, value: Value) -> Result<()> {
        if !value.is_object() {
            return Err(Error::invalid("script storage", "object required"));
        }
        self.storage = value.clone();
        if let Some(vm) = &mut self.vm {
            vm.set_storage(value).map_err(script_error)?;
        }
        Ok(())
    }
    /// Platform consumes sound, shortcut and video requests once, after submit.
    pub fn drain_host_commands(&mut self) -> Vec<wpe_script::HostCommand> {
        std::mem::take(&mut self.host_commands)
    }
    /// Apply decoder/playback feedback without resetting scripts, clocks or resources.
    pub fn apply_provider_state(&mut self, object_id: &Value, properties: &Value) -> Result<()> {
        let properties = properties
            .as_object()
            .ok_or_else(|| Error::invalid("provider state", "object required"))?;
        let object = self
            .scene
            .get_mut("objects")
            .and_then(Value::as_array_mut)
            .and_then(|a| a.iter_mut().find(|o| o.get("id") == Some(object_id)))
            .ok_or_else(|| Error::invalid("provider state", "object id missing"))?;
        for (k, v) in properties {
            object[k] = v.clone();
        }
        if let Some(vm) = &mut self.vm {
            vm.replace_scene(self.scene.clone()).map_err(script_error)?;
        }
        Ok(())
    }
    pub fn drain_video_requests(&mut self) -> Vec<(String, Vec<u8>)> {
        std::mem::take(&mut self.assets.video_requests)
            .into_iter()
            .collect()
    }
    pub fn provide_texture_frame(&mut self, name: &str, image: crate::RgbaImage) -> Result<()> {
        self.issues.retain(|issue| {
            !(issue.message.contains(name)
                && issue.message.contains("awaits a decoded provider frame"))
        });
        let image = std::sync::Arc::new(image);
        let texture = wpe_render::Texture::rgba2d(name, image.clone());
        texture.validate()?;
        if let Some(asset) = self.assets.textures.get_mut(name) {
            asset.texture = texture;
            asset.images = vec![wpe_render::TextureData::Image2D(image)];
        } else {
            self.assets.textures.insert(
                name.into(),
                assets::TextureAsset {
                    id: resource_id(&format!("video:{name}")),
                    logical: [image.width as f32, image.height as f32],
                    storage: [image.width as f32, image.height as f32],
                    frames: Vec::new(),
                    frame_size: None,
                    flags: 0,
                    gpu_format: 0,
                    texture,
                    images: vec![wpe_render::TextureData::Image2D(image)],
                },
            );
        }
        Ok(())
    }
    pub fn set_native_text_provider(&mut self, enabled: bool) {
        self.native_text = enabled;
    }
    pub fn provide_text_raster(
        &mut self,
        object_id: &Value,
        image: crate::RgbaImage,
    ) -> Result<()> {
        self.provide_text_raster_shared(object_id, std::sync::Arc::new(image))
    }
    pub fn provide_text_raster_shared(
        &mut self,
        object_id: &Value,
        image: std::sync::Arc<crate::RgbaImage>,
    ) -> Result<()> {
        image.validate()?;
        let object = self
            .scene
            .get("objects")
            .and_then(Value::as_array)
            .and_then(|a| a.iter().find(|o| o.get("id") == Some(object_id)))
            .ok_or_else(|| Error::invalid("text raster", "object missing"))?;
        self.text_rasters
            .insert(object_id.to_string(), (draw::text_key(object), image));
        Ok(())
    }
    pub fn register_font(
        &mut self,
        family: &str,
        bytes: Vec<u8>,
        collection_index: u32,
    ) -> Result<()> {
        let font = wpe_render::text::TextLayouter::with_index(bytes, collection_index)?;
        self.assets.fonts.insert(family.into(), font);
        Ok(())
    }
    pub fn set_paused(&mut self, value: bool) {
        self.paused = value;
    }
    /// Transient editor viewport. Pan is measured in scene canvas units and is
    /// shared by rendering, script cursor projection and hit testing.
    pub fn set_preview_camera(&mut self, zoom: f32, pan: [f32; 2]) -> Result<()> {
        if !zoom.is_finite() || !(0.01..=100.).contains(&zoom) || !pan.iter().all(|v| v.is_finite())
        {
            return Err(Error::invalid(
                "preview camera",
                "finite pan and zoom0.01..100 required",
            ));
        }
        self.preview_camera = [zoom, pan[0], pan[1]];
        Ok(())
    }
    pub fn set_preview_view(&mut self, camera: Option<Value>) -> Result<()> {
        if camera.as_ref().is_some_and(|v| !v.is_object()) {
            return Err(Error::invalid("preview camera", "camera object required"));
        }
        self.preview_view = camera;
        Ok(())
    }
    pub fn set_user_properties(&mut self, values: &Value) -> Result<()> {
        let values = values
            .as_object()
            .ok_or_else(|| Error::invalid("properties", "expected object"))?;
        let output = self
            .user_properties
            .as_object_mut()
            .expect("property object");
        for (k, v) in values {
            output.insert(k.clone(), v.get("value").unwrap_or(v).clone());
        }
        resolve_user_bindings(&mut self.scene, &self.user_properties);
        if let Some(vm) = &mut self.vm {
            vm.replace_scene(self.scene.clone()).map_err(script_error)?;
        }
        Ok(())
    }
    pub fn replace_scene(&mut self, scene: Value) -> Result<()> {
        if !scene.get("objects").is_some_and(Value::is_array) {
            return Err(Error::invalid("scene", "objects array required"));
        }
        if let Some(vm) = &mut self.vm {
            let out = vm.destroy().map_err(script_error)?;
            self.host_commands.extend(out.commands);
        }
        self.scene = scene;
        resolve_user_bindings(&mut self.scene, &self.user_properties);
        self.assets = AssetStore::default();
        self.particles.clear();
        self.children.clear();
        self.models.clear();
        self.entity_sources.clear();
        self.hits.clear();
        self.text_rasters.clear();
        self.hover.clear();
        self.pressed.clear();
        self.refresh_camera_paths()?;
        self.initialize_entities()?;
        self.initialize_scripts()
    }
    pub fn dispatch(&mut self, hook: &str, payload: Value) -> Result<()> {
        if let Some(vm) = &mut self.vm {
            let output = vm.dispatch(hook, payload).map_err(script_error)?;
            self.apply_script_output(output)?;
        }
        Ok(())
    }
    pub fn advance(&mut self, delta: f32, input: &PlayerInput) -> Result<RenderFrame> {
        if !delta.is_finite() || delta < 0. {
            return Err(Error::invalid(
                "frame delta",
                "must be finite and nonnegative",
            ));
        }
        if self.paused {
            return self.render_current(input);
        }
        if !input
            .screen_resolution
            .iter()
            .all(|v| v.is_finite() && *v > 0.)
            || !input.cursor.iter().all(|v| v.is_finite())
        {
            return Err(Error::invalid(
                "input",
                "finite pointer and positive display dimensions required",
            ));
        }
        let delta = if self.paused { 0. } else { delta.min(0.25) };
        self.frame_delta = delta;
        self.time += delta as f64;
        if self.project.standalone_image {
            return Ok(frame_from_plan(
                crate::load_scene(&self.project)?,
                self.time as f32,
            ));
        }
        let frame_input = self.script_input(delta, input);
        self.pointer_events(&frame_input, input)?;
        if let Some(vm) = &mut self.vm {
            let output = vm.step(frame_input).map_err(script_error)?;
            self.apply_script_output(output)?;
        }
        self.refresh_camera_paths()?;
        self.initialize_entities()?;
        let frame = self.render_scene(delta, input, true)?;
        self.render_input = input.clone();
        self.last_input = input.clone();
        Ok(frame)
    }
    /// Render a dirty paused viewport/property/provider frame without running
    /// VM hooks, clocks, timers, particle operators or animation physics.
    pub fn render_current(&mut self, input: &PlayerInput) -> Result<RenderFrame> {
        if !input.cursor.iter().all(|v| v.is_finite())
            || !input
                .screen_resolution
                .iter()
                .all(|v| v.is_finite() && *v > 0.)
        {
            return Err(Error::invalid(
                "input",
                "finite pointer and positive display dimensions required",
            ));
        }
        let previous = std::mem::replace(&mut self.frame_delta, 0.);
        let previous_render_only = std::mem::replace(&mut self.render_only, true);
        let result = if self.project.standalone_image {
            crate::load_scene(&self.project).map(|plan| frame_from_plan(plan, self.time as f32))
        } else {
            self.render_scene(0., input, false)
        };
        self.frame_delta = previous;
        self.render_only = previous_render_only;
        if result.is_ok() {
            self.render_input = input.clone();
        }
        result
    }
    fn canvas(&self) -> [u32; 2] {
        let projection = self.scene.pointer("/general/orthogonalprojection");
        [
            number(projection.and_then(|v| v.get("width")), 1920.).clamp(1., 16384.) as u32,
            number(projection.and_then(|v| v.get("height")), 1080.).clamp(1., 16384.) as u32,
        ]
    }
    fn camera_parts(&self, width: u32, height: u32, input: &PlayerInput) -> (Mat4, Mat4, Vec3) {
        let v = self
            .preview_view
            .as_ref()
            .or_else(|| self.scene.get("camera"))
            .unwrap_or(&Value::Null);
        let mut center = vec3(
            v.get("center"),
            Vec3::new(width as f32 * 0.5, height as f32 * 0.5, 0.),
        );
        let mut eye = vec3(v.get("eye"), center + Vec3::new(0., 0., 1.));
        let mut up = vec3(v.get("up"), Vec3::new(0., 1., 0.));
        if self.preview_view.is_none()
            && let Some(sample) = self.camera_paths.sample(self.time)
        {
            eye = sample.eye;
            center = sample.center;
            up = sample.up;
        }
        let pan = Vec3::new(self.preview_camera[1], self.preview_camera[2], 0.);
        center += pan;
        eye += pan;
        if boolean(self.scene.pointer("/general/cameraparallax"), false) {
            let amount = number(self.scene.pointer("/general/cameraparallaxamount"), 0.5);
            let offset = Vec3::new(
                (input.cursor[0] - 0.5) * amount * width as f32,
                (0.5 - input.cursor[1]) * amount * height as f32,
                0.,
            );
            center += offset;
            eye += offset;
        }
        if boolean(self.scene.pointer("/general/camerashake"), false) {
            let amplitude = number(self.scene.pointer("/general/camerashakeamplitude"), 1.);
            let rate = number(self.scene.pointer("/general/camerashakespeed"), 1.);
            let t = self.time as f32 * rate;
            let offset = Vec3::new(
                wpe_sim::noise::noise1(t),
                wpe_sim::noise::noise1(t + 127.3),
                0.,
            ) * amplitude;
            center += offset;
            eye += offset;
        }
        let view = Mat4::look_at(eye, center, up);
        let zoom =
            number(self.scene.pointer("/general/zoom"), 1.).max(0.0001) * self.preview_camera[0];
        let projection = if self
            .scene
            .pointer("/general/orthogonalprojection")
            .is_some()
        {
            let mut p =
                Mat4::orthographic(width as f32 / zoom, height as f32 / zoom, -10000., 10000.);
            p.0[12] = 0.;
            p.0[13] = 0.;
            p
        } else {
            Mat4::perspective(
                number(
                    v.get("fov").or_else(|| self.scene.pointer("/general/fov")),
                    50.,
                )
                .to_radians()
                    / zoom,
                width as f32 / height as f32,
                number(
                    v.get("nearz")
                        .or_else(|| self.scene.pointer("/general/nearz")),
                    0.1,
                )
                .max(0.00001),
                number(
                    v.get("farz")
                        .or_else(|| self.scene.pointer("/general/farz")),
                    10000.,
                )
                .max(0.001),
            )
        };
        (projection, view, eye)
    }
    fn camera(&self, width: u32, height: u32, input: &PlayerInput) -> Mat4 {
        let (p, v, _) = self.camera_parts(width, height, input);
        p * v
    }
    fn script_input(&self, delta: f32, input: &PlayerInput) -> wpe_script::FrameInput {
        let [width, height] = self.canvas();
        let camera = self.camera(width, height, input);
        let cursor = cursor_world(camera, input.cursor).unwrap_or(Vec3::new(
            input.cursor[0] * width as f32,
            (1. - input.cursor[1]) * height as f32,
            0.,
        ));
        let bins = |data: &[f32]| {
            (0..64)
                .map(|i| data.get(i).copied().filter(|v| v.is_finite()).unwrap_or(0.))
                .collect()
        };
        wpe_script::FrameInput {
            delta_seconds: delta,
            runtime_seconds: self.time,
            time_of_day: input.time_of_day,
            screen_resolution: input.screen_resolution,
            canvas_size: [width as f32, height as f32],
            cursor_screen: [
                input.cursor[0] * input.screen_resolution[0],
                input.cursor[1] * input.screen_resolution[1],
            ],
            cursor_world: cursor.0,
            cursor_left_down: input.pointer_active && input.buttons & 1 != 0,
            audio_left: bins(&input.audio_left),
            audio_right: bins(&input.audio_right),
            user_properties: self.user_properties.clone(),
            general_settings: input.general_settings.clone(),
            media: input.media.clone(),
        }
    }
    fn issue(&mut self, code: &str, name: Option<&str>, message: impl Into<String>) {
        let message = message.into();
        if !self
            .issues
            .iter()
            .any(|i| i.code == code && i.object.as_deref() == name && i.message == message)
        {
            if self.issues.len() == 1024 {
                self.issues.remove(0);
            }
            self.issues
                .push(CompatibilityIssue::new(code, name, message));
        }
    }
}
fn script_error(error: wpe_script::Error) -> Error {
    Error::invalid("SceneScript", error.to_string())
}
fn literal(value: &Value) -> &Value {
    value.get("value").map_or(value, literal)
}
fn number(value: Option<&Value>, default: f32) -> f32 {
    value
        .map(literal)
        .and_then(|v| {
            v.as_f64()
                .map(|v| v as f32)
                .or_else(|| v.as_str()?.parse().ok())
        })
        .filter(|v| v.is_finite())
        .unwrap_or(default)
}
fn boolean(value: Option<&Value>, default: bool) -> bool {
    value
        .map(literal)
        .and_then(|v| v.as_bool().or_else(|| v.as_f64().map(|n| n != 0.)))
        .unwrap_or(default)
}
fn vec3(value: Option<&Value>, default: Vec3) -> Vec3 {
    let Some(v) = value.map(literal) else {
        return default;
    };
    if let Some(n) = v.as_f64() {
        return Vec3([n as f32; 3]);
    }
    let values = if let Some(s) = v.as_str() {
        s.split_whitespace()
            .filter_map(|n| n.parse().ok())
            .collect::<Vec<f32>>()
    } else if let Some(v) = v.as_array() {
        v.iter()
            .filter_map(|v| v.as_f64().map(|v| v as f32))
            .collect()
    } else {
        return default;
    };
    Vec3(std::array::from_fn(|i| {
        values
            .get(i)
            .copied()
            .filter(|n| n.is_finite())
            .unwrap_or(default.0[i])
    }))
}
fn object_key(object: &Value, at: usize) -> String {
    object
        .get("id")
        .map(Value::to_string)
        .unwrap_or_else(|| format!("object-{at}"))
}
fn hierarchy(objects: &[Value]) -> Result<Vec<Mat4>> {
    fn resolve(
        index: usize,
        objects: &[Value],
        ids: &BTreeMap<String, usize>,
        output: &mut [Mat4],
        state: &mut [u8],
    ) -> Result<()> {
        if state[index] == 2 {
            return Ok(());
        }
        if state[index] == 1 {
            return Err(Error::invalid("scene hierarchy", "parent cycle"));
        }
        state[index] = 1;
        let object = &objects[index];
        let local = Mat4::trs(
            vec3(object.get("origin"), Vec3::ZERO),
            Quat::euler_xyz(vec3(object.get("angles"), Vec3::ZERO)),
            vec3(object.get("scale"), Vec3::ONE),
        );
        let parent = object
            .get("parent")
            .filter(|p| !p.is_null())
            .and_then(|p| ids.get(&p.to_string()))
            .copied();
        let world = if let Some(parent) = parent {
            resolve(parent, objects, ids, output, state)?;
            let attachment = object
                .get("parentattachment")
                .or_else(|| object.get("attachment"))
                .and_then(|name| {
                    objects[parent]
                        .get("attachments")?
                        .as_array()?
                        .iter()
                        .find(|a| a.get("name") == Some(name))
                })
                .and_then(|a| matrix(a.get("transform")));
            output[parent] * attachment.unwrap_or_default() * local
        } else {
            local
        };
        if !world.finite() {
            return Err(Error::invalid("scene transform", "nonfinite matrix"));
        }
        output[index] = world;
        state[index] = 2;
        Ok(())
    }
    let ids = objects
        .iter()
        .enumerate()
        .map(|(i, o)| (object_key(o, i), i))
        .collect();
    let mut output = vec![Mat4::IDENTITY; objects.len()];
    let mut state = vec![0; objects.len()];
    for i in 0..objects.len() {
        resolve(i, objects, &ids, &mut output, &mut state)?;
    }
    Ok(output)
}
fn matrix(value: Option<&Value>) -> Option<Mat4> {
    let values = value?.as_array()?;
    if values.len() != 16 {
        return None;
    }
    let mut output = [0.; 16];
    for (i, v) in values.iter().enumerate() {
        output[i] = v.as_f64()? as f32;
    }
    output.iter().all(|n| n.is_finite()).then_some(Mat4(output))
}
fn resolve_user_bindings(value: &mut Value, properties: &Value) {
    match value {
        Value::Object(map) => {
            if let Some(binding) = map.get("user")
                && let Some(name) = binding
                    .as_str()
                    .or_else(|| binding.get("name").and_then(Value::as_str))
                && let Some(value) = properties.get(name)
            {
                let value = value.get("value").unwrap_or(value);
                let resolved =
                    if let Some(condition) = binding.get("condition").and_then(Value::as_str) {
                        Value::Bool(
                            value.as_str() == Some(condition)
                                || *value == condition
                                || value.as_f64().is_some_and(|number| {
                                    condition.parse::<f64>().ok() == Some(number)
                                }),
                        )
                    } else {
                        value.clone()
                    };
                map.insert("value".into(), resolved);
            }
            if let Some(names) = map.get("usertextures").and_then(Value::as_array).cloned() {
                let mut textures = map
                    .get("textures")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                for (i, name) in names.iter().enumerate() {
                    if let Some(value) = name
                        .as_str()
                        .and_then(|name| properties.get(name))
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                    {
                        while textures.len() <= i {
                            textures.push(Value::Null)
                        }
                        textures[i] = json!(value);
                    }
                }
                map.insert("textures".into(), json!(textures));
            }
            for (k, v) in map.iter_mut() {
                if k != "script" && k != "user" {
                    resolve_user_bindings(v, properties)
                }
            }
        }
        Value::Array(a) => {
            for v in a {
                resolve_user_bindings(v, properties)
            }
        }
        _ => {}
    }
}
fn cursor_world(camera: Mat4, position: [f32; 2]) -> Option<Vec3> {
    let inv = camera.inverse()?;
    let project = |z| {
        let p = inv.transform4([position[0] * 2. - 1., 1. - position[1] * 2., z, 1.]);
        (p[3].abs() > 1e-12).then(|| Vec3::new(p[0] / p[3], p[1] / p[3], p[2] / p[3]))
    };
    let a = project(-1.)?;
    let b = project(1.)?;
    let d = b - a;
    if d.0[2].abs() < 1e-12 {
        return Some(a);
    }
    Some(a + d * (-a.0[2] / d.0[2]))
}
fn resource_id(key: &str) -> u64 {
    key.bytes().fold(0xcbf29ce484222325_u64, |hash, b| {
        (hash ^ b as u64).wrapping_mul(0x100000001b3)
    })
}
fn draw_from_quad(quad: &crate::Quad, id: u64) -> DrawCall {
    let vertices = quad
        .vertices
        .iter()
        .map(|v| GpuVertex {
            position: [v.position[0], v.position[1], 0.5, 1.],
            uv: [v.uv[0], v.uv[1], 0., 0.],
            color: quad.color,
            ..GpuVertex::default()
        })
        .collect();
    let material = Material {
        textures: vec![TextureBinding {
            name: "g_Texture0".into(),
            slot: 0,
            texture: id,
        }],
        ..Material::default()
    };
    DrawCall {
        name: quad.name.clone(),
        vertices,
        indices: vec![0, 1, 2, 2, 1, 3],
        material,
        ..DrawCall::default()
    }
}
pub fn frame_from_plan(plan: crate::RenderPlan, time: f32) -> RenderFrame {
    let mut frame = RenderFrame::new(plan.width as u32, plan.height as u32);
    frame.clear = plan.clear_color;
    frame.time = time;
    for (at, quad) in plan.quads.iter().enumerate() {
        let id = at as u64 + 1;
        let mut texture = Texture::rgba2d(&quad.name, quad.image.clone());
        texture.sampler.min_filter = FilterMode::Nearest;
        texture.sampler.mag_filter = FilterMode::Nearest;
        frame.textures.insert(id, texture);
        frame.draws.push(draw_from_quad(quad, id));
    }
    frame.issues = plan
        .issues
        .into_iter()
        .map(|i| wpe_render::CapabilityIssue {
            code: i.code,
            resource: i.object,
            message: i.message,
        })
        .collect();
    frame
}
