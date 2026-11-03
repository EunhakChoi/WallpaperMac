//! Owned Metal/CPU shadow atlas orchestration around the retained PBR equations.
//! This is a working compatibility profile, not a claim of native shadow-kernel parity.
use super::*;
use std::sync::Arc;
use wpe_render::shader::{UniformValue, builtin_material};
use wpe_render::{
    BlendState, CompareFunction, CpuProgram, PipelineState, RenderPass, Target, TextureData,
    TextureFormat, Viewport,
};

const MAX_LIGHTS: usize = 16;
const SHADOW_TILE: u32 = 128;
const COOKIE_TILE: u32 = 64;

pub(super) const LIGHTING_REQUIREMENT_GLSL: &str = r#"
#if LIGHTING
uniform int wpe_LightCount;
uniform vec4 wpe_LightPositionRadius[16];
uniform vec4 wpe_LightColorExponent[16];
uniform vec4 wpe_LightDirectionCone[16];
uniform vec4 wpe_LightFeatures[16];
uniform vec4 wpe_LightCookie[16];
uniform mat4 g_ShadowMatrices[96];
uniform vec4 wpe_ShadowAtlas;
uniform sampler2D g_Texture24; // {"default":"_rt_wpe_shadow_atlas","hidden":true}
uniform sampler2D g_Texture25; // {"default":"_rt_wpe_cookie_atlas","hidden":true}
int wpe_light_face(vec3 d){
 vec3 a=abs(d);
 if(a.x>=a.y&&a.x>=a.z)return d.x>=0.0?0:1;
 if(a.y>=a.z)return d.y>=0.0?2:3;
 return d.z>=0.0?4:5;
}
vec3 wpe_light_projection(int light,vec3 position,out int face){
 face=wpe_LightFeatures[light].w<0.5?wpe_light_face(position-wpe_LightPositionRadius[light].xyz):0;
 vec4 projected=g_ShadowMatrices[light*6+face]*vec4(position,1.0);
 if(projected.w<=0.0)return vec3(-2.0);
 vec3 p=projected.xyz/projected.w;
 return vec3(p.x*0.5+0.5,0.5-p.y*0.5,p.z);
}
float wpe_point_shadow(int light,vec3 position){
 if(wpe_LightFeatures[light].x<0.5)return 1.0;
 int face;vec3 p=wpe_light_projection(light,position,face);
 if(any(lessThan(p,vec3(0.0)))||any(greaterThan(p,vec3(1.0))))return 1.0;
 int tile=int(wpe_ShadowAtlas.z);
 ivec2 origin=ivec2(face*tile,light*tile);
 ivec2 pixel=origin+ivec2(p.xy*float(tile));
 float visible=0.0;
 for(int y=-1;y<=1;y++)for(int x=-1;x<=1;x++){
  ivec2 q=clamp(pixel+ivec2(x,y),origin,origin+ivec2(tile-1));
  float stored=texelFetch(g_Texture24,q,0).r;
  visible+=p.z-wpe_LightFeatures[light].y<=stored?1.0:0.0;
 }
 return visible/9.0;
}
vec3 wpe_light_cookie(int light,vec3 position){
 if(wpe_LightCookie[light].x<0.5)return vec3(1.0);
 int face;vec3 p=wpe_light_projection(light,position,face);
 if(any(lessThan(p.xy,vec2(0.0)))||any(greaterThan(p.xy,vec2(1.0))))return vec3(0.0);
 vec2 uv=(vec2(float(face),float(light))+clamp(p.xy,vec2(0.5/64.0),vec2(1.0-0.5/64.0)))/vec2(6.0,max(float(wpe_LightCount),1.0));
 return textureLod(g_Texture25,uv,0.0).rgb;
}
vec3 PerformLighting_V1(vec3 worldPos,vec3 albedo,vec3 normal,vec3 viewDirection,vec3 specularTint,vec3 f0,float roughness,float metallic){
 vec3 result=vec3(0.0);
 for(int i=0;i<min(wpe_LightCount,16);i++){
  vec3 delta=wpe_LightPositionRadius[i].xyz-worldPos;
  vec3 color=wpe_LightColorExponent[i].rgb*wpe_light_cookie(i,worldPos);
  if(wpe_LightFeatures[i].w>1.5){
   result+=ComputePBRLightShadowInfinite(normal,-wpe_LightDirectionCone[i].xyz,viewDirection,albedo,color,specularTint,f0,roughness,metallic,wpe_point_shadow(i,worldPos));
  }else if(length(delta)>0.0&&wpe_LightPositionRadius[i].w>0.0){
   float cone=1.0;
   if(wpe_LightFeatures[i].w>0.5){
    float cosine=dot(normalize(-delta),wpe_LightDirectionCone[i].xyz);
    cone=smoothstep(wpe_LightDirectionCone[i].w,max(wpe_LightFeatures[i].z,wpe_LightDirectionCone[i].w+0.000001),cosine);
   }
   result+=ComputePBRLightShadow(normal,delta,viewDirection,albedo,color*cone,wpe_LightPositionRadius[i].w,wpe_LightColorExponent[i].w,specularTint,f0,roughness,metallic,wpe_point_shadow(i,worldPos));
  }
 }
 return result;
}
#endif
"#;

