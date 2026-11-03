//! Recovered auxiliary texture-channel production and CPU-skin shader inputs.
use super::draw::{quad_vertices, tangent_streams, target, vertex};
use super::*;
use wpe_render::shader::UniformValue;
use wpe_render::{
    BlendState, RenderPass, Target, TextureBinding, TextureFormat, VertexStream, VertexStreamData,
};
use wpe_sim::model::{ModelVertex, SkinnedMesh};

pub(super) struct ChannelImage<'a> {
    pub key: &'a str,
    pub label: &'a str,
    pub object: &'a Value,
    pub base: u64,
    pub logical: [u32; 2],
    pub uv_max: [f32; 2],
    pub meshes: &'a [SkinnedMesh],
    pub blend_map: [f32; 16],
}

impl ScenePlayer {
    pub(super) fn puppet_albedo(
        &mut self,
        image: ChannelImage<'_>,
        input: &PlayerInput,
        frame: &mut RenderFrame,
    ) -> Result<Option<u64>> {
        // Native selects the first flags2 mesh, independently of skin variants.
        let Some(mesh) = image
            .meshes
            .iter()
            .find(|mesh| mesh.texture_channels.is_some())
        else {
            return Ok(None);
        };
        let channels = mesh.texture_channels.as_ref().unwrap();
        let [width, height] = image.logical;
        if width == 0 || height == 0 || width > 16384 || height > 16384 {
            return Err(Error::invalid(
                image.label,
                "puppet channel target dimensions outside limit",
            ));
        }
        if mesh.material.is_empty() {
            return Err(Error::invalid(
                image.label,
                "puppet channel material required",
            ));
        }
        let format = if matches!(
            frame.textures[&image.base].format,
            TextureFormat::Rgba16Float
                | TextureFormat::Rgba32Float
                | TextureFormat::Rg16Float
                | TextureFormat::R16Float
        ) {
            TextureFormat::Rgba16Float
        } else {
            TextureFormat::Rgba8Unorm
        };
        let albedo = target(
            frame,
            &format!("_rt_imageLayerAlbedo_{}", image.key),
            width,
            height,
            format,
        );
        self.assets
            .targets
            .insert(format!("_rt_imagelayeralbedo_{}", image.key), albedo);
        let copy = frame.draws.len();
        frame.draws.push(DrawCall {
            name: format!("{}:channel-base-copy", image.label),
            vertices: quad_vertices(
                [2.; 2],
                [0.5; 2],
                [1.; 4],
                |uv| [uv[0] * image.uv_max[0], uv[1] * image.uv_max[1]],
                Mat4::IDENTITY,
                Mat4::IDENTITY,
                false,
            ),
            indices: vec![0, 2, 1, 1, 2, 3],
            material: Material {
                textures: vec![TextureBinding {
                    name: "g_Texture0".into(),
                    slot: 0,
                    texture: image.base,
                }],
                state: wpe_render::PipelineState {
                    blend: BlendState::Replace,
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        });
        frame.passes.push(RenderPass {
            name: format!("{}:channel-base-copy", image.label),
            target: Target::Texture(albedo),
            draws: vec![copy],
            ..Default::default()
        });
        let overrides = json!({"combos":{"BLENDROWCOUNT":channels.row_count}});
        let materials = self.assets.materials(
            &self.project,
            &mesh.material,
            Some(&overrides),
            frame,
            self.time as f32,
        )?;
        let projection = Mat4::orthographic(width as f32, height as f32, -1000., 1000.);
        let mut draws = Vec::new();
        for mut material in materials {
            self.uniforms(
                &mut material,
                Mat4::IDENTITY,
                projection,
                Mat4::IDENTITY,
                Vec3::new(0., 0., 1.),
                image.object,
                input,
            );
            material.uniforms.values.insert(
                "g_BlendMap".into(),
                UniformValue::Array(
                    image
                        .blend_map
                        .chunks_exact(4)
                        .take(channels.row_count as usize)
                        .map(|row| UniformValue::Vector(row.to_vec()))
                        .collect(),
                ),
            );
            let vertex_streams = BTreeMap::from([
                (
                    "a_TexCoordVec4".into(),
                    VertexStream {
                        components: 4,
                        data: VertexStreamData::Float32(
                            channels.tex_coords.iter().flatten().copied().collect(),
                        ),
                    },
                ),
                (
                    "a_BlendIndices".into(),
                    VertexStream {
                        components: 4,
                        data: VertexStreamData::Uint32(
                            channels.blend_indices.iter().flatten().copied().collect(),
                        ),
                    },
                ),
            ]);
            let vertices = mesh
                .vertices
                .iter()
                .map(|v| {
                    vertex(
                        Vec3(v.position),
                        Vec3(v.normal),
                        v.uv,
                        [1.; 4],
                        Mat4::IDENTITY,
                        projection,
                        material.vertex.is_some(),
                    )
                })
                .collect();
            let index = frame.draws.len();
            frame.draws.push(DrawCall {
                name: format!("{}:texture-channels", image.label),
                vertices,
                indices: mesh.indices.clone(),
                material,
                vertex_streams,
                ..Default::default()
            });
            draws.push(index);
        }
        frame.passes.push(RenderPass {
            name: format!("{}:texture-channels", image.label),
            target: Target::Texture(albedo),
            draws,
            ..Default::default()
        });
        Ok(Some(albedo))
    }
}

pub(super) fn bind_albedo(material: &mut Material, id: u64, frame: &RenderFrame) {
    if let Some(binding) = material
        .textures
        .iter_mut()
        .find(|binding| binding.slot == 0)
    {
        binding.texture = id;
        let texture = &frame.textures[&id];
        let [width, height, _] = texture.data.dimensions();
        material.uniforms.values.insert(
            "g_Texture0Resolution".into(),
            UniformValue::Vector(vec![
                width as f32,
                height as f32,
                width as f32,
                height as f32,
            ]),
        );
        material.uniforms.values.insert(
            "g_Texture0TexelSize".into(),
            UniformValue::Vector(vec![1. / width as f32, 1. / height as f32]),
        );
        material
            .uniforms
            .values
            .insert("g_Texture0MipMapInfo".into(), UniformValue::Float(1.));
    }
}

fn cpu_skin_alpha(material: &Material) -> bool {
    matches!(
        material.uniforms.values.get("wpe_CpuSkinAlpha"),
        Some(UniformValue::Int(1))
    )
}
pub(super) fn skin_uniforms(material: &mut Material) {
    if !cpu_skin_alpha(material) {
        return;
    }
    let identity = UniformValue::Matrix {
        columns: 4,
        rows: 3,
        values: vec![1., 0., 0., 0., 1., 0., 0., 0., 1., 0., 0., 0.],
    };
    material.uniforms.values.insert(
        "g_Bones".into(),
        UniformValue::Array(vec![identity.clone(), identity]),
    );
    material.uniforms.values.insert(
        "g_BonesAlpha".into(),
        UniformValue::Array(vec![UniformValue::Float(0.), UniformValue::Float(1.)]),
    );
}
pub(super) fn model_streams(
    vertices: &[ModelVertex],
    material: &Material,
) -> BTreeMap<String, VertexStream> {
    let mut streams = tangent_streams(vertices);
    if cpu_skin_alpha(material) {
        streams.insert(
            "a_BlendIndices".into(),
            VertexStream {
                components: 4,
                data: VertexStreamData::Uint32(
                    vertices.iter().flat_map(|_| [0, 1, 0, 0]).collect(),
                ),
            },
        );
        streams.insert(
            "a_BlendWeights".into(),
            VertexStream {
                components: 4,
                data: VertexStreamData::Float32(
                    vertices
                        .iter()
                        .flat_map(|v| [1. - v.skin_alpha, v.skin_alpha, 0., 0.])
                        .collect(),
                ),
            },
        );
    }
    streams
}
pub(super) fn model_color(vertex: &ModelVertex, tint: [f32; 4], material: &Material) -> [f32; 4] {
    if material.vertex.is_none() {
        return std::array::from_fn(|i| vertex.color[i] * tint[i]);
    }
    let mut color = vertex.color;
    if cpu_skin_alpha(material) {
        // The native fragment branch applies this factor once. At zero its
        // final alpha is zero regardless of the unobservable original alpha.
        color[3] = if vertex.skin_alpha > 0. {
            color[3] / vertex.skin_alpha
        } else {
            1.
        };
    }
    color
}
