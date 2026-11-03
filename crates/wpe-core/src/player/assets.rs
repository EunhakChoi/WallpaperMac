use super::{literal, resource_id};
use crate::{Error, Project, Result};
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc};
use wpe_render::shader::{
    CompiledShader, ShaderCompiler, ShaderStage, SourceLibrary, UniformValue,
};
use wpe_render::{
    BlendState, CompareFunction, CpuProgram, CullMode, Material, RenderFrame, Texture,
    TextureBinding, TextureData,
};

#[derive(Clone)]
pub(super) struct TextureAsset {
    pub id: u64,
    pub logical: [f32; 2],
    pub storage: [f32; 2],
    pub frames: Vec<wpe_formats::tex::SpriteFrame>,
    pub frame_size: Option<[u32; 2]>,
    pub flags: u32,
    pub gpu_format: u32,
    pub texture: Texture,
    pub images: Vec<TextureData>,
}
impl TextureAsset {
    pub fn frame_index(&self, time: f32) -> usize {
        if self.frames.is_empty() {
            return 0;
        }
        let total = self
            .frames
            .iter()
            .map(|f| f.frame_time.max(0.00001))
            .sum::<f32>();
        let mut time = time.rem_euclid(total);
        for (i, f) in self.frames.iter().enumerate() {
            let duration = f.frame_time.max(0.00001);
            if time < duration {
                return i;
            }
            time -= duration;
        }
        self.frames.len() - 1
    }
    pub fn uv(&self, index: usize, uv: [f32; 2]) -> [f32; 2] {
        if let Some(frame) = self.frames.get(index) {
            let v = frame.values;
            [
                (v[0] + v[2] * uv[0] + v[4] * uv[1]) / self.storage[0],
                (v[1] + v[3] * uv[0] + v[5] * uv[1]) / self.storage[1],
            ]
        } else {
            [
                uv[0] * self.logical[0] / self.storage[0],
                uv[1] * self.logical[1] / self.storage[1],
            ]
        }
    }
    pub fn logical_frame(&self, index: usize) -> [f32; 2] {
        if let Some(size) = self.frame_size {
            [size[0] as f32, size[1] as f32]
        } else if let Some(frame) = self.frames.get(index) {
            let v = frame.values;
            [
                (v[2] * v[2] + v[3] * v[3]).sqrt(),
                (v[4] * v[4] + v[5] * v[5]).sqrt(),
            ]
        } else {
            self.logical
        }
    }
}
#[derive(Default)]
pub(super) struct AssetStore {
    pub video_requests: BTreeMap<String, Vec<u8>>,
    pub video_requested: std::collections::BTreeSet<String>,
    pub fonts: BTreeMap<String, wpe_render::text::TextLayouter>,
    pub textures: BTreeMap<String, TextureAsset>,
    json: BTreeMap<String, Value>,
    compiler: Option<ShaderCompiler>,
    shaders: BTreeMap<String, (Arc<CompiledShader>, Arc<CompiledShader>)>,
    pub targets: BTreeMap<String, u64>,
    pub global_defines: BTreeMap<String, i32>,
    scope: Option<String>,
    metadata: BTreeMap<String, Value>,
    pub fbo_state: BTreeMap<String, u64>,
    pub initialized_targets: BTreeMap<u64, (u32, u32, wpe_render::TextureFormat)>,
    pub pending_clears: BTreeMap<String, [f32; 4]>,
}
#[derive(Clone, Copy)]
enum MaterialProfile {
    Ordinary,
    Puppet,
    Model,
}
impl AssetStore {
    pub fn json(&mut self, project: &Project, name: &str) -> Result<Value> {
        let key = format!("{}:{name}", self.scope.as_deref().unwrap_or(""));
        if let Some(value) = self.json.get(&key) {
            return Ok(value.clone());
        }
        let resource = self.read(project, name)?;
        let value = wpe_formats::jsonc::parse(&resource.bytes)?;
        self.json.insert(key, value.clone());
        Ok(value)
    }
    pub fn json_scoped(&mut self, project: &Project, name: &str, scope: &str) -> Result<Value> {
        let previous = self.scope.replace(scope.into());
        let result = self.json(project, name);
        self.scope = previous;
        result
    }
    pub fn pass_scoped(
        &mut self,
        project: &Project,
        name: &str,
        pass: &Value,
        scope: &str,
        frame: &mut RenderFrame,
        time: f32,
    ) -> Result<Material> {
        let previous = self.scope.replace(scope.into());
        let result = self.pass(project, name, pass, frame, time);
        self.scope = previous;
        result
    }
    fn read(&self, project: &Project, name: &str) -> Result<crate::vfs::Resource> {
        match project.read_resource(name) {
            Ok(resource) => Ok(resource),
            Err(original) => {
                if let Some(scope) = &self.scope {
                    project
                        .read_resource(&format!("{scope}/{name}"))
                        .map_err(|_| original)
                } else {
                    Err(original)
                }
            }
        }
    }
    pub fn hydrate_material(
        &mut self,
        project: &Project,
        pass: &mut Value,
        scope: &str,
    ) -> Result<()> {
        let shader = pass
            .get("shader")
            .and_then(Value::as_str)
            .unwrap_or("genericimage2");
        let defines = pass
            .get("combos")
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .map(|(k, v)| (k.clone(), super::number(Some(v), 0.) as i32))
                    .collect::<BTreeMap<_, _>>()
            })
            .unwrap_or_default();
        let key = format!("{scope}:{shader}:{defines:?}");
        let metadata = if let Some(value) = self.metadata.get(&key) {
            value.clone()
        } else {
            let compiler = self.compiler(project)?;
            let mut defaults = serde_json::Map::new();
            let mut aliases = BTreeMap::<String, Vec<String>>::new();
            for (stage, extension) in [
                (ShaderStage::Vertex, "vert"),
                (ShaderStage::Fragment, "frag"),
            ] {
                if let Some(path) = shader_path(&compiler.library, shader, extension, Some(scope)) {
                    let reflected = compiler.reflect_metadata(&path, stage, &defines)?;
                    for (k, v) in reflected.defaults {
                        defaults.insert(k, uniform_json(v));
                    }
                    for (k, v) in reflected.uniform_aliases {
                        aliases.entry(k).or_default().extend(v);
                    }
                }
            }
            let value = serde_json::json!({"defaults":defaults,"aliases":aliases});
            self.metadata.insert(key, value.clone());
            value
        };
        pass["__shaderDefaults"] = metadata["defaults"].clone();
        pass["__uniformAliases"] = metadata["aliases"].clone();
        Ok(())
    }
    pub fn texture_asset(&mut self, project: &Project, name: &str) -> Result<TextureAsset> {
        let cache_name = if let Some(scope) = &self.scope {
            format!("{scope}:{name}")
        } else {
            name.into()
        };
        if let Some(asset) = self.textures.get(&cache_name) {
            return Ok(asset.clone());
        }
        let key = if name.starts_with("materials/") && name.ends_with(".tex") {
            name.into()
        } else {
            format!("materials/{name}.tex")
        };
        // Native logical texture names may themselves end in .jpg/.png/.gif.
        // Prefer the compiled TEX resource before the direct-image convenience.
        let native = self.read(project, &key).ok();
        if native.is_none() && name.to_ascii_lowercase().ends_with(".gif") {
            use image::AnimationDecoder;
            let resource = self.read(project, name)?;
            let decoder =
                image::codecs::gif::GifDecoder::new(std::io::Cursor::new(&resource.bytes))
                    .map_err(|e| Error::invalid(name, e.to_string()))?;
            let mut images = Vec::new();
            let mut frames = Vec::new();
            let mut dimensions = [0; 2];
            let mut bytes = 0usize;
            for (i, frame) in decoder.into_frames().take(4097).enumerate() {
                if i >= 4096 {
                    return Err(Error::invalid(name, "GIF exceeds4096 frames"));
                }
                let frame = frame.map_err(|e| Error::invalid(name, e.to_string()))?;
                let (n, d) = frame.delay().numer_denom_ms();
                let delay = (n as f32 / d.max(1) as f32 / 1000.).max(0.01);
                let image = frame.into_buffer();
                dimensions = [image.width(), image.height()];
                bytes += image.as_raw().len();
                if bytes > wpe_formats::MAX_ALLOCATION {
                    return Err(Error::invalid(
                        name,
                        "GIF decoded frames exceed allocation policy",
                    ));
                }
                let image = Arc::new(crate::RgbaImage::new(
                    image.width(),
                    image.height(),
                    image.into_raw(),
                )?);
                images.push(TextureData::Image2D(image));
                frames.push(wpe_formats::tex::SpriteFrame {
                    offset: 0,
                    image_id: i as u32,
                    frame_time: delay,
                    raw_words: [0; 8],
                    values: [0., 0., dimensions[0] as f32, 0., 0., dimensions[1] as f32],
                });
            }
            let first = images
                .first()
                .cloned()
                .ok_or_else(|| Error::invalid(name, "GIF has no frames"))?;
            let texture = Texture {
                name: name.into(),
                data: first,
                mips: Vec::new(),
                sampler: Default::default(),
                format: wpe_render::TextureFormat::Rgba8Unorm,
            };
            let asset = TextureAsset {
                id: resource_id(&cache_name),
                logical: [dimensions[0] as f32, dimensions[1] as f32],
                storage: [dimensions[0] as f32, dimensions[1] as f32],
                frame_size: Some(dimensions),
                frames,
                flags: 0,
                gpu_format: 0,
                images,
                texture,
            };
            self.textures.insert(cache_name.clone(), asset.clone());
            return Ok(asset);
        }
        if native.is_none()
            && [
                ".png", ".jpg", ".jpeg", ".webp", ".bmp", ".tif", ".tiff", ".tga", ".ico",
            ]
            .iter()
            .any(|extension| name.to_ascii_lowercase().ends_with(extension))
        {
            let resource = self.read(project, name)?;
            let image = image::load_from_memory(&resource.bytes)
                .map_err(|e| Error::invalid(name, e.to_string()))?
                .to_rgba8();
            let (width, height) = image.dimensions();
            let image = Arc::new(crate::RgbaImage::new(width, height, image.into_raw())?);
            let texture = Texture::rgba2d(name, image.clone());
            let asset = TextureAsset {
                id: resource_id(&cache_name),
                logical: [width as f32, height as f32],
                storage: [width as f32, height as f32],
                frames: Vec::new(),
                frame_size: None,
                flags: 0,
                gpu_format: 0,
                images: vec![TextureData::Image2D(image)],
                texture,
            };
            self.textures.insert(cache_name.clone(), asset.clone());
            return Ok(asset);
        }
        if self.video_requested.contains(&cache_name) {
            return Err(Error::Unsupported(format!(
                "embedded video {name} awaits a decoded provider frame"
            )));
        }
        let source = wpe_formats::tex::Texture::parse(
            &native
                .ok_or_else(|| Error::ResourceNotFound(key.clone()))?
                .bytes,
        )?;
        if source.video != 0 {
            if self.video_requested.insert(cache_name.clone()) {
                let mip = source
                    .images
                    .first()
                    .and_then(|i| i.mips.first())
                    .ok_or_else(|| Error::invalid(&key, "embedded video payload missing"))?;
                self.video_requests
                    .insert(cache_name.clone(), mip.payload()?);
            }
            return Err(Error::Unsupported(format!(
                "embedded video {key} awaits a decoded provider frame"
            )));
        }
        let mut images = Vec::new();
        let mut mips = Vec::new();
        for (image_index, image) in source.images.iter().enumerate() {
            for (mip_index, mip) in image.mips.iter().enumerate() {
                let data = if mip.depth > 1 {
                    if source.header.format != 0 {
                        return Err(Error::Unsupported(format!(
                            "volume GPU format {}",
                            source.header.format
                        )));
                    }
                    TextureData::Volume3D {
                        width: mip.width,
                        height: mip.height,
                        depth: mip.depth,
                        rgba: Arc::new(mip.payload()?),
                    }
                } else {
                    match source.decode_image(image_index, mip_index)? {
                        wpe_formats::tex::DecodedImage::Rgba8(image) => {
                            TextureData::Image2D(Arc::new(image))
                        }
                        wpe_formats::tex::DecodedImage::Rgba32Float(image) => {
                            TextureData::Float2D {
                                width: image.width,
                                height: image.height,
                                rgba: Arc::new(image.rgba),
                            }
                        }
                    }
                };
                if mip_index == 0 {
                    images.push(data)
                } else if image_index == 0 {
                    mips.push(data)
                }
            }
        }
        let first = images
            .first()
            .cloned()
            .ok_or_else(|| Error::invalid(&key, "texture has no images"))?;
        let mut texture = match &first {
            TextureData::Image2D(image) => Texture::rgba2d(&key, image.clone()),
            _ => Texture {
                name: key.clone(),
                data: first.clone(),
                mips: Vec::new(),
                sampler: Default::default(),
                format: wpe_render::TextureFormat::Rgba8Unorm,
            },
        };
        texture.mips = mips;
        texture.format = match source.header.format {
            10 => wpe_render::TextureFormat::Rg16Float,
            11 => wpe_render::TextureFormat::R16Float,
            _ => texture.format,
        };
        let asset = TextureAsset {
            id: resource_id(&format!("{cache_name}:{key}")),
            logical: [
                source.header.image_width as f32,
                source.header.image_height as f32,
            ],
            storage: [
                source.header.texture_width as f32,
                source.header.texture_height as f32,
            ],
            frame_size: source.sprites.as_ref().and_then(|s| s.dimensions),
            frames: source.sprites.map(|s| s.frames).unwrap_or_default(),
            flags: source.header.flags,
            gpu_format: source.header.format,
            texture,
            images,
        };
        self.textures.insert(cache_name.clone(), asset.clone());
        Ok(asset)
    }
    pub fn bind_texture(
        &mut self,
        project: &Project,
        name: &str,
        frame: &mut RenderFrame,
        time: f32,
    ) -> Result<u64> {
        if name.starts_with("_rt_") || self.targets.contains_key(&name.to_ascii_lowercase()) {
            let id = self
                .targets
                .get(&name.to_ascii_lowercase())
                .copied()
                .ok_or_else(|| Error::invalid(name, "render target alias has not been produced"))?;
            if name.eq_ignore_ascii_case("_rt_Reflection") && !frame.textures.contains_key(&id) {
                let format = frame
                    .textures
                    .values()
                    .find(|t| matches!(t.data, TextureData::RenderTarget { .. }))
                    .map(|t| t.format)
                    .unwrap_or(wpe_render::TextureFormat::Rgba8Unorm);
                frame.textures.insert(
                    id,
                    Texture::target_with_format(
                        name,
                        (frame.width / 2).max(1),
                        (frame.height / 2).max(1),
                        format,
                    ),
                );
            }
            if name.eq_ignore_ascii_case("_rt_MipMappedFrameBuffer") {
                let texture = frame
                    .textures
                    .get_mut(&id)
                    .ok_or_else(|| Error::invalid(name, "target declaration missing"))?;
                if texture.mips.is_empty() {
                    let [mut width, mut height, _] = texture.data.dimensions();
                    while width > 1 || height > 1 {
                        width = (width / 2).max(1);
                        height = (height / 2).max(1);
                        texture
                            .mips
                            .push(TextureData::RenderTarget { width, height });
                    }
                }
            }
            return Ok(id);
        }
        let asset = self.texture_asset(project, name)?;
        let mut texture = asset.texture.clone();
        let index = asset.frame_index(time);
        let image = asset
            .frames
            .get(index)
            .map(|f| f.image_id as usize)
            .unwrap_or_else(|| {
                if asset.images.len() > 1 {
                    (time * 30.).floor() as usize % asset.images.len()
                } else {
                    0
                }
            });
        texture.data = asset
            .images
            .get(image)
            .cloned()
            .ok_or_else(|| Error::invalid(name, "sprite image index outside texture"))?;
        frame.textures.insert(asset.id, texture);
        Ok(asset.id)
    }
    fn compiler(&mut self, project: &Project) -> Result<&ShaderCompiler> {
        if self.compiler.is_none() {
            let mut library = SourceLibrary::new();
            for name in project.vfs.resource_names(&["vert", "frag", "glsl", "h"])? {
                let data = project.read_resource(&name)?;
                if data.bytes.len() > 8 * 1024 * 1024 {
                    continue;
                }
                if let Ok(source) = String::from_utf8(data.bytes) {
                    library.insert(&name, &source);
                    if let Some(alias) = name.strip_prefix("shaders/") {
                        library.insert(alias, &source);
                    }
                }
            }
            library.insert(
                "requirements/LightingV1.glsl",
                super::lighting::LIGHTING_REQUIREMENT_GLSL,
            );
            self.compiler = Some(ShaderCompiler::new(library));
        }
        Ok(self.compiler.as_ref().unwrap())
    }
    #[allow(clippy::too_many_arguments)]
    fn material_texture(
        &mut self,
        project: &Project,
        name: &str,
        slot: u32,
        settings: &Value,
        material: &mut Material,
        frame: &mut RenderFrame,
        time: f32,
    ) -> Result<()> {
        let id = self.bind_texture(project, name, frame, time)?;
        if settings.is_object() {
            let mut texture = frame.textures[&id].clone();
            let address = |value: Option<&Value>| match value.and_then(Value::as_str) {
                Some("repeat" | "wrap") => Some(wpe_render::AddressMode::Repeat),
                Some("mirror") => Some(wpe_render::AddressMode::Mirror),
                Some("border") => Some(wpe_render::AddressMode::Border),
                Some("clamp") => Some(wpe_render::AddressMode::Clamp),
                _ => None,
            };
            let filter = |value: Option<&Value>| match value.and_then(Value::as_str) {
                Some("nearest" | "point") => Some(wpe_render::FilterMode::Nearest),
                Some("linear" | "anisotropic") => Some(wpe_render::FilterMode::Linear),
                _ => None,
            };
            texture.sampler.address_u =
                address(settings.get("addressu").or_else(|| settings.get("wrap")))
                    .unwrap_or(texture.sampler.address_u);
            texture.sampler.address_v =
                address(settings.get("addressv").or_else(|| settings.get("wrap")))
                    .unwrap_or(texture.sampler.address_v);
            texture.sampler.address_w =
                address(settings.get("addressw").or_else(|| settings.get("wrap")))
                    .unwrap_or(texture.sampler.address_w);
            texture.sampler.min_filter =
                filter(settings.get("minfilter").or_else(|| settings.get("filter")))
                    .unwrap_or(texture.sampler.min_filter);
            texture.sampler.mag_filter =
                filter(settings.get("magfilter").or_else(|| settings.get("filter")))
                    .unwrap_or(texture.sampler.mag_filter);
            texture.sampler.mip_filter =
                filter(settings.get("mipfilter")).unwrap_or(texture.sampler.mip_filter);
            material.samplers.insert(slot, texture.sampler);
        }
        material.textures.push(TextureBinding {
            name: format!("g_Texture{slot}"),
            slot,
            texture: id,
        });
        let [w, h, _] = frame.textures[&id].data.dimensions();
        let logical = self
            .textures
            .values()
            .find(|a| a.id == id)
            .map(|a| a.logical)
            .unwrap_or([w as f32, h as f32]);
        material.uniforms.values.insert(
            format!("g_Texture{slot}Resolution"),
            UniformValue::Vector(vec![w as f32, h as f32, logical[0], logical[1]]),
        );
        material.uniforms.values.insert(
            format!("g_Texture{slot}TexelSize"),
            UniformValue::Vector(vec![1. / w as f32, 1. / h as f32]),
        );
        material.uniforms.values.insert(
            format!("g_Texture{slot}MipMapInfo"),
            UniformValue::Float(frame.textures[&id].mips.len() as f32),
        );
        Ok(())
    }
    pub fn materials(
        &mut self,
        project: &Project,
        name: &str,
        overrides: Option<&Value>,
        frame: &mut RenderFrame,
        time: f32,
    ) -> Result<Vec<Material>> {
        self.materials_profile(
            project,
            name,
            overrides,
            frame,
            time,
            MaterialProfile::Ordinary,
        )
    }
    pub fn puppet_materials(
        &mut self,
        project: &Project,
        name: &str,
        overrides: Option<&Value>,
        frame: &mut RenderFrame,
        time: f32,
    ) -> Result<Vec<Material>> {
        self.materials_profile(
            project,
            name,
            overrides,
            frame,
            time,
            MaterialProfile::Puppet,
        )
    }
    pub fn model_materials(
        &mut self,
        project: &Project,
        name: &str,
        overrides: Option<&Value>,
        frame: &mut RenderFrame,
        time: f32,
    ) -> Result<Vec<Material>> {
        self.materials_profile(
            project,
            name,
            overrides,
            frame,
            time,
            MaterialProfile::Model,
        )
    }
    fn materials_profile(
        &mut self,
        project: &Project,
        name: &str,
        overrides: Option<&Value>,
        frame: &mut RenderFrame,
        time: f32,
        profile: MaterialProfile,
    ) -> Result<Vec<Material>> {
        let value = self.json(project, name)?;
        let passes = value
            .get("passes")
            .and_then(Value::as_array)
            .ok_or_else(|| Error::invalid(name, "material passes array required"))?;
        passes
            .iter()
            .enumerate()
            .map(|(index, pass)| {
                let mut pass = pass.clone();
                if matches!(profile, MaterialProfile::Model) {
                    if pass.get("depthtest").is_none() {
                        pass["depthtest"] = serde_json::json!("enabled");
                    }
                    if pass.get("depthwrite").is_none() {
                        pass["depthwrite"] = serde_json::json!(if pass
                            .get("blending")
                            .and_then(Value::as_str)
                            .is_some_and(|s| s != "normal")
                        {
                            "disabled"
                        } else {
                            "enabled"
                        });
                    }
                }
                if let Some(overrides) = overrides {
                    super::draw::merge_pass(&mut pass, overrides);
                }
                let skin_alpha = !matches!(profile, MaterialProfile::Ordinary)
                    && matches!(
                        pass.get("shader").and_then(Value::as_str),
                        Some("genericimage2" | "genericimage3" | "genericimage4")
                    );
                if skin_alpha
                    && let Some(combos) = pass.get_mut("combos").and_then(Value::as_object_mut)
                {
                    combos.remove("SKINNING_ALPHA");
                    combos.remove("BONECOUNT");
                }
                let mut material =
                    self.pass(project, &format!("{name}:{index}"), &pass, frame, time)?;
                if skin_alpha && material.vertex.is_some() {
                    if !pass.get("combos").is_some_and(Value::is_object) {
                        pass["combos"] = serde_json::json!({});
                    }
                    pass["combos"]["SKINNING"] = serde_json::json!(1);
                    pass["combos"]["SKINNING_ALPHA"] = serde_json::json!(1);
                    pass["combos"]["BONECOUNT"] = serde_json::json!(2);
                    material =
                        self.pass(project, &format!("{name}:{index}"), &pass, frame, time)?;
                    material
                        .uniforms
                        .values
                        .insert("wpe_CpuSkinAlpha".into(), UniformValue::Int(1));
                }
                Ok(material)
            })
            .collect()
    }
    pub fn pass(
        &mut self,
        project: &Project,
        name: &str,
        pass: &Value,
        frame: &mut RenderFrame,
        time: f32,
    ) -> Result<Material> {
        let mut material = Material {
            name: name.into(),
            ..Material::default()
        };
        if let Some(textures) = pass.get("textures").and_then(Value::as_array) {
            for (slot, value) in textures.iter().enumerate() {
                let texture = literal(value).as_str().or_else(|| {
                    value
                        .get("file")
                        .or_else(|| value.get("name"))
                        .and_then(Value::as_str)
                });
                if let Some(texture) = texture {
                    self.material_texture(
                        project,
                        texture,
                        slot as u32,
                        value,
                        &mut material,
                        frame,
                        time,
                    )?;
                }
            }
        }
        material.state.blend = match pass
            .get("blending")
            .and_then(Value::as_str)
            .unwrap_or("translucent")
        {
            "additive" => BlendState::Additive,
            "normal" => BlendState::Replace,
            "premultiplied" => BlendState::Premultiplied,
            "multiply" => BlendState::Multiply,
            "screen" => BlendState::Screen,
            _ => BlendState::Alpha,
        };
        material.state.depth_test = pass
            .get("depthtest")
            .and_then(Value::as_str)
            .is_some_and(|s| s != "disabled");
        material.state.depth_write = pass
            .get("depthwrite")
            .and_then(Value::as_str)
            .is_some_and(|s| s != "disabled");
        material.state.depth_compare = match pass
            .get("depthfunc")
            .and_then(Value::as_str)
            .unwrap_or("lessequal")
        {
            "less" => CompareFunction::Less,
            "equal" => CompareFunction::Equal,
            "greater" => CompareFunction::Greater,
            "always" => CompareFunction::Always,
            "never" => CompareFunction::Never,
            _ => CompareFunction::LessEqual,
        };
        material.state.cull = match pass
            .get("cullmode")
            .and_then(Value::as_str)
            .unwrap_or("nocull")
        {
            "cw" | "front" => CullMode::Front,
            "ccw" | "back" => CullMode::Back,
            _ => CullMode::None,
        };
        material
            .uniforms
            .values
            .insert("g_Time".into(), UniformValue::Float(time));
        material
            .uniforms
            .values
            .entry("g_Texture0TexelSize".into())
            .or_insert(UniformValue::Vector(vec![
                1. / frame.width as f32,
                1. / frame.height as f32,
            ]));
        if let Some(values) = pass.get("constantshadervalues").and_then(Value::as_object) {
            for (k, v) in values {
                if let Some(value) = uniform(v) {
                    material.uniforms.values.insert(k.clone(), value);
                }
            }
        }
        let shader = pass
            .get("shader")
            .and_then(Value::as_str)
            .unwrap_or("genericimage2");
        let mut defines =
            pass.get("combos")
                .and_then(Value::as_object)
                .map(|map| {
                    map.iter()
                        .map(|(k, v)| {
                            (
                                k.clone(),
                                literal(v).as_i64().unwrap_or_else(|| {
                                    i64::from(literal(v).as_bool().unwrap_or(false))
                                }) as i32,
                            )
                        })
                        .collect::<BTreeMap<_, _>>()
                })
                .unwrap_or_default();
        for (name, value) in &self.global_defines {
            defines.entry(name.clone()).or_insert(*value);
        }
        let authored_simple = defines.values().all(|value| *value == 0);
        for slot in 0..32 {
            let format = material
                .textures
                .iter()
                .find(|b| b.slot == slot)
                .map(|b| {
                    if let Some(asset) = self.textures.values().find(|a| a.id == b.texture) {
                        match asset.gpu_format {
                            8..=11 => asset.gpu_format as i32,
                            _ => 0,
                        }
                    } else {
                        match frame.textures[&b.texture].format {
                            wpe_render::TextureFormat::Rg16Float => 10,
                            wpe_render::TextureFormat::R16Float => 11,
                            wpe_render::TextureFormat::R8Unorm => 9,
                            _ => 0,
                        }
                    }
                })
                .unwrap_or(0);
            defines.entry(format!("TEX{slot}FORMAT")).or_insert(format);
        }
        defines.insert("GS_ENABLED".into(), 0);
        if shader.to_ascii_lowercase().contains("fur") {
            let instances = super::number(
                pass.get("instancecount").or_else(|| pass.get("instances")),
                32.,
            )
            .clamp(2., 128.) as i32;
            let instances = *defines.entry("INSTANCECOUNT".into()).or_insert(instances);
            material.uniforms.values.insert(
                "wpe_DrawInstances".into(),
                UniformValue::Int(instances.max(2)),
            );
        }
        let canonical = shader == "wpe:textured"
            || matches!(shader, "genericimage2" | "genericimage3" | "genericimage4")
                && authored_simple
                && !defines
                    .iter()
                    .any(|(name, value)| name.starts_with("TEX") && *value != 0)
                && pass
                    .get("constantshadervalues")
                    .is_none_or(|v| v.as_object().is_none_or(|o| o.is_empty()));
        if !canonical {
            let key = format!(
                "{}:{shader}:{defines:?}",
                self.scope.as_deref().unwrap_or("")
            );
            let pair = if let Some(value) = self.shaders.get(&key) {
                value.clone()
            } else {
                let scope = self.scope.clone();
                let compiler = self.compiler(project)?;
                let vertex = shader_path(&compiler.library, shader, "vert", scope.as_deref())
                    .ok_or_else(|| Error::ResourceNotFound(format!("shader {shader}.vert")))?;
                let fragment = shader_path(&compiler.library, shader, "frag", scope.as_deref())
                    .ok_or_else(|| Error::ResourceNotFound(format!("shader {shader}.frag")))?;
                let pair = compiler.compile_pair(&vertex, &fragment, &defines)?;
                let pair = (Arc::new(pair.0), Arc::new(pair.1));
                self.shaders.insert(key, pair.clone());
                pair
            };
            for hint in pair
                .0
                .resource_hints
                .iter()
                .chain(pair.1.resource_hints.iter())
            {
                if !material.textures.iter().any(|b| b.slot == hint.slot)
                    && let Some(texture) = hint.default_texture.as_deref().filter(|s| !s.is_empty())
                {
                    self.material_texture(
                        project,
                        texture,
                        hint.slot,
                        &Value::Null,
                        &mut material,
                        frame,
                        time,
                    )?;
                }
            }
            material.vertex = Some(pair.0);
            material.fragment = Some(pair.1);
            material.cpu = CpuProgram::GpuOnly {
                reason: format!("original shader {shader} requires its translated GPU program"),
            };
        }
        let _ = ShaderStage::Vertex;
        Ok(material)
    }
}
fn shader_path(
    library: &SourceLibrary,
    name: &str,
    extension: &str,
    scope: Option<&str>,
) -> Option<String> {
    let mut candidates = vec![
        format!("shaders/{name}.{extension}"),
        format!("{name}.{extension}"),
        name.into(),
    ];
    if let Some(scope) = scope {
        candidates.push(format!("{scope}/shaders/{name}.{extension}"));
        candidates.push(format!("{scope}/{name}.{extension}"));
    }
    candidates
        .into_iter()
        .find(|key| library.get(key).is_some())
}
pub(super) fn uniform(value: &Value) -> Option<UniformValue> {
    let value = literal(value);
    if let Some(n) = value.as_f64() {
        return Some(UniformValue::Float(n as f32));
    }
    if let Some(v) = value.as_bool() {
        return Some(UniformValue::Int(i32::from(v)));
    }
    let v = if let Some(s) = value.as_str() {
        s.split_whitespace()
            .map(str::parse::<f32>)
            .collect::<std::result::Result<Vec<_>, _>>()
            .ok()?
    } else {
        value
            .as_array()?
            .iter()
            .map(|v| v.as_f64().map(|v| v as f32))
            .collect::<Option<Vec<_>>>()?
    };
    if v.len() == 16 {
        Some(UniformValue::Matrix {
            columns: 4,
            rows: 4,
            values: v,
        })
    } else if v.len() == 1 {
        Some(UniformValue::Float(v[0]))
    } else {
        Some(UniformValue::Vector(v))
    }
}

fn uniform_json(value: UniformValue) -> Value {
    match value {
        UniformValue::Float(v) => serde_json::json!(v),
        UniformValue::Int(v) => serde_json::json!(v),
        UniformValue::Uint(v) => serde_json::json!(v),
        UniformValue::Vector(values) => Value::String(
            values
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(" "),
        ),
        UniformValue::Matrix { values, .. } => serde_json::json!(values),
        UniformValue::Array(values) => Value::Array(values.into_iter().map(uniform_json).collect()),
        UniformValue::Struct(values) => Value::Object(
            values
                .into_iter()
                .map(|(k, v)| (k, uniform_json(v)))
                .collect(),
        ),
    }
}