#[derive(Default)]
pub(super) struct LightingState {
    pub uniforms: BTreeMap<String, UniformValue>,
}
struct Light {
    position: Vec3,
    direction: Vec3,
    radius: f32,
    color: Vec3,
    exponent: f32,
    kind: u8,
    inner: f32,
    outer: f32,
    shadow: bool,
    bias: f32,
    cookie: Option<String>,
}
struct Caster {
    name: String,
    vertices: Vec<GpuVertex>,
    indices: Vec<u32>,
    texture: Option<u64>,
    alpha_cut: f32,
}

impl ScenePlayer {
    pub(super) fn prepare_lighting(
        &mut self,
        objects: &[Value],
        transforms: &[Mat4],
        frame: &mut RenderFrame,
    ) -> Result<LightingState> {
        if objects.len() != transforms.len() {
            return Err(Error::invalid(
                "lighting",
                "object/transform count mismatch",
            ));
        }
        let mut lights = Vec::new();
        for (at, object) in objects.iter().enumerate() {
            let Some(name) = object.get("light").and_then(Value::as_str) else {
                continue;
            };
            if !boolean(object.get("visible"), true) {
                continue;
            }
            if lights.len() == MAX_LIGHTS {
                self.issue(
                    "light-limit",
                    None,
                    "The current lighting profile supports at most 16 visible lights.",
                );
                break;
            }
            let name = name.to_ascii_lowercase();
            let kind = if name.contains("directional") {
                2
            } else if name.contains("spot") {
                1
            } else if name.contains("point") {
                0
            } else {
                self.issue(
                    "light-kind",
                    Some(&name),
                    "Unknown light kind; the light is not rendered.",
                );
                continue;
            };
            let radius = number(object.get("radius"), 100.).max(0.001);
            let direction = transforms[at].vector(Vec3::new(0., 0., -1.)).normalize();
            let direction = if direction.length_squared() > 0. {
                direction
            } else {
                Vec3::new(0., 0., -1.)
            };
            // The owned spot profile interprets these as half-cone angles in degrees.
            let outer = number(object.get("outercone"), 45.)
                .clamp(0.1, 89.5)
                .to_radians();
            let inner = number(object.get("innercone"), 30.)
                .clamp(0., outer.to_degrees())
                .to_radians();
            let cookie = if boolean(object.get("usecookie"), name.contains("cookie")) {
                object
                    .get("cookie")
                    .or_else(|| object.get("cookietexture"))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            } else {
                None
            };
            if boolean(object.get("usecookie"), name.contains("cookie")) && cookie.is_none() {
                self.issue("light-cookie-resource",Some(&object_key(object,at)),"Cookie lighting was requested without a cookie/cookietexture resource name; no cookie is substituted.");
            }
            lights.push(Light {
                position: transforms[at].translation(),
                direction,
                radius,
                color: vec3(object.get("color"), Vec3::ONE) * number(object.get("intensity"), 1.),
                exponent: number(object.get("exponent"), 2.).max(0.),
                kind,
                inner,
                outer,
                shadow: boolean(object.get("castshadow"), name.contains("shadow")),
                bias: number(object.get("shadowbias"), 0.001).max(0.),
                cookie,
            });
        }
        let rows = lights.len().max(1) as u32;
        let shadow = resource_id("wpe-shadow-atlas");
        let cookie_id = resource_id("wpe-cookie-atlas");
        frame.textures.insert(
            shadow,
            Texture::target_with_format(
                "wpe-shadow-atlas",
                SHADOW_TILE * 6,
                SHADOW_TILE * rows,
                TextureFormat::R32Float,
            ),
        );
        self.assets
            .targets
            .insert("_rt_wpe_shadow_atlas".into(), shadow);
        self.assets
            .targets
            .insert("_rt_wpe_cookie_atlas".into(), cookie_id);
        let mut cookie_pixels = vec![255u8; (COOKIE_TILE * 6 * COOKIE_TILE * rows * 4) as usize];
        let mut cookie_enabled = vec![false; lights.len()];
        for (i, light) in lights.iter().enumerate() {
            if let Some(name) = &light.cookie {
                let result = (|| -> Result<()> {
                    let id =
                        self.assets
                            .bind_texture(&self.project, name, frame, self.time as f32)?;
                    if !matches!(frame.textures[&id].data, TextureData::Image2D(_)) {
                        return Err(Error::Unsupported(
                            "light cookie must be a 2D decoded image in this profile".into(),
                        ));
                    }
                    for y in 0..COOKIE_TILE {
                        for x in 0..COOKIE_TILE {
                            let p = wpe_render::cpu::sample_texture(
                                frame,
                                id,
                                [
                                    (x as f32 + 0.5) / COOKIE_TILE as f32,
                                    (y as f32 + 0.5) / COOKIE_TILE as f32,
                                    0.,
                                ],
                                0.,
                            )?;
                            for face in 0..6 {
                                let at = (((i as u32 * COOKIE_TILE + y) * (COOKIE_TILE * 6)
                                    + face * COOKIE_TILE
                                    + x)
                                    * 4) as usize;
                                for c in 0..3 {
                                    cookie_pixels[at + c] =
                                        (p[c] * p[3] * 255.).clamp(0., 255.).round() as u8;
                                }
                            }
                        }
                    }
                    Ok(())
                })();
                match result {
                    Ok(()) => cookie_enabled[i] = true,
                    Err(error) => self.issue("light-cookie", Some(name), error.to_string()),
                }
            }
        }
        frame.textures.insert(
            cookie_id,
            Texture::rgba2d(
                "wpe-cookie-atlas",
                Arc::new(crate::RgbaImage::new(
                    COOKIE_TILE * 6,
                    COOKIE_TILE * rows,
                    cookie_pixels,
                )?),
            ),
        );
        let mut matrices = vec![Mat4::IDENTITY; MAX_LIGHTS * 6];
        for (i, light) in lights.iter().enumerate() {
            let projection = light_projections(light);
            matrices[i * 6..i * 6 + 6].copy_from_slice(&projection);
        }
        let mut state = LightingState::default();
        state.uniforms.insert(
            "wpe_LightCount".into(),
            UniformValue::Int(lights.len() as i32),
        );
        state.uniforms.insert(
            "g_ShadowMatrices".into(),
            UniformValue::Array(
                matrices
                    .iter()
                    .map(|m| UniformValue::Matrix {
                        columns: 4,
                        rows: 4,
                        values: m.0.to_vec(),
                    })
                    .collect(),
            ),
        );
        state.uniforms.insert(
            "wpe_ShadowAtlas".into(),
            UniformValue::Vector(vec![
                (SHADOW_TILE * 6) as f32,
                (SHADOW_TILE * rows) as f32,
                SHADOW_TILE as f32,
                rows as f32,
            ]),
        );
        for (name, values) in [
            (
                "wpe_LightPositionRadius",
                lights
                    .iter()
                    .map(|l| {
                        UniformValue::Vector(vec![
                            l.position.0[0],
                            l.position.0[1],
                            l.position.0[2],
                            l.radius,
                        ])
                    })
                    .collect(),
            ),
            (
                "wpe_LightColorExponent",
                lights
                    .iter()
                    .map(|l| {
                        UniformValue::Vector(vec![
                            l.color.0[0],
                            l.color.0[1],
                            l.color.0[2],
                            l.exponent,
                        ])
                    })
                    .collect(),
            ),
            (
                "wpe_LightDirectionCone",
                lights
                    .iter()
                    .map(|l| {
                        UniformValue::Vector(vec![
                            l.direction.0[0],
                            l.direction.0[1],
                            l.direction.0[2],
                            l.outer.cos(),
                        ])
                    })
                    .collect(),
            ),
            (
                "wpe_LightFeatures",
                lights
                    .iter()
                    .map(|l| {
                        UniformValue::Vector(vec![
                            if l.shadow { 1. } else { 0. },
                            l.bias,
                            l.inner.cos(),
                            l.kind as f32,
                        ])
                    })
                    .collect(),
            ),
            (
                "wpe_LightCookie",
                cookie_enabled
                    .iter()
                    .map(|enabled| {
                        UniformValue::Vector(vec![if *enabled { 1. } else { 0. }, 0., 0., 0.])
                    })
                    .collect(),
            ),
        ] {
            state
                .uniforms
                .insert(name.into(), UniformValue::Array(values));
        }
        // Initialize even an unshadowed atlas, so all required bindings have defined bytes.
        frame.passes.push(RenderPass {
            name: "shadow-atlas-clear".into(),
            target: Target::Texture(shadow),
            clear: Some([1., 0., 0., 1.]),
            ..Default::default()
        });
        if lights.iter().any(|light| light.shadow) {
            self.issue("owned-shadow-profile",None,"Shadows use an owned 128px/face R32 atlas, 3×3 nearest-depth PCF and CPU-skinned/alpha-cut casters. Spot cones use degree half-angles; point cookies repeat a 2D image across six faces. Native kernel/default parity and physical Mac GPU output remain unverified.");
            let casters = self.shadow_casters(objects, transforms, frame)?;
            let mut programs = BTreeMap::<(bool, u32), Material>::new();
            let mut total_vertices = 0usize;
            for (i, light) in lights.iter().enumerate().filter(|(_, l)| l.shadow) {
                let face_count = if light.kind == 0 { 6 } else { 1 };
                for face in 0..face_count {
                    let mut draws = Vec::new();
                    let camera = matrices[i * 6 + face];
                    for caster in &casters {
                        let vertices = caster
                            .vertices
                            .iter()
                            .map(|v| GpuVertex {
                                position: camera.transform4(v.position),
                                ..*v
                            })
                            .collect::<Vec<_>>();
                        if !intersects_clip(&vertices) {
                            continue;
                        }
                        total_vertices =
                            total_vertices.checked_add(vertices.len()).ok_or_else(|| {
                                Error::invalid("shadow atlas", "vertex count overflow")
                            })?;
                        if total_vertices > wpe_formats::MAX_ALLOCATION / 64 {
                            return Err(Error::invalid(
                                "shadow atlas",
                                "projected caster vertices exceed allocation policy",
                            ));
                        }
                        let key = (caster.texture.is_some(), caster.alpha_cut.to_bits());
                        if let std::collections::btree_map::Entry::Vacant(entry) =
                            programs.entry(key)
                        {
                            let program = Material {
                                name: "shadow-depth".into(),
                                cpu: CpuProgram::ShadowDepth {
                                    texture_slot: caster.texture.map(|_| 0),
                                    alpha_test: Some(caster.alpha_cut),
                                },
                                state: PipelineState {
                                    blend: BlendState::Replace,
                                    depth_test: true,
                                    depth_write: true,
                                    depth_compare: CompareFunction::LessEqual,
                                    ..Default::default()
                                },
                                ..Default::default()
                            };
                            entry.insert(builtin_material(&program)?);
                        }
                        let mut material = programs[&key].clone();
                        if let Some(texture) = caster.texture {
                            material.textures.push(TextureBinding {
                                name: "g_Texture0".into(),
                                slot: 0,
                                texture,
                            });
                        }
                        draws.push(frame.draws.len());
                        frame.draws.push(DrawCall {
                            name: format!("shadow-{}-{face}-{}", i, caster.name),
                            vertices,
                            indices: caster.indices.clone(),
                            material,
                            ..Default::default()
                        });
                    }
                    frame.passes.push(RenderPass {
                        name: format!("shadow-light-{i}-face-{face}"),
                        target: Target::Texture(shadow),
                        draws,
                        viewports: vec![Viewport {
                            x: face as f32 * SHADOW_TILE as f32,
                            y: i as f32 * SHADOW_TILE as f32,
                            width: SHADOW_TILE as f32,
                            height: SHADOW_TILE as f32,
                            z_near: 0.,
                            z_far: 1.,
                        }],
                        scissor: Some([
                            face as u32 * SHADOW_TILE,
                            i as u32 * SHADOW_TILE,
                            SHADOW_TILE,
                            SHADOW_TILE,
                        ]),
                        ..Default::default()
                    });
                }
            }
        }
        Ok(state)
    }

    fn shadow_casters(
        &mut self,
        objects: &[Value],
        transforms: &[Mat4],
        frame: &mut RenderFrame,
    ) -> Result<Vec<Caster>> {
        let mut casters = Vec::new();
        for (at, object) in objects.iter().enumerate() {
            if !boolean(object.get("visible"), true) || !boolean(object.get("castshadow"), true) {
                continue;
            }
            let key = object_key(object, at);
            let world = transforms[at];
            let alpha = number(object.get("alpha"), 1.);
            if alpha <= 0. {
                continue;
            }
            if let Some(model) = self.models.get(&key) {
                let meshes = model
                    .skinned_meshes()
                    .map_err(|e| Error::invalid(&key, e.to_string()))?;
                let image = object
                    .get("image")
                    .and_then(Value::as_str)
                    .map(|file| self.assets.json(&self.project, file))
                    .transpose()?;
                let offset = vec3(image.as_ref().and_then(|d| d.get("cropoffset")), Vec3::ZERO);
                for mesh in meshes
                    .into_iter()
                    .filter(|mesh| mesh.texture_channels.is_none())
                {
                    let material = image
                        .as_ref()
                        .and_then(|d| d.get("material"))
                        .and_then(Value::as_str)
                        .unwrap_or(&mesh.material);
                    let texture = match self.shadow_material_texture(material, frame) {
                        Ok(texture) => texture,
                        Err(error) => {
                            self.issue("shadow-material", Some(material), error.to_string());
                            continue;
                        }
                    };
                    let vertices = mesh
                        .vertices
                        .iter()
                        .map(|v| {
                            let p = world.point(Vec3(v.position) + offset);
                            GpuVertex {
                                position: [p.0[0], p.0[1], p.0[2], 1.],
                                uv: [v.uv[0], v.uv[1], 0., 0.],
                                color: [1., 1., 1., v.color[3] * alpha],
                                ..Default::default()
                            }
                        })
                        .collect();
                    casters.push(Caster {
                        name: key.clone(),
                        vertices,
                        indices: mesh.indices,
                        texture,
                        alpha_cut: number(object.get("alphatest"), 0.5).clamp(0., 1.),
                    });
                }
                continue;
            }
            let Some(file) = object.get("image").and_then(Value::as_str) else {
                continue;
            };
            let result = (|| -> Result<Caster> {
                let descriptor = self.assets.json(&self.project, file)?;
                if boolean(descriptor.get("fullscreen"), false)
                    || boolean(descriptor.get("passthrough"), false)
                    || boolean(descriptor.get("projectlayer"), false)
                {
                    return Err(Error::Unsupported(
                        "screen-space/passthrough image is not a world shadow caster".into(),
                    ));
                }
                let material = descriptor.get("material").and_then(Value::as_str);
                let texture = material
                    .map(|name| self.shadow_material_texture(name, frame))
                    .transpose()?
                    .flatten();
                let asset = texture
                    .and_then(|id| self.assets.textures.values().find(|a| a.id == id).cloned());
                let mut size = vec3(
                    object.get("size"),
                    Vec3::new(
                        number(descriptor.get("width"), 1.),
                        number(descriptor.get("height"), 1.),
                        0.,
                    ),
                )
                .0;
                let index = asset
                    .as_ref()
                    .map(|a| a.frame_index(self.time as f32))
                    .unwrap_or(0);
                if boolean(descriptor.get("autosize"), false)
                    && let Some(asset) = &asset
                {
                    let logical = asset.logical_frame(index);
                    size[0] = logical[0];
                    size[1] = logical[1];
                }
                let alignment = object
                    .get("alignment")
                    .or_else(|| object.get("anchor"))
                    .and_then(Value::as_str)
                    .unwrap_or("center");
                let anchor = [
                    if alignment.contains("left") {
                        0.
                    } else if alignment.contains("right") {
                        1.
                    } else {
                        0.5
                    },
                    if alignment.contains("top") {
                        1.
                    } else if alignment.contains("bottom") {
                        0.
                    } else {
                        0.5
                    },
                ];
                let vertices = [
                    ([0., 1.], [0., 0.]),
                    ([1., 1.], [1., 0.]),
                    ([0., 0.], [0., 1.]),
                    ([1., 0.], [1., 1.]),
                ]
                .into_iter()
                .map(|(p, uv)| {
                    let p = world.point(Vec3::new(
                        (p[0] - anchor[0]) * size[0],
                        (p[1] - anchor[1]) * size[1],
                        0.,
                    ));
                    let uv = asset
                        .as_ref()
                        .map(|a| {
                            let logical = a.logical_frame(index);
                            let inset = if boolean(descriptor.get("nopadding"), false) {
                                [0.; 2]
                            } else {
                                [0.15 / logical[0], 0.15 / logical[1]]
                            };
                            a.uv(
                                index,
                                [
                                    inset[0] + uv[0] * (1. - 2. * inset[0]),
                                    inset[1] + uv[1] * (1. - 2. * inset[1]),
                                ],
                            )
                        })
                        .unwrap_or(uv);
                    GpuVertex {
                        position: [p.0[0], p.0[1], p.0[2], 1.],
                        uv: [uv[0], uv[1], 0., 0.],
                        color: [1., 1., 1., alpha],
                        ..Default::default()
                    }
                })
                .collect();
                Ok(Caster {
                    name: key.clone(),
                    vertices,
                    indices: vec![0, 2, 1, 1, 2, 3],
                    texture,
                    alpha_cut: number(object.get("alphatest"), 0.5).clamp(0., 1.),
                })
            })();
            match result {
                Ok(caster) => casters.push(caster),
                Err(error) => self.issue("shadow-caster", Some(&key), error.to_string()),
            }
        }
        Ok(casters)
    }
    fn shadow_material_texture(
        &mut self,
        name: &str,
        frame: &mut RenderFrame,
    ) -> Result<Option<u64>> {
        if name.is_empty() {
            return Ok(None);
        }
        let value = self.assets.json(&self.project, name)?;
        let texture = value
            .pointer("/passes/0/textures/0")
            .and_then(|v| v.as_str().or_else(|| v.get("name").and_then(Value::as_str)));
        let Some(texture) = texture else {
            return Ok(None);
        };
        if texture.starts_with("_rt_") {
            return Err(Error::Unsupported(
                "shadow alpha cannot depend on an unproduced scene/effect target".into(),
            ));
        }
        let id = self
            .assets
            .bind_texture(&self.project, texture, frame, self.time as f32)?;
        if !matches!(frame.textures[&id].data, TextureData::Image2D(_)) {
            return Err(Error::Unsupported(
                "shadow alpha caster texture must be 2D".into(),
            ));
        }
        Ok(Some(id))
    }
}
fn light_projections(light: &Light) -> [Mat4; 6] {
    let near = (light.radius * 0.001).clamp(0.0001, 0.1);
    let far = light.radius.max(near * 2.);
    if light.kind == 2 {
        let eye = light.position - light.direction * light.radius;
        let mut projection = Mat4::orthographic(
            light.radius * 2.,
            light.radius * 2.,
            near,
            light.radius * 2.,
        );
        projection.0[12] = 0.;
        projection.0[13] = 0.;
        let m = projection * Mat4::look_at(eye, light.position, safe_up(light.direction));
        return [m; 6];
    }
    if light.kind == 1 {
        let m = Mat4::perspective(light.outer * 2., 1., near, far)
            * Mat4::look_at(
                light.position,
                light.position + light.direction,
                safe_up(light.direction),
            );
        return [m; 6];
    }
    let directions = [
        (Vec3::new(1., 0., 0.), Vec3::new(0., -1., 0.)),
        (Vec3::new(-1., 0., 0.), Vec3::new(0., -1., 0.)),
        (Vec3::new(0., 1., 0.), Vec3::new(0., 0., 1.)),
        (Vec3::new(0., -1., 0.), Vec3::new(0., 0., -1.)),
        (Vec3::new(0., 0., 1.), Vec3::new(0., -1., 0.)),
        (Vec3::new(0., 0., -1.), Vec3::new(0., -1., 0.)),
    ];
    directions.map(|(direction, up)| {
        Mat4::perspective(std::f32::consts::FRAC_PI_2, 1., near, far)
            * Mat4::look_at(light.position, light.position + direction, up)
    })
}
fn safe_up(direction: Vec3) -> Vec3 {
    if direction.0[1].abs() > 0.99 {
        Vec3::new(0., 0., 1.)
    } else {
        Vec3::new(0., 1., 0.)
    }
}
fn intersects_clip(vertices: &[GpuVertex]) -> bool {
    if vertices.is_empty() {
        return false;
    }
    !(0..6).any(|plane| {
        vertices.iter().all(|v| {
            let p = v.position;
            match plane {
                0 => p[0] < -p[3],
                1 => p[0] > p[3],
                2 => p[1] < -p[3],
                3 => p[1] > p[3],
                4 => p[2] < 0.,
                _ => p[2] > p[3],
            }
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scene_alpha_cut_image_actually_populates_shadow_atlas() {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        struct Owned(std::path::PathBuf);
        impl Drop for Owned {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let root = Owned(std::env::temp_dir().join(format!(
            "wpe-shadow-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        )));
        std::fs::create_dir_all(root.0.join("materials")).unwrap();
        std::fs::create_dir_all(root.0.join("models")).unwrap();
        let write = |name: &str, value: Value| {
            std::fs::write(root.0.join(name), serde_json::to_vec(&value).unwrap()).unwrap()
        };
        write("project.json", json!({"type":"scene","file":"scene.json"}));
        let objects = vec![
            json!({"id":1,"light":"pointshadow","origin":"0 0 5","radius":10,"castshadow":true}),
            json!({"id":2,"image":"models/image.json","origin":"0 0 0","size":"2 2","castshadow":true}),
        ];
        write("scene.json", json!({"objects":objects}));
        write(
            "models/image.json",
            json!({"width":2,"height":2,"nopadding":true,"material":"materials/image.json"}),
        );
        write(
            "materials/image.json",
            json!({"passes":[{"shader":"genericimage2","textures":["cutout"]}]}),
        );
        let image =
            crate::RgbaImage::new(2, 1, vec![255, 255, 255, 0, 255, 255, 255, 255]).unwrap();
        std::fs::write(
            root.0.join("materials/cutout.tex"),
            wpe_formats::tex::encode_rgba8(&image).unwrap(),
        )
        .unwrap();
        let mut player = ScenePlayer::new(crate::load_project(&root.0, None).unwrap()).unwrap();
        let transforms = hierarchy(&objects).unwrap();
        let mut frame = RenderFrame::new(2, 2);
        let state = player
            .prepare_lighting(&objects, &transforms, &mut frame)
            .unwrap();
        assert!(state.uniforms.contains_key("g_ShadowMatrices"));
        assert!(frame.draws.iter().any(|d| matches!(
            d.material.cpu,
            CpuProgram::ShadowDepth {
                texture_slot: Some(0),
                ..
            }
        )));
        let atlas = player.assets.targets["_rt_wpe_shadow_atlas"];
        let mut renderer = wpe_render::cpu::Renderer::new();
        renderer.render(&frame).unwrap();
        let projection = if let UniformValue::Array(m) = &state.uniforms["g_ShadowMatrices"] {
            if let UniformValue::Matrix { values, .. } = &m[5] {
                Mat4(values.clone().try_into().unwrap())
            } else {
                panic!()
            }
        } else {
            panic!()
        };
        let sample = |x: f32| {
            let p = projection.transform4([x, 0., 0., 1.]);
            let uv = [
                (5. + p[0] / p[3] * 0.5 + 0.5) / 6.,
                0.5 - p[1] / p[3] * 0.5,
                0.,
            ];
            renderer.sample(&frame, atlas, uv, 0.).unwrap()[0]
        };
        assert_eq!(sample(-0.5), 1.);
        assert!(sample(0.5) < 1.);
        assert!(sample(0.5) > 0.9);
    }
    #[test]
    fn all_cube_centers_map_inside_corresponding_projection() {
        let light = Light {
            position: Vec3::ZERO,
            direction: Vec3::new(0., 0., -1.),
            radius: 10.,
            color: Vec3::ONE,
            exponent: 2.,
            kind: 0,
            inner: 0.,
            outer: 0.7,
            shadow: true,
            bias: 0.001,
            cookie: None,
        };
        let matrices = light_projections(&light);
        for (i, p) in [
            Vec3::new(1., 0., 0.),
            Vec3::new(-1., 0., 0.),
            Vec3::new(0., 1., 0.),
            Vec3::new(0., -1., 0.),
            Vec3::new(0., 0., 1.),
            Vec3::new(0., 0., -1.),
        ]
        .into_iter()
        .enumerate()
        {
            let c = matrices[i].transform4([p.0[0], p.0[1], p.0[2], 1.]);
            assert!(c[0].abs() < 1e-6 && c[1].abs() < 1e-6 && c[2] > 0. && c[2] < c[3]);
        }
    }
    #[test]
    fn owned_lighting_template_actually_compiles_to_msl() {
        use wpe_render::shader::{ShaderCompiler, SourceLibrary};
        let mut library = SourceLibrary::new();
        library.insert("requirements/LightingV1.glsl", LIGHTING_REQUIREMENT_GLSL);
        library.insert(
            "test.vert",
            "attribute vec4 a_Position;void main(){gl_Position=a_Position;}",
        );
        library.insert("test.frag","vec3 ComputePBRLightShadow(vec3 a,vec3 b,vec3 c,vec3 d,vec3 e,float f,float g,vec3 h,vec3 i,float j,float k,float l){return e*l;}vec3 ComputePBRLightShadowInfinite(vec3 a,vec3 b,vec3 c,vec3 d,vec3 e,vec3 h,vec3 i,float j,float k,float l){return e*l;}\n#require LightingV1\nvoid main(){gl_FragColor=vec4(PerformLighting_V1(vec3(1),vec3(1),vec3(0,0,1),vec3(0,0,1),vec3(1),vec3(1),0.5,0.0),1);}");
        let (_, fragment) = ShaderCompiler::new(library)
            .compile_pair(
                "test.vert",
                "test.frag",
                &BTreeMap::from([("LIGHTING".into(), 1)]),
            )
            .unwrap();
        assert!(fragment.msl.contains("fragment"));
        assert!(fragment.bindings.iter().any(|b| b.texture_slot == Some(24)));
        assert!(fragment.pack_uniforms(&BTreeMap::new()).is_ok());
    }
    #[test]
    #[ignore = "requires WPE_SHADER_ROOT pointing to external retained selected-text"]
    fn retained_generic4_pbr_compiles_with_actual_atlas_template() {
        use wpe_render::shader::{ShaderCompiler, SourceLibrary};
        let mut library = SourceLibrary::from_directory(std::path::Path::new(
            &std::env::var_os("WPE_SHADER_ROOT").unwrap(),
        ))
        .unwrap();
        library.insert("requirements/LightingV1.glsl", LIGHTING_REQUIREMENT_GLSL);
        let defines = BTreeMap::from([
            ("LIGHTING".into(), 1),
            ("TEX0FORMAT".into(), 0),
            ("TEX1FORMAT".into(), 0),
            ("TEX4FORMAT".into(), 0),
            ("GS_ENABLED".into(), 0),
        ]);
        let (v, f) = ShaderCompiler::new(library)
            .compile_pair(
                "assets/shaders/generic4.vert",
                "assets/shaders/generic4.frag",
                &defines,
            )
            .unwrap();
        assert!(v.msl.contains("vertex"));
        assert!(f.msl.contains("fragment"));
        assert!(f.bindings.iter().any(|b| b.texture_slot == Some(24)));
        assert!(f.pack_uniforms(&BTreeMap::new()).is_ok());
    }
}
