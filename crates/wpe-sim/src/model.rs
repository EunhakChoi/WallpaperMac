use crate::{
    Error, Result,
    math::{Mat4, Quat, Vec3},
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use wpe_formats::mdl::{Clip, DenseTrack, Mesh, Model, MorphMesh, SectionData, SkinVertex};

#[derive(Clone, Copy, Debug)]
pub struct Transform {
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}
impl Default for Transform {
    fn default() -> Self {
        Self {
            translation: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        }
    }
}
impl Transform {
    pub fn matrix(self) -> Mat4 {
        Mat4::trs(self.translation, self.rotation, self.scale)
    }
    pub fn blend(self, b: Self, t: f32) -> Self {
        Self {
            translation: self.translation.lerp(b.translation, t),
            rotation: nlerp_shortest(self.rotation, b.rotation, t),
            scale: self.scale.lerp(b.scale, t),
        }
    }
    pub fn from_matrix(m: Mat4) -> Self {
        let x = Vec3::new(m.0[0], m.0[1], m.0[2]);
        let y = Vec3::new(m.0[4], m.0[5], m.0[6]);
        let z = Vec3::new(m.0[8], m.0[9], m.0[10]);
        let mut scale = Vec3::new(x.length(), y.length(), z.length());
        if x.dot(y.cross(z)) < 0. {
            scale.0[0] = -scale.0[0];
        }
        let mut r = m;
        for c in 0..3 {
            let divisor = scale.0[c];
            if divisor.abs() > 1e-20 {
                for row in 0..3 {
                    r.0[c * 4 + row] /= divisor
                }
            }
        }
        let trace = r.0[0] + r.0[5] + r.0[10];
        let q = if trace > 0. {
            let s = (trace + 1.).sqrt() * 2.;
            Quat([
                s * 0.25,
                (r.0[6] - r.0[9]) / s,
                (r.0[8] - r.0[2]) / s,
                (r.0[1] - r.0[4]) / s,
            ])
        } else {
            let axis = if r.0[0] > r.0[5] && r.0[0] > r.0[10] {
                0
            } else if r.0[5] > r.0[10] {
                1
            } else {
                2
            };
            let j = (axis + 1) % 3;
            let k = (axis + 2) % 3;
            let s = (1. + r.0[axis * 4 + axis] - r.0[j * 4 + j] - r.0[k * 4 + k])
                .max(0.)
                .sqrt()
                * 2.;
            let mut q = [0.; 4];
            if s > 1e-20 {
                q[axis + 1] = s * 0.25;
                q[0] = (r.0[j * 4 + k] - r.0[k * 4 + j]) / s;
                q[j + 1] = (r.0[j * 4 + axis] + r.0[axis * 4 + j]) / s;
                q[k + 1] = (r.0[k * 4 + axis] + r.0[axis * 4 + k]) / s;
            }
            Quat(q)
        };
        Self {
            translation: m.translation(),
            rotation: q.normalize(),
            scale,
        }
    }
}
#[derive(Clone, Debug)]
pub struct BoneState {
    pub name: String,
    pub parent: Option<usize>,
    pub bind: Transform,
    pub properties: Value,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaybackMode {
    Loop,
    Once,
    PingPong,
}
#[derive(Clone, Debug)]
pub struct AnimationLayer {
    pub clip: usize,
    pub time: f32,
    pub rate: f32,
    pub weight: f32,
    pub target_weight: f32,
    pub fade_seconds: f32,
    pub additive: bool,
    pub playing: bool,
    pub visible: bool,
    pub mode: PlaybackMode,
    pub direction: f32,
    previous_root_motion: Option<(Mat4, f32)>,
    external_clock: bool,
    external_previous_time: Option<f32>,
}
impl AnimationLayer {
    pub fn new(clip: usize) -> Self {
        Self {
            clip,
            time: 0.,
            rate: 1.,
            weight: 1.,
            target_weight: 1.,
            fade_seconds: 0.,
            additive: false,
            playing: true,
            visible: true,
            mode: PlaybackMode::Loop,
            direction: 1.,
            previous_root_motion: None,
            external_clock: false,
            external_previous_time: None,
        }
    }
    fn for_clip(index: usize, clip: &Clip) -> Self {
        let mut layer = Self::new(index);
        layer.mode = if clip.mode.eq_ignore_ascii_case("mirror") {
            PlaybackMode::PingPong
        } else if clip.mode.eq_ignore_ascii_case("single") {
            PlaybackMode::Once
        } else {
            PlaybackMode::Loop
        };
        layer
    }
    /// Synchronizes an external SceneScript/property clock without discarding
    /// the previous root-motion sample. No extra clock tick occurs here.
    pub fn set_external_time(&mut self, time: f32) -> Result<()> {
        if !time.is_finite() {
            return Err(Error("external animation time must be finite".into()));
        }
        if !self.external_clock {
            self.external_previous_time = Some(self.time);
        }
        self.external_clock = true;
        self.time = time;
        Ok(())
    }
    pub fn reset_root_motion_history(&mut self) {
        self.previous_root_motion = None;
        self.external_previous_time = Some(self.time);
    }
}
#[derive(Clone, Debug)]
pub struct ModelEvent {
    pub clip: String,
    pub frame: f32,
    pub payload: Value,
}
#[derive(Clone, Debug)]
pub struct Pose {
    pub local: Vec<Transform>,
    pub world: Vec<Mat4>,
    pub skin: Vec<Mat4>,
    pub events: Vec<ModelEvent>,
    pub mesh_weights: Vec<BTreeMap<u16, f32>>,
    pub morph_active_masks: Vec<u64>,
    /// Native MDLA group A feeds g_BonesAlpha for image puppets.
    pub bone_alpha: Vec<f32>,
    /// Native MDLA group B samples a discrete key for per-bone draw ordering.
    pub draw_depth: Vec<f32>,
    /// Native MDLA general scalar tracks feed g_BlendMap (up to four vec4 rows).
    pub blend_map: [f32; 16],
    pub root_motion: RootMotion,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct RootMotion {
    /// Model-local displacement. The scene host applies the layer world basis.
    pub translation: Vec3,
    /// Rotation about the native upright Y axis, in radians.
    pub yaw: f32,
}
#[derive(Clone, Debug)]
pub struct ModelVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub tangent: [f32; 4],
    pub uv: [f32; 2],
    pub color: [f32; 4],
    /// Interpolated bone-alpha factor before multiplication by vertex color.
    /// Allows original GPU skin-alpha shader branches without double alpha.
    pub skin_alpha: f32,
}
#[derive(Clone, Debug)]
pub struct SkinnedMesh {
    pub material: String,
    pub vertices: Vec<ModelVertex>,
    pub indices: Vec<u32>,
    /// Auxiliary puppet texture-channel geometry, rather than a second copy of
    /// the main image mesh. The renderer composites its actual material first.
    pub texture_channels: Option<TextureChannels>,
}
#[derive(Clone, Debug)]
pub struct TextureChannels {
    pub row_count: u8,
    pub tex_coords: Vec<[f32; 4]>,
    pub blend_indices: Vec<[u32; 4]>,
}
#[derive(Clone, Debug)]
struct EditableMesh {
    material: Option<String>,
    vertices: Vec<ModelVertex>,
    indices: Vec<u32>,
    skin: Option<Vec<SkinVertex>>,
}
#[derive(Clone, Debug)]
pub struct IkTarget {
    pub bone: usize,
    pub target: Vec3,
    pub chain_length: usize,
    pub iterations: usize,
    pub weight: f32,
}

pub struct ModelAnimator {
    pub model: Model,
    pub bones: Vec<BoneState>,
    pub clips: Vec<Clip>,
    pub layers: Vec<AnimationLayer>,
    pub bone_overrides: BTreeMap<usize, Transform>,
    /// Image-layer blend-shape ordinals address the first mesh's native target order.
    pub morph_weights: BTreeMap<usize, f32>,
    pub morph_names: Vec<String>,
    pub morph_data: Vec<MorphMesh>,
    pub issues: Vec<String>,
    pub root_motion_enabled: bool,
    /// Native scene `skin` selects a clamped material variant for every mesh.
    pub material_variant: usize,
    /// Owned editable-profile order overrides; skeleton indices stay stable.
    pub draw_depth_overrides: BTreeMap<usize, f32>,
    pub ik_targets: Vec<IkTarget>,
    pub pose: Pose,
    inverse_bind: Vec<Mat4>,
    spring_positions: Vec<Vec3>,
    spring_velocities: Vec<Vec3>,
    editable_meshes: Option<Vec<EditableMesh>>,
    source_bones: Vec<BoneState>,
    source_clips: Vec<Clip>,
    reference_clips: Vec<Clip>,
    editable_attachments: Option<Vec<(String, usize, Mat4)>>,
    last_editable: Option<Value>,
}
impl ModelAnimator {
    /// Builds the owned editable model profile, including sampled FBX local
    /// bone matrices. No native MDL writer or opaque binary upgrade is implied.
    pub fn from_editable(value: &Value) -> Result<Self> {
        let shapes = value
            .get("shapes")
            .and_then(Value::as_array)
            .ok_or_else(|| Error("editable model shapes required".into()))?;
        if shapes.len() > 100_000 {
            return Err(Error("editable mesh count limit".into()));
        }
        let meshes = shapes
            .iter()
            .map(|shape| Mesh {
                offset: 0,
                end: 0,
                materials: shape
                    .get("material")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(|s| vec![s.to_owned()])
                    .unwrap_or_default(),
                flags: 0,
                extra_flags: None,
                bounds: None,
                vertex_format: 1,
                vertex_stride: 12,
                vertex_count: 0,
                vertex_offset: 0,
                vertex_bytes: 0,
                vertices: Vec::new(),
                index_offset: 0,
                index_bytes: 0,
                indices: Vec::new(),
                index_count: 0,
                extra_buffers: Vec::new(),
                batches: Vec::new(),
                skin: None,
                active_skin_indices_in_bounds: None,
                skin_weights_finite: None,
            })
            .collect();
        let mut animator = Self::new(Model {
            version: 0,
            global_vertex_format: 1,
            material_slots: 1,
            meshes,
            sections: Vec::new(),
            file_bytes: 0,
            parsed_bytes: 0,
            unsupported_sections: Vec::new(),
        })?;
        animator.apply_editable_shapes(value)?;
        if let Some(clip) = animator.clips.first() {
            animator.layers = vec![AnimationLayer::for_clip(0, clip)];
        }
        Ok(animator)
    }
    pub fn new(model: Model) -> Result<Self> {
        let skeleton = model.sections.iter().find_map(|s| {
            if let SectionData::Skeleton(s) = &s.data {
                Some(s)
            } else {
                None
            }
        });
        let bones = skeleton
            .map(|s| {
                s.bones
                    .iter()
                    .map(|b| BoneState {
                        name: b.name.clone(),
                        parent: usize::try_from(b.parent).ok(),
                        bind: Transform::from_matrix(b.matrix.map(Mat4).unwrap_or_default()),
                        properties: serde_json::from_str(&b.properties_json).unwrap_or(Value::Null),
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for b in &bones {
            if b.parent.is_some_and(|p| p >= bones.len()) {
                return Err(Error("bone parent outside skeleton".into()));
            }
        }
        let local = bones.iter().map(|b| b.bind).collect::<Vec<_>>();
        let world = world_matrices(&bones, &local)?;
        let inverse_bind = world
            .iter()
            .map(|m| m.inverse().unwrap_or_default())
            .collect::<Vec<_>>();
        let clips = model
            .sections
            .iter()
            .flat_map(|s| {
                if let SectionData::Animation(c) = &s.data {
                    c.clone()
                } else {
                    Vec::new()
                }
            })
            .collect::<Vec<_>>();
        let mut morph_data = model
            .meshes
            .iter()
            .enumerate()
            .map(|(mesh, m)| MorphMesh {
                mesh,
                flags: m.flags,
                base_influence: 0.,
                declared_vertex_count: 0,
                effective_vertex_count: 0,
                targets: Vec::new(),
            })
            .collect::<Vec<_>>();
        for section in &model.sections {
            if let SectionData::Morph(section) = &section.data {
                for source in &section.meshes {
                    let target = morph_data
                        .get_mut(source.mesh)
                        .ok_or_else(|| Error("MDMP mesh outside model".into()))?;
                    if !source.targets.is_empty() {
                        target.base_influence = source.base_influence;
                        target.declared_vertex_count = source.declared_vertex_count;
                        target.effective_vertex_count = source.effective_vertex_count;
                        target.targets.extend(source.targets.iter().cloned());
                    }
                }
            }
        }
        let mut issues = Vec::new();
        for mesh in &morph_data {
            if !mesh.base_influence.is_finite() {
                return Err(Error("MDMP base influence is not finite".into()));
            }
            for target in &mesh.targets {
                if target.positions.records < model.meshes[mesh.mesh].vertex_count {
                    return Err(Error(format!(
                        "MDMP target {} has fewer vertices than its mesh",
                        target.name
                    )));
                }
            }
            if mesh.flags & 0x3800 != 0 && !mesh.targets.is_empty() {
                issues.push(format!("MDMP mesh {} preserves extra-vector/scalar/binding-rule channels; CPU morph applies position and normal channels",mesh.mesh));
            }
        }
        if let Some(skeleton) = skeleton
            && (!skeleton.constraints.is_empty()
                || !skeleton.table_a.is_empty()
                || !skeleton.table_b.is_empty())
        {
            issues.push(format!("[native-constraint-tables] {} constraints, {} table-A and {} table-B rows are preserved; the owned CCD IK/limits solver does not implement the unnamed native table solver",skeleton.constraints.len(),skeleton.table_a.len(),skeleton.table_b.len()));
        }
        if clips
            .iter()
            .any(|clip| !clip.secondary_tracks.is_empty() || !clip.constraint_tracks.is_empty())
        {
            issues.push("[secondary-pose-channels] secondary-transform/constraint tracks are preserved; primary skeleton, bone-alpha, draw-depth, scalar blend-map and mesh-morph channels execute".into());
        }
        if bones
            .iter()
            .any(|bone| bone.properties.get("se").and_then(Value::as_bool) == Some(true))
        {
            issues.push("[owned-spring-profile] enabled bone springs use the owned mass/stiffness/damped-translation solver, not a claim of native joint/cloth numeric equivalence".into());
        }
        if clips.iter().any(|clip| clip.flags & 0x14000 != 0) {
            issues.push("[root-motion-auxiliary-flags] MDLA flags0x4000/0x10000 are preserved; axis0x800/0x1000/0x2000 and yaw0x8000 extraction execute".into());
        }
        let morph_names = morph_data
            .first()
            .map(|m| m.targets.iter().map(|t| t.name.clone()).collect())
            .unwrap_or_default();
        let spring_positions = world.iter().map(|m| m.translation()).collect();
        let pose = Pose {
            local,
            skin: vec![Mat4::IDENTITY; bones.len()],
            world,
            events: Vec::new(),
            mesh_weights: vec![BTreeMap::new(); model.meshes.len()],
            morph_active_masks: vec![0; model.meshes.len()],
            bone_alpha: vec![1.; bones.len()],
            draw_depth: vec![0.; bones.len()],
            blend_map: [0.; 16],
            root_motion: RootMotion::default(),
        };
        Ok(Self {
            source_bones: bones.clone(),
            source_clips: clips.clone(),
            reference_clips: clips.clone(),
            layers: if clips.is_empty() {
                Vec::new()
            } else {
                vec![AnimationLayer::for_clip(0, &clips[0])]
            },
            spring_velocities: vec![Vec3::ZERO; bones.len()],
            model,
            bones,
            clips,
            bone_overrides: BTreeMap::new(),
            morph_weights: BTreeMap::new(),
            morph_names,
            morph_data,
            issues,
            root_motion_enabled: true,
            material_variant: 0,
            draw_depth_overrides: BTreeMap::new(),
            ik_targets: Vec::new(),
            pose,
            inverse_bind,
            spring_positions,
            editable_meshes: None,
            editable_attachments: None,
            last_editable: None,
        })
    }
    /// Applies checked editable geometry over the original model. Skeleton,
    /// clips, morph payloads and opaque/source metadata are retained unchanged.
    pub fn apply_editable_shapes(&mut self, value: &Value) -> Result<()> {
        if self.last_editable.as_ref() == Some(value) {
            return Ok(());
        }
        let metadata = editable_metadata(value, &self.source_bones, &self.source_clips)?;
        let shapes = value
            .get("shapes")
            .and_then(Value::as_array)
            .ok_or_else(|| Error("editable model requires shapes array".into()))?;
        if shapes.len() != self.model.meshes.len() {
            return Err(Error(
                "editable shape count must match source model meshes".into(),
            ));
        }
        let mut meshes = Vec::with_capacity(shapes.len());
        for (index, shape) in shapes.iter().enumerate() {
            if shape
                .get("sourceMesh")
                .and_then(Value::as_u64)
                .is_some_and(|n| n != index as u64)
            {
                return Err(Error("editable sourceMesh order mismatch".into()));
            }
            let format = shape
                .get("vertexFormat")
                .and_then(Value::as_array)
                .ok_or_else(|| Error("editable vertexFormat required".into()))?;
            let canonical = [
                ("position", 3),
                ("normal", 3),
                ("tangentSigned", 4),
                ("uv", 2),
                ("color", 4),
            ];
            let mut attributes = Vec::new();
            let mut prior = None;
            let mut stride = 0;
            for field in format {
                let attribute = canonical
                    .iter()
                    .position(|(name, _)| field.as_str() == Some(*name))
                    .ok_or_else(|| Error("unknown editable vertex attribute".into()))?;
                if prior.is_some_and(|p| attribute <= p) {
                    return Err(Error("editable vertex format is not canonical".into()));
                }
                prior = Some(attribute);
                attributes.push(attribute);
                stride += canonical[attribute].1;
            }
            if attributes.first() != Some(&0) {
                return Err(Error("editable position attribute required".into()));
            }
            let data = shape
                .get("vertexBuffer")
                .and_then(Value::as_array)
                .ok_or_else(|| Error("editable vertexBuffer required".into()))?;
            if data.len() % stride != 0 || data.len() / stride > 1_000_000 {
                return Err(Error("editable vertex count/stride limit".into()));
            }
            let mut vertices = Vec::with_capacity(data.len() / stride);
            for row in data.chunks_exact(stride) {
                let mut vertex = ModelVertex {
                    position: [0.; 3],
                    normal: [0., 0., 1.],
                    tangent: [1., 0., 0., 1.],
                    uv: [0.; 2],
                    color: [1.; 4],
                    skin_alpha: 1.,
                };
                let mut at = 0;
                for &attribute in &attributes {
                    let count = canonical[attribute].1;
                    let values = row[at..at + count]
                        .iter()
                        .map(|v| {
                            v.as_f64()
                                .map(|v| v as f32)
                                .filter(|v| v.is_finite())
                                .ok_or_else(|| Error("nonfinite editable vertex".into()))
                        })
                        .collect::<Result<Vec<_>>>()?;
                    match attribute {
                        0 => vertex.position.copy_from_slice(&values),
                        1 => vertex.normal.copy_from_slice(&values),
                        2 => vertex.tangent.copy_from_slice(&values),
                        3 => vertex.uv.copy_from_slice(&values),
                        4 => vertex.color.copy_from_slice(&values),
                        _ => unreachable!(),
                    }
                    at += count;
                }
                vertices.push(vertex);
            }
            let indices = shape
                .get("indexBuffer")
                .and_then(Value::as_array)
                .ok_or_else(|| Error("editable indexBuffer required".into()))?
                .iter()
                .map(|n| {
                    n.as_u64()
                        .filter(|n| *n < (vertices.len() as u64))
                        .and_then(|n| u32::try_from(n).ok())
                        .ok_or_else(|| Error("editable index out of bounds".into()))
                })
                .collect::<Result<Vec<_>>>()?;
            if !indices.len().is_multiple_of(3) {
                return Err(Error("editable triangle indices required".into()));
            }
            let skin = if let Some(weights) = shape.get("weights").and_then(Value::as_array) {
                if weights.len() != vertices.len() {
                    return Err(Error("editable skin count mismatch".into()));
                }
                Some(
                    weights
                        .iter()
                        .map(|row| {
                            if let Some(dense) = row.as_array() {
                                if dense.len() != metadata.bones.len() {
                                    return Err(Error("editable dense skin width must match skeleton".into()));
                                }
                                let active = dense.iter().enumerate().map(|(bone,value)| {
                                    value.as_f64().map(|v|v as f32).filter(|v|v.is_finite())
                                        .map(|weight|(bone,weight))
                                        .ok_or_else(||Error("nonfinite editable skin weight".into()))
                                }).collect::<Result<Vec<_>>>()?.into_iter().filter(|(_,weight)|*weight!=0.)
                                    .collect::<Vec<_>>();
                                if active.len()>4 {
                                    return Err(Error("editable native skin profile permits four active influences per vertex".into()));
                                }
                                let mut skin=SkinVertex{bone_indices:[0;4],weights:[0.;4]};
                                for (index,(bone,weight)) in active.into_iter().enumerate(){
                                    skin.bone_indices[index]=bone as u32;skin.weights[index]=weight;
                                }
                                return Ok(skin);
                            }
                            let weights = row
                                .get("weights")
                                .and_then(Value::as_array)
                                .filter(|a| a.len() == 4)
                                .ok_or_else(|| {
                                    Error("four editable skin weights required".into())
                                })?;
                            let ids = row
                                .get("boneIndices")
                                .and_then(Value::as_array)
                                .filter(|a| a.len() == 4)
                                .ok_or_else(|| {
                                    Error("four editable skin bone indices required".into())
                                })?;
                            let mut skin = SkinVertex {
                                bone_indices: [0; 4],
                                weights: [0.; 4],
                            };
                            for i in 0..4 {
                                skin.weights[i] = weights[i]
                                    .as_f64()
                                    .map(|v| v as f32)
                                    .filter(|v| v.is_finite())
                                    .ok_or_else(|| {
                                        Error("nonfinite editable skin weight".into())
                                    })?;
                                skin.bone_indices[i] = ids[i]
                                    .as_u64()
                                    .and_then(|v| u32::try_from(v).ok())
                                    .ok_or_else(|| Error("invalid editable bone index".into()))?;
                                if skin.weights[i] != 0.
                                    && skin.bone_indices[i] as usize >= metadata.bones.len()
                                {
                                    return Err(Error(
                                        "active editable skin bone outside skeleton".into(),
                                    ));
                                }
                            }
                            Ok(skin)
                        })
                        .collect::<Result<Vec<_>>>()?,
                )
            } else if vertices.len() == self.model.meshes[index].vertex_count {
                self.model.meshes[index].skin.clone()
            } else if self.model.meshes[index].skin.is_some() {
                return Err(Error(
                    "changed editable topology requires explicit skin weights".into(),
                ));
            } else {
                None
            };
            if let Some(skin) = &skin
                && (skin.len() != vertices.len()
                    || skin.iter().any(|v| {
                        (0..4).any(|i| {
                            !v.weights[i].is_finite()
                                || (v.weights[i] != 0.
                                    && v.bone_indices[i] as usize >= metadata.bones.len())
                        })
                    }))
            {
                return Err(Error("editable skin/source skeleton mismatch".into()));
            }
            if !self.morph_data[index].targets.is_empty()
                && vertices.len() != self.model.meshes[index].vertex_count
            {
                return Err(Error(
                    "editable topology changes require remapping native morph targets".into(),
                ));
            }
            meshes.push(EditableMesh {
                material: shape
                    .get("material")
                    .and_then(Value::as_str)
                    .filter(|material| {
                        shape.get("materialOverride").and_then(Value::as_bool) == Some(true)
                            || Some(*material)
                                != self.model.meshes[index]
                                    .materials
                                    .first()
                                    .map(String::as_str)
                    })
                    .map(str::to_owned),
                vertices,
                indices,
                skin,
            });
        }
        let mut order = BTreeMap::new();
        if let Some(depth) = value.get("boneDrawDepth").and_then(Value::as_array) {
            if depth.len() != metadata.bones.len() {
                return Err(Error("editable bone draw-depth count mismatch".into()));
            }
            for (bone, value) in depth.iter().enumerate() {
                let value = value
                    .as_f64()
                    .map(|v| v as f32)
                    .filter(|v| v.is_finite())
                    .ok_or_else(|| Error("nonfinite editable bone draw depth".into()))?;
                order.insert(bone, value);
            }
        }
        self.editable_meshes = Some(meshes);
        self.draw_depth_overrides = order;
        self.bones = metadata.bones;
        self.inverse_bind = metadata.inverse_bind;
        self.clips = metadata.clips;
        self.reference_clips = metadata.reference_clips;
        self.editable_attachments = metadata.attachments;
        if let Some(targets) = metadata.ik_targets {
            self.ik_targets = targets;
        }
        self.layers.retain(|layer| layer.clip < self.clips.len());
        self.bone_overrides
            .retain(|&bone, _| bone < self.bones.len());
        self.spring_positions = self
            .bones
            .iter()
            .map(|bone| bone.bind.translation)
            .collect();
        self.spring_velocities = vec![Vec3::ZERO; self.bones.len()];
        self.pose.local = self.bones.iter().map(|bone| bone.bind).collect();
        self.pose.world = world_matrices(&self.bones, &self.pose.local)?;
        self.pose.skin = vec![Mat4::IDENTITY; self.bones.len()];
        self.pose.bone_alpha = vec![1.; self.bones.len()];
        self.pose.draw_depth = vec![0.; self.bones.len()];
        self.last_editable = Some(value.clone());
        Ok(())
    }
    pub fn play(&mut self, clip: usize, fade: f32, additive: bool) -> Result<usize> {
        if clip >= self.clips.len() {
            return Err(Error("animation clip index outside model".into()));
        }
        for layer in &mut self.layers {
            layer.target_weight = 0.;
            layer.fade_seconds = fade;
        }
        let mut layer = AnimationLayer::for_clip(clip, &self.clips[clip]);
        layer.additive = additive;
        if fade > 0. {
            layer.weight = 0.;
            layer.fade_seconds = fade;
        }
        self.layers.push(layer);
        Ok(self.layers.len() - 1)
    }
    pub fn set_frame(&mut self, layer: usize, frame: f32) -> Result<()> {
        let l = self
            .layers
            .get_mut(layer)
            .ok_or_else(|| Error("unknown animation layer".into()))?;
        let c = &self.clips[l.clip];
        if !frame.is_finite() || !c.fps.is_finite() || c.fps <= 0. {
            return Err(Error("invalid frame/fps".into()));
        }
        l.time = frame / c.fps;
        Ok(())
    }
    pub fn frame(&self, layer: usize) -> Option<f32> {
        let l = self.layers.get(layer)?;
        Some(l.time * self.clips[l.clip].fps)
    }
    pub fn step(&mut self, dt: f32) -> Result<&Pose> {
        if !dt.is_finite() || dt.abs() > 60. {
            return Err(Error("invalid model frame delta".into()));
        }
        self.pose.local = self.bones.iter().map(|b| b.bind).collect();
        self.pose.events.clear();
        self.pose.bone_alpha.fill(1.);
        self.pose.draw_depth.fill(0.);
        self.pose.root_motion = RootMotion::default();
        for weights in &mut self.pose.mesh_weights {
            weights.clear();
        }
        self.pose.morph_active_masks.fill(0);
        let mut future_blends = vec![1.; self.layers.len()];
        let mut remaining = 1.;
        for (index, layer) in self.layers.iter().enumerate().rev() {
            future_blends[index] = remaining;
            if !layer.additive && layer.visible {
                remaining *= 1. - predicted_weight(layer, dt).clamp(0., 1.);
            }
        }
        for (layer_index, layer) in self.layers.iter_mut().enumerate() {
            if !layer.visible {
                continue;
            }
            let clip = self
                .clips
                .get(layer.clip)
                .ok_or_else(|| Error("animation layer clip outside model".into()))?;
            if clip.frames == 0 || !clip.fps.is_finite() || clip.fps <= 0. {
                continue;
            }
            let duration = clip.frames as f32 / clip.fps;
            let clock = advance_layer_clock(layer, dt, duration)?;
            if layer.fade_seconds > 0. {
                let amount = dt.abs() / layer.fade_seconds;
                layer.weight += (layer.target_weight - layer.weight).signum() * amount;
                if (layer.target_weight - layer.weight).abs() <= amount {
                    layer.weight = layer.target_weight;
                }
            } else {
                layer.weight = layer.target_weight;
            }
            let mut sample = frame_sample(layer.time, clip.fps, clip.frames);
            let pose_clip = if clip.flags & 0x401 == 1 {
                let transition = clip
                    .transition
                    .as_ref()
                    .ok_or_else(|| Error("referenced animation has no transition".into()))?;
                sample.0 = sample
                    .0
                    .checked_add(transition.words[0] as usize)
                    .ok_or_else(|| Error("referenced animation key overflow".into()))?;
                sample.1 = sample
                    .1
                    .checked_add(transition.words[0] as usize)
                    .ok_or_else(|| Error("referenced animation key overflow".into()))?;
                self.reference_clips
                    .get(transition.reference as usize)
                    .ok_or_else(|| Error("referenced animation outside clip table".into()))?
            } else {
                clip
            };
            let weight = layer.weight;
            if pose_clip.scalar_tracks.len() > self.pose.blend_map.len() {
                return Err(Error("native puppet scalar channel count exceeds16".into()));
            }
            for (index, track) in pose_clip.scalar_tracks.iter().enumerate() {
                let value = sample_scalar(track, sample)?;
                let current = &mut self.pose.blend_map[index];
                *current = if layer.additive {
                    *current + value * weight
                } else {
                    *current + (value - *current) * weight
                };
                if !current.is_finite() {
                    return Err(Error("puppet scalar channel overflow".into()));
                }
            }
            for (bone, track) in pose_clip
                .tracks
                .iter()
                .enumerate()
                .take(self.pose.local.len())
            {
                if track.flags & 1 != 0 {
                    continue;
                }
                let sampled = sample_transform(track, sample)?;
                if layer.additive {
                    let bind = self.bones[bone].bind;
                    let local = &mut self.pose.local[bone];
                    local.translation += (sampled.translation - bind.translation) * weight;
                    local.rotation = local.rotation
                        * nlerp_shortest(
                            Quat::IDENTITY,
                            bind.rotation.conjugate() * sampled.rotation,
                            weight,
                        );
                    local.scale += (sampled.scale - bind.scale) * weight;
                } else {
                    self.pose.local[bone] = self.pose.local[bone].blend(sampled, weight);
                }
            }
            if self.root_motion_enabled && clip.flags & 0x1f800 != 0 {
                apply_root_motion(
                    &self.bones,
                    &self.reference_clips,
                    clip,
                    pose_clip,
                    (sample, future_blends[layer_index]),
                    layer,
                    &mut self.pose,
                )?;
            } else {
                layer.previous_root_motion = None;
            }
            if let Some(tracks) = &clip.bone_scalar_group_a {
                for (bone, track) in tracks.iter().enumerate().take(self.bones.len()) {
                    if clip.tracks.get(bone).is_some_and(|t| t.flags & 3 == 1) {
                        continue;
                    }
                    let value = sample_scalar(track, sample)?;
                    let old = self.pose.bone_alpha[bone];
                    self.pose.bone_alpha[bone] = if layer.additive {
                        (old + (value - 1.) * weight).clamp(old.min(value), old.max(value))
                    } else {
                        (1. - weight) * old + weight * value
                    };
                }
            }
            if let Some(tracks) = &clip.bone_scalar_group_b {
                for (bone, track) in tracks.iter().enumerate().take(self.bones.len()) {
                    if clip.tracks.get(bone).is_some_and(|t| t.flags & 3 == 1) {
                        continue;
                    }
                    let value = *track
                        .values
                        .get(sample.0)
                        .filter(|v| v.is_finite())
                        .ok_or_else(|| Error("invalid bone draw-depth key".into()))?;
                    let old = self.pose.draw_depth[bone];
                    self.pose.draw_depth[bone] = if layer.additive {
                        (old + value * weight).clamp(old.min(value), old.max(value))
                    } else {
                        (1. - weight) * old + weight * value
                    };
                }
            }
            if let Some(groups) = &clip.mesh_channels {
                for (mesh, group) in groups.iter().enumerate().take(self.pose.mesh_weights.len()) {
                    if group.flags & 1 == 0 {
                        continue;
                    }
                    for channel in &group.channels {
                        let index = channel.target_id as usize;
                        if index >= self.morph_data[mesh].targets.len() {
                            return Err(Error(format!(
                                "MDLA morph ordinal {index} outside mesh {mesh} targets"
                            )));
                        }
                        let a = channel
                            .values
                            .get(sample.0)
                            .copied()
                            .ok_or_else(|| Error("morph channel first key missing".into()))?;
                        let b =
                            channel.values.get(sample.1).copied().ok_or_else(|| {
                                Error("morph channel sentinel key missing".into())
                            })?;
                        let sampled = (1. - sample.2) * a + sample.2 * b;
                        if !sampled.is_finite() {
                            return Err(Error("nonfinite morph weight sample".into()));
                        }
                        let previous = self.pose.mesh_weights[mesh]
                            .get(&channel.target_id)
                            .copied()
                            .unwrap_or(0.);
                        let bit = 1_u64 << (index & 63);
                        if weight == 1. && !layer.additive && sampled.abs() < f32::EPSILON {
                            self.pose.morph_active_masks[mesh] &= !bit;
                            continue;
                        }
                        if sampled.abs() >= f32::EPSILON {
                            self.pose.morph_active_masks[mesh] |= bit;
                        }
                        let value = if layer.additive {
                            let delta = sampled * weight;
                            (previous + delta).clamp(previous.min(delta), previous.max(delta))
                        } else if weight == 1. {
                            sampled
                        } else {
                            ((1. - weight) * previous + weight * sampled).clamp(0., 1.)
                        };
                        self.pose.mesh_weights[mesh].insert(channel.target_id, value);
                    }
                }
            }
            for phase in 0..2 {
                for event in &clip.events {
                    // MDLA stores clock seconds. Authored JSON event.frame is
                    // separately converted to seconds by the native JSON loader.
                    let at = f32::from_bits(event.frame_word);
                    if clock.crossed(at, phase) {
                        self.pose.events.push(ModelEvent {
                            clip: clip.name.clone(),
                            frame: at * clip.fps,
                            payload: serde_json::from_str(&event.text)
                                .unwrap_or_else(|_| Value::String(event.text.clone())),
                        });
                    }
                }
            }
        }
        self.layers
            .retain(|l| l.weight != 0. || l.target_weight != 0.);
        for (&index, &value) in &self.bone_overrides {
            if let Some(local) = self.pose.local.get_mut(index) {
                *local = value;
            }
        }
        for (i, bone) in self.bones.iter().enumerate() {
            apply_limits(&mut self.pose.local[i], &bone.properties)?;
        }
        self.pose.world = world_matrices(&self.bones, &self.pose.local)?;
        for target in &self.ik_targets {
            solve_ik(
                &self.bones,
                &mut self.pose.local,
                &mut self.pose.world,
                target,
            )?;
        }
        for (i, bone) in self.bones.iter().enumerate() {
            if bone.properties.get("se").and_then(Value::as_bool) == Some(true) && dt > 0. {
                let goal = self.pose.world[i].translation();
                let mass = crate::particle::number(&bone.properties, "m", 1000.).max(0.001);
                let stiffness = crate::particle::number(&bone.properties, "rs", 100.);
                let damping = crate::particle::number(&bone.properties, "gd", 5.);
                self.spring_velocities[i] +=
                    (goal - self.spring_positions[i]) * (stiffness / mass * dt);
                self.spring_velocities[i] *= (-damping * dt).exp();
                self.spring_positions[i] += self.spring_velocities[i] * dt;
                self.pose.world[i].0[12..15].copy_from_slice(&self.spring_positions[i].0);
            } else {
                self.spring_positions[i] = self.pose.world[i].translation();
            }
        }
        self.pose.skin = self
            .pose
            .world
            .iter()
            .zip(&self.inverse_bind)
            .map(|(a, b)| *a * *b)
            .collect();
        if self.pose.world.iter().any(|m| !m.finite()) {
            return Err(Error("nonfinite model pose".into()));
        }
        Ok(&self.pose)
    }
    pub fn skinned_meshes(&self) -> Result<Vec<SkinnedMesh>> {
        self.model
            .meshes
            .iter()
            .enumerate()
            .map(|(mesh_index, mesh)| {
                let active_morphs = self.active_morph_targets(mesh_index)?;
                let edited = self
                    .editable_meshes
                    .as_ref()
                    .map(|meshes| &meshes[mesh_index]);
                let count = edited.map_or(mesh.vertex_count, |mesh| mesh.vertices.len());
                let skin = edited.map_or(mesh.skin.as_ref(), |mesh| mesh.skin.as_ref());
                let mut vertices = Vec::with_capacity(count);
                for i in 0..count {
                    let mut vertex = if let Some(mesh) = edited {
                        mesh.vertices[i].clone()
                    } else {
                        read_vertex(mesh, i)?
                    };
                    self.apply_morph(mesh_index, i, &mut vertex, &active_morphs)?;
                    if let Some(weights) = skin {
                        let skin = &weights[i];
                        let (mut position, mut normal, mut tangent) =
                            (Vec3::ZERO, Vec3::ZERO, Vec3::ZERO);
                        let mut any = false;
                        let mut bone_alpha = 0.;
                        for j in 0..4 {
                            let weight = skin.weights[j];
                            if weight == 0. {
                                continue;
                            }
                            let m = *self
                                .pose
                                .skin
                                .get(skin.bone_indices[j] as usize)
                                .ok_or_else(|| Error("skin bone index out of range".into()))?;
                            bone_alpha += self
                                .pose
                                .bone_alpha
                                .get(skin.bone_indices[j] as usize)
                                .copied()
                                .unwrap_or(1.)
                                * weight;
                            position += m.point(Vec3(vertex.position)) * weight;
                            normal += m.vector(Vec3(vertex.normal)) * weight;
                            tangent += m.vector(Vec3::new(
                                vertex.tangent[0],
                                vertex.tangent[1],
                                vertex.tangent[2],
                            )) * weight;
                            any = true;
                        }
                        if any {
                            vertex.position = position.0;
                            vertex.normal = normal.normalize().0;
                            vertex.tangent[..3].copy_from_slice(&tangent.normalize().0);
                        }
                        vertex.skin_alpha = bone_alpha.clamp(0., 1.);
                        vertex.color[3] *= vertex.skin_alpha;
                    }
                    if vertex
                        .position
                        .iter()
                        .chain(&vertex.normal)
                        .chain(&vertex.tangent)
                        .chain(&vertex.uv)
                        .chain(&vertex.color)
                        .any(|v| !v.is_finite())
                    {
                        return Err(Error("skinned vertex is nonfinite".into()));
                    }
                    vertices.push(vertex);
                }
                let mut indices = edited.map_or_else(
                    || mesh.indices.iter().map(|v| *v as u32).collect::<Vec<_>>(),
                    |m| m.indices.clone(),
                );
                if let Some(skin) = skin
                    && (!self.draw_depth_overrides.is_empty()
                        || self.pose.draw_depth.iter().any(|v| *v != 0.))
                {
                    // Editable-profile ordering: stable triangle order by the
                    // dominant bone, preserving every parent/skin index. The
                    // native renderer instead sorts its authored bone batches.
                    let mut triangles = indices
                        .chunks_exact(3)
                        .map(|triangle| {
                            let mut influence = BTreeMap::<usize, f32>::new();
                            for &index in triangle {
                                let skin = &skin[index as usize];
                                for i in 0..4 {
                                    if skin.weights[i] != 0. {
                                        *influence
                                            .entry(skin.bone_indices[i] as usize)
                                            .or_default() += skin.weights[i];
                                    }
                                }
                            }
                            let dominant = influence
                                .into_iter()
                                .fold(None, |best: Option<(usize, f32)>, (bone, weight)| {
                                    if best.is_none_or(|(_, old)| weight > old) {
                                        Some((bone, weight))
                                    } else {
                                        best
                                    }
                                })
                                .map(|(bone, _)| bone);
                            let depth = dominant
                                .and_then(|bone| {
                                    self.draw_depth_overrides
                                        .get(&bone)
                                        .copied()
                                        .or_else(|| self.pose.draw_depth.get(bone).copied())
                                })
                                .unwrap_or(0.);
                            if !depth.is_finite() {
                                return Err(Error("nonfinite bone draw depth".into()));
                            }
                            Ok((depth, [triangle[0], triangle[1], triangle[2]]))
                        })
                        .collect::<Result<Vec<_>>>()?;
                    triangles.sort_by(|a, b| a.0.total_cmp(&b.0));
                    indices = triangles
                        .into_iter()
                        .flat_map(|(_, triangle)| triangle)
                        .collect();
                }
                Ok(SkinnedMesh {
                    material: edited
                        .and_then(|mesh| mesh.material.clone())
                        .unwrap_or_else(|| {
                            mesh.materials
                                .get(if mesh.flags & 2 != 0 {
                                    0
                                } else {
                                    self.material_variant
                                        .min(mesh.materials.len().saturating_sub(1))
                                })
                                .cloned()
                                .unwrap_or_default()
                        }),
                    vertices,
                    indices,
                    texture_channels: texture_channels(mesh, count)?,
                })
            })
            .collect()
    }
    /// Native renderer gathers the first eleven active ordinals, then stable
    /// sorts that bounded set by descending weight; it is not global top-k.
    pub fn active_morph_targets(&self, mesh: usize) -> Result<Vec<(usize, f32)>> {
        let data = self
            .morph_data
            .get(mesh)
            .ok_or_else(|| Error("morph mesh outside model".into()))?;
        let mut selected = Vec::new();
        let mut mask = self.pose.morph_active_masks.get(mesh).copied().unwrap_or(0);
        if mesh == 0 {
            // Native image setter uses a signed32-bit shift widened to64 bits.
            // Snapshot overrides have a deterministic ascending-ordinal order;
            // aliased indices>=32 have no preserved JS call-order oracle.
            for (&index, &weight) in &self.morph_weights {
                if index >= data.targets.len() {
                    continue;
                }
                if !weight.is_finite() {
                    return Err(Error("nonfinite morph override".into()));
                }
                let bit = (1_u32 << ((index as u32) & 31)) as i32 as i64 as u64;
                if weight.abs() >= f32::EPSILON {
                    mask |= bit;
                } else {
                    mask &= !bit;
                }
            }
        }
        for index in 0..data.targets.len() {
            let explicit = if mesh == 0 {
                self.morph_weights.get(&index).copied()
            } else {
                None
            };
            let weight = explicit
                .or_else(|| {
                    u16::try_from(index)
                        .ok()
                        .and_then(|i| self.pose.mesh_weights.get(mesh)?.get(&i).copied())
                })
                .unwrap_or(0.);
            if !weight.is_finite() {
                return Err(Error("nonfinite morph override".into()));
            }
            if mask >> (index & 63) & 1 != 0 {
                selected.push((index, weight));
                if selected.len() == 11 {
                    break;
                }
            }
        }
        selected.sort_by(|a, b| b.1.total_cmp(&a.1));
        Ok(selected)
    }
    fn apply_morph(
        &self,
        mesh: usize,
        vertex_index: usize,
        vertex: &mut ModelVertex,
        selected: &[(usize, f32)],
    ) -> Result<()> {
        let Some(data) = self.morph_data.get(mesh) else {
            return Ok(());
        };
        if data.targets.is_empty() {
            return Ok(());
        }
        let (mut position, mut normal) = (Vec3::ZERO, Vec3::ZERO);
        let mut normals = data.flags & 0x400 != 0;
        for &(index, weight) in selected {
            let target = &data.targets[index];
            let start = vertex_index * 3;
            let values = target
                .positions
                .values
                .get(start..start + 3)
                .ok_or_else(|| Error("morph position outside HALF payload".into()))?;
            let delta = Vec3::new(values[0], values[1], values[2]);
            if !delta.finite() {
                return Err(Error("nonfinite active morph delta".into()));
            }
            position += delta * weight;
            if let Some(channel) = &target.normals {
                let values = channel
                    .values
                    .get(start..start + 3)
                    .ok_or_else(|| Error("morph normal outside HALF payload".into()))?;
                let delta = Vec3::new(values[0], values[1], values[2]);
                if !delta.finite() {
                    return Err(Error("nonfinite active morph normal".into()));
                }
                normal += delta * weight;
                normals = true;
            }
        }
        vertex.position = (Vec3(vertex.position) + position * data.base_influence).0;
        if normals {
            vertex.normal = (Vec3(vertex.normal) + normal * 3.465).normalize().0;
        }
        if !Vec3(vertex.position).finite() || !Vec3(vertex.normal).finite() {
            return Err(Error("morphed vertex overflow".into()));
        }
        Ok(())
    }
    pub fn physics_impulse(
        &mut self,
        index: usize,
        directional: Vec3,
        angular: Vec3,
    ) -> Result<()> {
        if index >= self.bones.len() || !directional.finite() || !angular.finite() {
            return Err(Error("invalid bone physics impulse".into()));
        }
        let mass = crate::particle::number(&self.bones[index].properties, "m", 1000.).max(0.001);
        self.spring_velocities[index] += directional / mass;
        if angular.length_squared() > 0. {
            let mut transform = self
                .bone_overrides
                .get(&index)
                .copied()
                .unwrap_or(self.pose.local[index]);
            transform.rotation = transform.rotation * Quat::euler_xyz(angular / mass);
            self.bone_overrides.insert(index, transform);
        }
        Ok(())
    }
    pub fn reset_physics(&mut self, index: Option<usize>) -> Result<()> {
        if index.is_some_and(|i| i >= self.bones.len()) {
            return Err(Error("invalid bone physics reset".into()));
        }
        for i in 0..self.bones.len() {
            if index.is_none_or(|selected| selected == i) {
                self.spring_positions[i] = self.pose.world[i].translation();
                self.spring_velocities[i] = Vec3::ZERO;
            }
        }
        Ok(())
    }
    pub fn attachment(&self, name: &str) -> Option<Mat4> {
        if let Some(rows) = &self.editable_attachments {
            return rows
                .iter()
                .find(|(key, _, _)| key == name)
                .and_then(|(_, bone, matrix)| {
                    self.pose.world.get(*bone).map(|world| *world * *matrix)
                });
        }
        for s in &self.model.sections {
            if let SectionData::Attachments(attachments) = &s.data {
                for a in attachments {
                    if a.name == name {
                        return self
                            .pose
                            .world
                            .get(a.bone as usize)
                            .map(|world| *world * Mat4(a.matrix));
                    }
                }
            }
        }
        None
    }
}
struct EditableMetadata {
    bones: Vec<BoneState>,
    inverse_bind: Vec<Mat4>,
    clips: Vec<Clip>,
    reference_clips: Vec<Clip>,
    attachments: Option<Vec<(String, usize, Mat4)>>,
    ik_targets: Option<Vec<IkTarget>>,
}
fn editable_vector(value: &Value, count: usize) -> Result<Vec<f32>> {
    let value = value
        .get("elements")
        .or_else(|| value.get("m"))
        .or_else(|| value.get("value"))
        .unwrap_or(value);
    let values = if let Some(text) = value.as_str() {
        text.split_whitespace()
            .map(|word| {
                word.parse::<f32>()
                    .ok()
                    .filter(|v| v.is_finite())
                    .ok_or_else(|| Error("finite editable matrix/vector required".into()))
            })
            .collect::<Result<Vec<_>>>()?
    } else {
        value
            .as_array()
            .ok_or_else(|| Error("editable matrix/vector array required".into()))?
            .iter()
            .map(|v| {
                v.as_f64()
                    .map(|v| v as f32)
                    .filter(|v| v.is_finite())
                    .ok_or_else(|| Error("finite editable matrix/vector required".into()))
            })
            .collect::<Result<Vec<_>>>()?
    };
    if values.len() != count {
        return Err(Error(format!(
            "editable vector requires {count} components"
        )));
    }
    Ok(values)
}
fn editable_matrix(value: &Value) -> Result<Mat4> {
    Ok(Mat4(editable_vector(value, 16)?.try_into().unwrap()))
}
fn constant_track(transform: Transform, frames: u32) -> DenseTrack {
    let q = transform.rotation;
    let [w, x, y, z] = q.0;
    let angles = [
        (2. * (w * x + y * z)).atan2(1. - 2. * (x * x + y * y)),
        (2. * (w * y - z * x)).clamp(-1., 1.).asin(),
        (2. * (w * z + x * y)).atan2(1. - 2. * (y * y + z * z)),
    ];
    let key = [
        transform.translation.0.as_slice(),
        angles.as_slice(),
        transform.scale.0.as_slice(),
    ]
    .concat();
    let values = key.repeat(frames as usize + 1);
    DenseTrack {
        flags: 0,
        offset: 0,
        bytes: values.len() * 4,
        stride: 36,
        record_count: frames as usize + 1,
        dense_frame_count_matches: true,
        raw_data: values.iter().flat_map(|f| f.to_le_bytes()).collect(),
        values,
    }
}
fn constant_scalar(value: f32, frames: u32) -> DenseTrack {
    let values = vec![value; frames as usize + 1];
    DenseTrack {
        flags: 0,
        offset: 0,
        bytes: values.len() * 4,
        stride: 4,
        record_count: values.len(),
        dense_frame_count_matches: true,
        raw_data: values.iter().flat_map(|f| f.to_le_bytes()).collect(),
        values,
    }
}
fn editable_matrix_clip(
    spec: &Value,
    matrices: &Value,
    bones: &[BoneState],
    index: usize,
) -> Result<Clip> {
    let rows = matrices
        .as_array()
        .filter(|a| a.len() == bones.len())
        .ok_or_else(|| Error("boneMatrices width must match editable skeleton".into()))?;
    let frames = spec
        .get("frames")
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| *n > 0 && *n <= 1_000_000)
        .ok_or_else(|| Error("sampled clip needs1..1000000 frames".into()))?;
    let fps = spec
        .get("fps")
        .and_then(Value::as_f64)
        .map(|v| v as f32)
        .filter(|v| v.is_finite() && *v > 0. && *v <= 100_000.)
        .ok_or_else(|| Error("sampled clip fps must be positive finite".into()))?;
    let sample_count = (frames as usize + 1)
        .checked_mul(bones.len())
        .filter(|n| *n <= 16_000_000)
        .ok_or_else(|| Error("sampled clip matrix limit".into()))?;
    let _ = sample_count;
    let tracks=rows.iter().map(|row|{
        let keys=row.as_array().filter(|keys|keys.len()==frames as usize+1)
            .ok_or_else(||Error("sampled clip requires N+1 real local matrix keys".into()))?;
        let mut values=Vec::with_capacity(keys.len()*9);
        for key in keys {
            let matrix=editable_matrix(key)?;
            if matrix.inverse().is_none(){return Err(Error("sampled local bone matrix is singular".into()))}
            let transform=Transform::from_matrix(matrix);
            let rebuilt=transform.matrix();
            if matrix.0.iter().zip(rebuilt.0).any(|(a,b)|(*a-b).abs()>0.001*(1.+a.abs())){
                return Err(Error("sampled local bone matrix contains shear/perspective outside TRS animation profile".into()))
            }
            values.extend_from_slice(&constant_track(transform,0).values);
        }
        let raw_data=values.iter().flat_map(|v|v.to_le_bytes()).collect::<Vec<_>>();
        Ok(DenseTrack{flags:0,offset:0,bytes:raw_data.len(),stride:36,record_count:keys.len(),
            dense_frame_count_matches:true,raw_data,values})
    }).collect::<Result<Vec<_>>>()?;
    Ok(Clip {
        offset: 0,
        end: 0,
        id: spec
            .get("id")
            .and_then(Value::as_u64)
            .unwrap_or(index as u64),
        name: spec
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("imported")
            .into(),
        mode: spec
            .get("mode")
            .and_then(Value::as_str)
            .unwrap_or("loop")
            .into(),
        fps,
        fps_bits: fps.to_bits(),
        frames,
        flags: 0,
        tracks,
        secondary_tracks: Vec::new(),
        constraint_tracks: Vec::new(),
        scalar_tracks: Vec::new(),
        bone_scalar_group_a: None,
        mesh_channels: None,
        metadata_words: Vec::new(),
        bone_scalar_group_b: None,
        transition: None,
        events: Vec::new(),
    })
}
fn remap_clip_bones(
    mut clip: Clip,
    bones: &[BoneState],
    mapping: &[Option<usize>],
) -> Result<Clip> {
    if clip.frames > 1_000_000 {
        return Err(Error(
            "editable source animation exceeds one million frames".into(),
        ));
    }
    clip.tracks = mapping
        .iter()
        .enumerate()
        .map(|(index, source)| {
            source
                .and_then(|i| clip.tracks.get(i).cloned())
                .unwrap_or_else(|| constant_track(bones[index].bind, clip.frames))
        })
        .collect();
    if let Some(old) = clip.bone_scalar_group_a.take() {
        clip.bone_scalar_group_a = Some(
            mapping
                .iter()
                .map(|source| {
                    source
                        .and_then(|i| old.get(i).cloned())
                        .unwrap_or_else(|| constant_scalar(1., clip.frames))
                })
                .collect(),
        );
    }
    if let Some(old) = clip.bone_scalar_group_b.take() {
        clip.bone_scalar_group_b = Some(
            mapping
                .iter()
                .map(|source| {
                    source
                        .and_then(|i| old.get(i).cloned())
                        .unwrap_or_else(|| constant_scalar(0., clip.frames))
                })
                .collect(),
        );
    }
    if let Some(transition) = &mut clip.transition {
        let source = transition.words[3] as usize;
        if source != u32::MAX as usize {
            transition.words[3] = mapping
                .iter()
                .position(|bone| *bone == Some(source))
                .map(|bone| bone as u32)
                .ok_or_else(|| Error("editable skeleton removed a root-motion bone".into()))?;
        }
    }
    Ok(clip)
}
fn crop_track(track: &mut DenseTrack, start: usize, frames: u32) -> Result<()> {
    let width = track.stride / 4;
    let begin = start
        .checked_mul(width)
        .ok_or_else(|| Error("editable track range overflow".into()))?;
    let count = (frames as usize + 1)
        .checked_mul(width)
        .ok_or_else(|| Error("editable track length overflow".into()))?;
    track.values = track
        .values
        .get(begin..begin + count)
        .ok_or_else(|| Error("editable clip range needs a real sentinel key".into()))?
        .to_vec();
    track.raw_data = track
        .values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect();
    track.record_count = frames as usize + 1;
    track.bytes = track.raw_data.len();
    track.dense_frame_count_matches = true;
    Ok(())
}
fn editable_metadata(
    value: &Value,
    source_bones: &[BoneState],
    source_clips: &[Clip],
) -> Result<EditableMetadata> {
    let mut mapping = (0..source_bones.len()).map(Some).collect::<Vec<_>>();
    let bones = if let Some(rows) = value.get("bones").and_then(Value::as_array) {
        if rows.len() > 128 {
            return Err(Error("editable skeleton exceeds128 bones".into()));
        }
        mapping.clear();
        rows.iter()
            .enumerate()
            .map(|(index, row)| {
                let source = if row.get("sourceBone").is_some_and(Value::is_null) {
                    None
                } else {
                    row.get("sourceBone")
                        .and_then(Value::as_u64)
                        .map(|v| v as usize)
                        .or_else(|| (index < source_bones.len()).then_some(index))
                };
                if source.is_some_and(|i| i >= source_bones.len()) {
                    return Err(Error(
                        "editable sourceBone outside original skeleton".into(),
                    ));
                }
                mapping.push(source);
                let bind = if let Some(matrix) = row.get("transform").filter(|v| !v.is_null()) {
                    let matrix = editable_matrix(matrix)?;
                    if matrix.inverse().is_none() {
                        return Err(Error("editable bind matrix is singular".into()));
                    }
                    Transform::from_matrix(matrix)
                } else {
                    source
                        .and_then(|i| source_bones.get(i))
                        .map(|bone| bone.bind)
                        .unwrap_or_default()
                };
                let parent = row.get("parent").and_then(Value::as_i64).unwrap_or(-1);
                if parent < -1 || parent >= rows.len() as i64 {
                    return Err(Error("editable bone parent outside skeleton".into()));
                }
                let properties = if let Some(text) = row.get("properties").and_then(Value::as_str) {
                    serde_json::from_str(text)
                        .map_err(|e| Error(format!("editable bone properties JSON: {e}")))?
                } else {
                    row.get("properties").cloned().unwrap_or_else(|| {
                        source
                            .and_then(|i| source_bones.get(i))
                            .map(|bone| bone.properties.clone())
                            .unwrap_or(Value::Null)
                    })
                };
                Ok(BoneState {
                    name: row
                        .get("name")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .unwrap_or_else(|| format!("bone-{index}")),
                    parent: usize::try_from(parent).ok(),
                    bind,
                    properties,
                })
            })
            .collect::<Result<Vec<_>>>()?
    } else {
        source_bones.to_vec()
    };
    let bind = bones.iter().map(|bone| bone.bind).collect::<Vec<_>>();
    let inverse_bind = world_matrices(&bones, &bind)?
        .into_iter()
        .map(|matrix| {
            matrix
                .inverse()
                .ok_or_else(|| Error("editable world bind matrix is singular".into()))
        })
        .collect::<Result<Vec<_>>>()?;
    let reference_clips = source_clips
        .iter()
        .cloned()
        .map(|clip| remap_clip_bones(clip, &bones, &mapping))
        .collect::<Result<Vec<_>>>()?;
    let clips = if let Some(specs) = value.get("clips").and_then(Value::as_array) {
        if specs.len() > 100_000 {
            return Err(Error("editable clip limit exceeded".into()));
        }
        specs
            .iter()
            .enumerate()
            .map(|(index, spec)| {
                let source = spec
                    .get("sourceClip")
                    .and_then(Value::as_u64)
                    .map(|n| n as usize)
                    .unwrap_or(index);
                let mut clip = if let Some(matrices) = spec.get("boneMatrices") {
                    editable_matrix_clip(spec, matrices, &bones, index)?
                } else {
                    reference_clips.get(source).cloned().ok_or_else(|| {
                        Error("editable clip requires sourceClip or sampled boneMatrices".into())
                    })?
                };
                let start = spec.get("startFrame").and_then(Value::as_u64).unwrap_or(0) as usize;
                let frames = spec
                    .get("frames")
                    .and_then(Value::as_u64)
                    .and_then(|v| u32::try_from(v).ok())
                    .unwrap_or(clip.frames);
                if frames > 1_000_000
                    || start
                        .checked_add(frames as usize)
                        .is_none_or(|end| end > clip.frames as usize)
                {
                    return Err(Error("editable clip frame range outside source".into()));
                }
                if clip.flags & 0x401 == 1 && start != 0 {
                    return Err(Error(
                        "referenced native clip requires its original startFrame".into(),
                    ));
                }
                if start != 0 || frames != clip.frames {
                    for track in clip
                        .tracks
                        .iter_mut()
                        .chain(&mut clip.secondary_tracks)
                        .chain(&mut clip.constraint_tracks)
                        .chain(&mut clip.scalar_tracks)
                    {
                        crop_track(track, start, frames)?;
                    }
                    for tracks in [&mut clip.bone_scalar_group_a, &mut clip.bone_scalar_group_b]
                        .into_iter()
                        .flatten()
                    {
                        for track in tracks {
                            crop_track(track, start, frames)?;
                        }
                    }
                    if let Some(groups) = &mut clip.mesh_channels {
                        for group in groups {
                            for channel in &mut group.channels {
                                channel.values = channel
                                    .values
                                    .get(start..start + frames as usize + 1)
                                    .ok_or_else(|| {
                                        Error("editable morph clip sentinel missing".into())
                                    })?
                                    .to_vec();
                                channel.raw_data = channel
                                    .values
                                    .iter()
                                    .flat_map(|f| f.to_le_bytes())
                                    .collect();
                                channel.bytes = channel.raw_data.len();
                                channel.record_count = channel.values.len();
                                channel.dense_frame_count_matches = true;
                            }
                        }
                    }
                }
                clip.frames = frames;
                if let Some(fps) = spec.get("fps") {
                    clip.fps = fps
                        .as_f64()
                        .map(|v| v as f32)
                        .filter(|v| v.is_finite() && *v > 0. && *v <= 100_000.)
                        .ok_or_else(|| Error("editable clip fps must be positive finite".into()))?;
                    clip.fps_bits = clip.fps.to_bits();
                }
                if let Some(name) = spec.get("name").and_then(Value::as_str) {
                    clip.name = name.into();
                }
                if let Some(mode) = spec.get("mode").and_then(Value::as_str) {
                    if !["loop", "single", "mirror"]
                        .iter()
                        .any(|known| mode.eq_ignore_ascii_case(known))
                    {
                        return Err(Error("unknown editable animation mode".into()));
                    }
                    clip.mode = mode.into();
                }
                if let Some(id) = spec.get("id").and_then(Value::as_u64) {
                    clip.id = id;
                }
                if let Some(events) = spec.get("events").and_then(Value::as_array) {
                    clip.events = events
                        .iter()
                        .map(|event| {
                            let seconds = event
                                .get("seconds")
                                .and_then(Value::as_f64)
                                .map(|v| v as f32)
                                .or_else(|| {
                                    event
                                        .get("frame")
                                        .and_then(Value::as_f64)
                                        .map(|v| v as f32 / clip.fps)
                                })
                                .filter(|v| v.is_finite())
                                .ok_or_else(|| {
                                    Error("editable event needs finite seconds or frame".into())
                                })?;
                            let text = event
                                .get("text")
                                .and_then(Value::as_str)
                                .map(str::to_owned)
                                .unwrap_or_else(|| event.to_string());
                            Ok(wpe_formats::mdl::AnimationEvent {
                                frame_word: seconds.to_bits(),
                                text,
                            })
                        })
                        .collect::<Result<Vec<_>>>()?;
                }
                Ok(clip)
            })
            .collect::<Result<Vec<_>>>()?
    } else {
        reference_clips.clone()
    };
    let attachments = if let Some(rows) = value.get("attachments").and_then(Value::as_array) {
        Some(
            rows.iter()
                .map(|row| {
                    let name = row
                        .get("name")
                        .and_then(Value::as_str)
                        .ok_or_else(|| Error("editable attachment name required".into()))?;
                    let bone = row
                        .get("bone")
                        .and_then(Value::as_u64)
                        .map(|n| n as usize)
                        .filter(|n| *n < bones.len())
                        .ok_or_else(|| Error("editable attachment bone outside skeleton".into()))?;
                    let matrix = editable_matrix(
                        row.get("transform")
                            .or_else(|| row.get("matrix"))
                            .ok_or_else(|| Error("editable attachment matrix required".into()))?,
                    )?;
                    Ok((name.to_owned(), bone, matrix))
                })
                .collect::<Result<Vec<_>>>()?,
        )
    } else {
        None
    };
    let ik_targets = if let Some(rows) = value.get("constraints").and_then(Value::as_array) {
        Some(rows.iter().map(|row|{
            if row.get("type").and_then(Value::as_str)!=Some("ik"){
                return Err(Error("[native-constraint-table] editable constraints require the explicit owned type=ik schema; native unnamed tables remain preserved".into()))
            }
            let bone=row.get("bone").and_then(Value::as_u64).map(|n|n as usize).filter(|n|*n<bones.len())
                .ok_or_else(||Error("editable IK bone outside skeleton".into()))?;
            let target=Vec3(editable_vector(row.get("target").ok_or_else(||Error("editable IK target required".into()))?,3)?.try_into().unwrap());
            let weight=row.get("weight").and_then(Value::as_f64).unwrap_or(1.)as f32;
            if !weight.is_finite(){return Err(Error("invalid editable IK weight".into()))}
            Ok(IkTarget{bone,target,weight,chain_length:row.get("chainLength").and_then(Value::as_u64).unwrap_or(2).min(128)as usize,iterations:row.get("iterations").and_then(Value::as_u64).unwrap_or(10).min(128)as usize})
        }).collect::<Result<Vec<_>>>()?)
    } else {
        None
    };
    Ok(EditableMetadata {
        bones,
        inverse_bind,
        clips,
        reference_clips,
        attachments,
        ik_targets,
    })
}
fn world_matrices(bones: &[BoneState], local: &[Transform]) -> Result<Vec<Mat4>> {
    fn visit(
        i: usize,
        bones: &[BoneState],
        local: &[Transform],
        output: &mut [Mat4],
        seen: &mut BTreeSet<usize>,
        done: &mut [bool],
    ) -> Result<()> {
        if done[i] {
            return Ok(());
        }
        if !seen.insert(i) {
            return Err(Error("cycle in model skeleton".into()));
        }
        let mut m = local[i].matrix();
        if let Some(parent) = bones[i].parent {
            visit(parent, bones, local, output, seen, done)?;
            m = output[parent] * m;
        }
        output[i] = m;
        done[i] = true;
        seen.remove(&i);
        Ok(())
    }
    let mut out = vec![Mat4::IDENTITY; bones.len()];
    let mut done = vec![false; bones.len()];
    let mut seen = BTreeSet::new();
    for i in 0..bones.len() {
        visit(i, bones, local, &mut out, &mut seen, &mut done)?;
    }
    Ok(out)
}
fn predicted_weight(layer: &AnimationLayer, dt: f32) -> f32 {
    if layer.fade_seconds > 0. {
        let amount = dt.abs() / layer.fade_seconds;
        if (layer.target_weight - layer.weight).abs() <= amount {
            layer.target_weight
        } else {
            layer.weight + (layer.target_weight - layer.weight).signum() * amount
        }
    } else {
        layer.target_weight
    }
}
fn pose_without_scale(
    bones: &[BoneState],
    clip: &Clip,
    sample: (usize, usize, f32),
) -> Result<Vec<Mat4>> {
    let local = bones
        .iter()
        .enumerate()
        .map(|(index, bone)| {
            let mut transform = if let Some(track) = clip.tracks.get(index) {
                sample_transform_slerp(track, sample)?
            } else {
                bone.bind
            };
            transform.scale = Vec3::ONE;
            Ok(transform)
        })
        .collect::<Result<Vec<_>>>()?;
    world_matrices(bones, &local)
}
fn rotation_only(mut matrix: Mat4) -> Mat4 {
    matrix.0[12..15].fill(0.);
    matrix
}
fn upright_yaw(matrix: Mat4) -> Result<f32> {
    let forward = Vec3::new(matrix.0[8], 0., matrix.0[10]);
    if !forward.finite() || forward.length_squared() <= 1e-20 {
        return Err(Error("root-motion upright basis is singular".into()));
    }
    Ok(forward.0[0].atan2(forward.0[2]))
}
/// Static producer225900 extracts selected translation axes and upright yaw,
/// removes them from the animated root, and returns scene-layer motion. MDLA
/// transition words retain their on-disk order: start/end/initial-offset/bone.
fn apply_root_motion(
    bones: &[BoneState],
    clips: &[Clip],
    clip: &Clip,
    pose_clip: &Clip,
    frame: ((usize, usize, f32), f32),
    layer: &mut AnimationLayer,
    pose: &mut Pose,
) -> Result<()> {
    let transition = clip
        .transition
        .as_ref()
        .ok_or_else(|| Error("root-motion clip has no reference descriptor".into()))?;
    let root = transition.words[3] as usize;
    if root == u32::MAX as usize {
        return Ok(());
    }
    if root >= bones.len() {
        return Err(Error("root-motion bone outside skeleton".into()));
    }
    let reference = if clip.flags & 0x400 != 0 {
        pose_clip
    } else {
        clips
            .get(transition.reference as usize)
            .ok_or_else(|| Error("root-motion reference clip outside model".into()))?
    };
    let (start, end) = if clip.flags & 0x400 != 0 {
        (0, clip.frames as usize)
    } else {
        (transition.words[0] as usize, transition.words[1] as usize)
    };
    let reference_start = pose_without_scale(bones, reference, (start, start, 0.))?[root];
    let reference_end = pose_without_scale(bones, reference, (end, end, 0.))?[root];
    let current = pose_without_scale(bones, pose_clip, frame.0)?[root];
    let inverse_reference = rotation_only(reference_start)
        .inverse()
        .ok_or_else(|| Error("root-motion reference matrix is singular".into()))?;
    let current_yaw = upright_yaw(rotation_only(current) * inverse_reference)?;
    let current_basis = Mat4::trs(
        Vec3::ZERO,
        Quat::axis_angle(Vec3::new(0., 1., 0.), current_yaw),
        Vec3::ONE,
    );
    let initial_frame = if clip.flags & 0x400 != 0 {
        0
    } else {
        start
            .checked_add(transition.words[2] as usize)
            .ok_or_else(|| Error("root-motion initial key overflow".into()))?
    };
    let initial = if transition.words[2] == 0 {
        reference_start
    } else {
        pose_without_scale(bones, reference, (initial_frame, initial_frame, 0.))?[root]
    };
    let (previous, previous_time) = layer.previous_root_motion.unwrap_or((initial, layer.time));
    let mut previous_rotation = rotation_only(previous);
    let mut wrap_translation = Vec3::ZERO;
    if previous_time > layer.time {
        wrap_translation = reference_end.translation() - reference_start.translation();
        previous_rotation = previous_rotation
            * rotation_only(reference_end)
                .inverse()
                .ok_or_else(|| Error("root-motion end matrix is singular".into()))?
            * rotation_only(reference_start);
    }
    let previous_yaw = upright_yaw(previous_rotation * inverse_reference)?;
    let previous_basis = Mat4::trs(
        Vec3::ZERO,
        Quat::axis_angle(Vec3::new(0., 1., 0.), previous_yaw),
        Vec3::ONE,
    );
    let pivot = -reference_start.translation();
    let delta = wrap_translation + current.translation() - previous.translation()
        + previous_basis.vector(pivot)
        - current_basis.vector(pivot);
    let correction = current_basis
        .inverse()
        .ok_or_else(|| Error("root-motion yaw matrix is singular".into()))?;
    let delta = correction.vector(delta);
    let mut selected = Vec3::ZERO;
    for (axis, flag) in [0x800, 0x1000, 0x2000].iter().enumerate() {
        if clip.flags & flag != 0 {
            selected.0[axis] = delta.0[axis];
            pose.local[root].translation.0[axis] += (reference_start.translation().0[axis]
                - current.translation().0[axis])
                * layer.weight;
        }
    }
    pose.root_motion.translation += selected * (layer.weight * frame.1);
    if clip.flags & 0x8000 != 0 {
        let rotation = Quat::axis_angle(Vec3::new(0., 1., 0.), -current_yaw);
        pose.local[root].rotation = rotation * pose.local[root].rotation;
        if clip.flags & 0xa800 == 0x8000 {
            pose.local[root].translation = correction.vector(pose.local[root].translation);
        }
        let yaw = (current_yaw - previous_yaw + std::f32::consts::PI)
            .rem_euclid(std::f32::consts::TAU)
            - std::f32::consts::PI;
        pose.root_motion.yaw += yaw * layer.weight * frame.1;
    }
    layer.previous_root_motion = Some((current, layer.time));
    if !pose.root_motion.translation.finite() || !pose.root_motion.yaw.is_finite() {
        return Err(Error("root-motion result is nonfinite".into()));
    }
    Ok(())
}
#[derive(Clone, Copy, Debug)]
struct ClockAdvance {
    old: f32,
    raw: f32,
    time: f32,
    duration: f32,
    mode: PlaybackMode,
    active: bool,
}
impl ClockAdvance {
    fn crossed(self, at: f32, phase: usize) -> bool {
        if !self.active || !at.is_finite() {
            return false;
        }
        if phase == 0 {
            if self.raw > self.old {
                self.old <= at && at < self.raw
            } else {
                self.raw < at && at <= self.old
            }
        } else if self.mode == PlaybackMode::Loop {
            if self.raw < 0. && self.time >= 0. {
                self.time < at && at <= self.duration
            } else if self.raw >= self.duration && self.time < self.duration {
                0. <= at && at < self.time
            } else {
                false
            }
        } else {
            false
        }
    }
}
/// The native clock makes one signed fmod/boundary transition per update. In
/// particular mirror playback retains its reverse bit between updates, and a
/// large delta is not expanded into an invented sequence of intermediate ticks.
fn advance_layer_clock(layer: &mut AnimationLayer, dt: f32, duration: f32) -> Result<ClockAdvance> {
    if !duration.is_finite()
        || duration <= 0.
        || !layer.time.is_finite()
        || !layer.rate.is_finite()
        || !layer.direction.is_finite()
        || !layer.weight.is_finite()
        || !layer.target_weight.is_finite()
        || !layer.fade_seconds.is_finite()
    {
        return Err(Error("invalid animation clock state".into()));
    }
    let old = if layer.external_clock {
        layer.external_previous_time.unwrap_or(layer.time)
    } else {
        layer.time
    };
    let active = layer.playing && !(layer.mode == PlaybackMode::Once && old >= duration);
    if layer.external_clock {
        let time = layer.time;
        let mut raw = time;
        if active && dt != 0. {
            if layer.mode == PlaybackMode::Loop {
                let forward = dt * layer.rate * if layer.direction < 0. { -1. } else { 1. } >= 0.;
                if forward && time < old {
                    raw += duration;
                } else if !forward && time > old {
                    raw -= duration;
                }
            } else if layer.mode == PlaybackMode::PingPong {
                if layer.direction >= 0. && time < old {
                    raw = duration + (duration - time);
                    layer.direction = -1.;
                } else if layer.direction < 0. && time > old {
                    raw = -time;
                    layer.direction = 1.;
                }
            }
        }
        layer.external_previous_time = Some(time);
        return Ok(ClockAdvance {
            old,
            raw,
            time,
            duration,
            mode: layer.mode,
            active,
        });
    }
    let delta = if active {
        dt * layer.rate * if layer.direction < 0. { -1. } else { 1. }
    } else {
        0.
    };
    let raw = old + delta;
    if !raw.is_finite() {
        return Err(Error("animation clock overflow".into()));
    }
    if active {
        layer.time = match layer.mode {
            PlaybackMode::Loop => {
                let mut time = raw;
                if time < 0. {
                    time = (time + duration) % duration;
                }
                if time >= duration {
                    time %= duration;
                }
                time
            }
            PlaybackMode::Once => {
                if raw >= duration {
                    layer.playing = false;
                    duration
                } else {
                    raw
                }
            }
            PlaybackMode::PingPong => {
                if layer.direction >= 0. && raw >= duration {
                    layer.direction = -1.;
                    duration - raw % duration
                } else if layer.direction < 0. && raw <= 0. {
                    layer.direction = 1.;
                    -(raw % duration)
                } else {
                    raw
                }
            }
        };
    }
    Ok(ClockAdvance {
        old,
        raw,
        time: layer.time,
        duration,
        mode: layer.mode,
        active,
    })
}
pub fn frame_sample(time: f32, fps: f32, count: u32) -> (usize, usize, f32) {
    let duration = 1. / fps;
    let first =
        ((time / duration).trunc() as i64).clamp(0, count.saturating_sub(1) as i64) as usize;
    let next = (first + 1).min(count as usize);
    (first, next, (time % duration) / duration)
}
fn nlerp_shortest(a: Quat, mut b: Quat, t: f32) -> Quat {
    if a.dot(b) < 0. {
        b = Quat(b.0.map(|v| -v));
    }
    Quat(std::array::from_fn(|i| a.0[i] * (1. - t) + b.0[i] * t)).normalize()
}
fn sample_transform(track: &DenseTrack, sample: (usize, usize, f32)) -> Result<Transform> {
    sample_transform_with(track, sample, false)
}
fn sample_transform_slerp(track: &DenseTrack, sample: (usize, usize, f32)) -> Result<Transform> {
    sample_transform_with(track, sample, true)
}
fn sample_transform_with(
    track: &DenseTrack,
    sample: (usize, usize, f32),
    slerp: bool,
) -> Result<Transform> {
    if track.stride != 36 || track.values.len() < 9 {
        return Err(Error("unsupported model transform track layout".into()));
    }
    let decode = |index: usize| -> Result<Transform> {
        let base = index
            .checked_mul(9)
            .ok_or_else(|| Error("model track key offset overflow".into()))?;
        let f = track
            .values
            .get(base..base + 9)
            .filter(|_| index < track.record_count)
            .ok_or_else(|| Error("model transform sentinel key missing".into()))?;
        if f.iter().any(|v| !v.is_finite()) {
            return Err(Error("nonfinite model transform key".into()));
        }
        Ok(Transform {
            translation: Vec3::new(f[0], f[1], f[2]),
            rotation: Quat::euler_xyz(Vec3::new(f[3], f[4], f[5])),
            scale: Vec3::new(f[6], f[7], f[8]),
        })
    };
    let a = decode(sample.0)?;
    let b = decode(sample.1)?;
    let mut transform = a.blend(b, sample.2);
    if slerp {
        transform.rotation = a.rotation.slerp(b.rotation, sample.2);
    }
    Ok(transform)
}
fn sample_scalar(track: &DenseTrack, sample: (usize, usize, f32)) -> Result<f32> {
    if track.stride != 4 {
        return Err(Error("unsupported scalar pose track layout".into()));
    }
    let a = track.values.get(sample.0).copied();
    let b = track.values.get(sample.1).copied();
    let (Some(a), Some(b)) = (a, b) else {
        return Err(Error("scalar pose sentinel key missing".into()));
    };
    let value = (1. - sample.2) * a + sample.2 * b;
    if !value.is_finite() {
        return Err(Error("nonfinite scalar pose key".into()));
    }
    Ok(value)
}
fn apply_limits(pose: &mut Transform, properties: &Value) -> Result<()> {
    if properties.get("lt").and_then(Value::as_bool) == Some(true) {
        let limit = crate::particle::number(properties, "ltmax", 0.);
        if !limit.is_finite() || limit < 0. {
            return Err(Error("invalid quaternion-ratio limit".into()));
        }
        let q = pose.rotation;
        let mut v = q.0;
        for i in 0..3 {
            let ratio = if v[i + 1] == 0. && v[0] == 0. {
                0.
            } else {
                v[i + 1] / v[0]
            };
            let angle = 2. * ratio.atan();
            v[i + 1] = (angle.clamp(-limit.to_radians(), limit.to_radians()) * 0.5).tan();
        }
        v[0] = 1.;
        pose.rotation = Quat(v).normalize();
    }
    if properties.get("la").and_then(Value::as_bool) == Some(true)
        && properties.get("lamin").is_some_and(|v| !v.is_null())
        && properties.get("lamax").is_some_and(|v| !v.is_null())
    {
        // These stored compact property vectors are already radians. ltmax
        // above is the distinct scalar that native code converts from degrees.
        let min = crate::particle::vector(properties, "lamin", Vec3::ZERO);
        let max = crate::particle::vector(properties, "lamax", Vec3::ZERO);
        if !min.finite() || !max.finite() || (0..3).any(|i| min.0[i] > max.0[i]) {
            return Err(Error("invalid angular minimum/maximum limits".into()));
        }
        let q = pose.rotation;
        let [w, x, y, z] = q.0;
        let angles = Vec3::new(
            (2. * (w * x + y * z)).atan2(1. - 2. * (x * x + y * y)),
            (2. * (w * y - z * x)).clamp(-1., 1.).asin(),
            (2. * (w * z + x * y)).atan2(1. - 2. * (y * y + z * z)),
        );
        let clamped = Vec3(std::array::from_fn(|i| {
            angles.0[i].clamp(min.0[i], max.0[i])
        }));
        pose.rotation = Quat::euler_xyz(clamped);
    }
    Ok(())
}
fn align(a: Vec3, b: Vec3) -> Quat {
    let a = a.normalize();
    let b = b.normalize();
    let dot = a.dot(b);
    if dot < -0.999999 {
        let axis = if a.0[0].abs() < 0.9 {
            a.cross(Vec3::new(1., 0., 0.))
        } else {
            a.cross(Vec3::new(0., 1., 0.))
        };
        Quat::axis_angle(axis, std::f32::consts::PI)
    } else {
        let c = a.cross(b);
        Quat([1. + dot, c.0[0], c.0[1], c.0[2]]).normalize()
    }
}
fn solve_ik(
    bones: &[BoneState],
    local: &mut [Transform],
    world: &mut Vec<Mat4>,
    target: &IkTarget,
) -> Result<()> {
    if target.bone >= bones.len() {
        return Err(Error("IK effector outside skeleton".into()));
    }
    for _ in 0..target.iterations.min(128) {
        if (world[target.bone].translation() - target.target).length() < 0.001 {
            break;
        }
        let mut node = bones[target.bone].parent;
        for _ in 0..target.chain_length.min(bones.len()) {
            let Some(i) = node else { break };
            let pivot = world[i].translation();
            let rotation = align(
                world[target.bone].translation() - pivot,
                target.target - pivot,
            );
            let parent = bones[i]
                .parent
                .and_then(|p| world[p].inverse())
                .unwrap_or_default();
            let world_rotation = Transform::from_matrix(world[i]).rotation;
            let parent_rotation = Transform::from_matrix(parent).rotation;
            let delta = parent_rotation * rotation * parent_rotation.conjugate();
            local[i].rotation = local[i]
                .rotation
                .slerp(delta * local[i].rotation, target.weight.clamp(0., 1.));
            let _ = world_rotation;
            apply_limits(&mut local[i], &bones[i].properties)?;
            *world = world_matrices(bones, local)?;
            node = bones[i].parent;
        }
    }
    Ok(())
}
fn texture_channels(mesh: &Mesh, count: usize) -> Result<Option<TextureChannels>> {
    if mesh.flags & 2 == 0 {
        return Ok(None);
    }
    if mesh.vertex_format != 0x800021 || mesh.vertex_stride != 44 || count != mesh.vertex_count {
        return Err(Error("puppet texture-channel geometry requires the native0x800021/44-byte stream with unchanged vertex count".into()));
    }
    let row_count = mesh
        .extra_flags
        .and_then(|n| u8::try_from(n).ok())
        .filter(|n| (1..=4).contains(n))
        .ok_or_else(|| Error("puppet texture-channel blend rows must be1..4".into()))?;
    let mut tex_coords = Vec::with_capacity(count);
    let mut blend_indices = Vec::with_capacity(count);
    for row in mesh.vertices.chunks_exact(44) {
        let uv = std::array::from_fn(|i| {
            f32::from_le_bytes(row[28 + i * 4..32 + i * 4].try_into().unwrap())
        });
        let indices = std::array::from_fn(|i| {
            u32::from_le_bytes(row[12 + i * 4..16 + i * 4].try_into().unwrap())
        });
        if uv.iter().any(|v| !v.is_finite()) || indices[0] >= u32::from(row_count) * 4 {
            return Err(Error(
                "invalid puppet texture-channel coordinate/index".into(),
            ));
        }
        tex_coords.push(uv);
        blend_indices.push(indices);
    }
    if tex_coords.len() != count {
        return Err(Error("truncated puppet texture-channel stream".into()));
    }
    Ok(Some(TextureChannels {
        row_count,
        tex_coords,
        blend_indices,
    }))
}
fn read_vertex(mesh: &Mesh, index: usize) -> Result<ModelVertex> {
    let bytes = &mesh.vertices[index * mesh.vertex_stride..(index + 1) * mesh.vertex_stride];
    if mesh.vertex_format == 0x180000f {
        let word = |offset| f32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
        return Ok(ModelVertex {
            position: std::array::from_fn(|i| word(i * 4)),
            normal: std::array::from_fn(|i| word(12 + i * 4)),
            tangent: std::array::from_fn(|i| word(24 + i * 4)),
            uv: [word(72), word(76)],
            color: [1.; 4],
            skin_alpha: 1.,
        });
    }
    let mut offset = 0;
    let mut position = [0.; 3];
    let mut normal = [0., 0., 1.];
    let mut tangent = [1., 0., 0., 1.];
    let mut uv = [0.; 2];
    let mut color = [1.; 4];
    let widths = [
        12, 12, 16, 8, 12, 16, 8, 12, 16, 8, 12, 16, 8, 12, 16, 16, 16, 8, 12, 16, 8, 12, 16, 16,
        16, 12,
    ];
    for (bit, width) in widths.iter().enumerate() {
        if mesh.vertex_format & (1 << bit) == 0 {
            continue;
        }
        if offset + width > bytes.len() {
            return Err(Error("mesh attribute exceeds stride".into()));
        }
        let data = &bytes[offset..offset + width];
        let word = |i: usize| f32::from_le_bytes(data[i * 4..i * 4 + 4].try_into().unwrap());
        match bit {
            0 => position = std::array::from_fn(word),
            1 => normal = std::array::from_fn(word),
            2 => tangent = std::array::from_fn(word),
            3 => uv = [word(0), word(1)],
            4 => color = [word(0), word(1), word(2), 1.],
            _ => {}
        }
        offset += width;
    }
    Ok(ModelVertex {
        position,
        normal,
        tangent,
        uv,
        color,
        skin_alpha: 1.,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wpe_formats::mdl::{AnimationEvent, MeshChannel, MeshChannels, Section, SkinVertex};

    #[test]
    fn sampled_editable_matrices_execute_real_skin_motion_and_reject_missing_sentinel() {
        use serde_json::json;
        let mut value = json!({"profile":"wpe-editable-model-v1","bones":[{"name":"root","sourceBone":null,"parent":-1,"transform":Mat4::IDENTITY.0}],
            "shapes":[{"vertexFormat":["position"],"vertexBuffer":[0,0,0,1,0,0,0,1,0],"indexBuffer":[0,1,2],"weights":[[1],[1],[1]]}],
            "clips":[{"name":"move","fps":1,"frames":2,"mode":"single","boneMatrices":[[Mat4::IDENTITY.0,Mat4::trs(Vec3::new(2.,0.,0.),Quat::IDENTITY,Vec3::ONE).0,Mat4::trs(Vec3::new(4.,0.,0.),Quat::IDENTITY,Vec3::ONE).0]],"events":[{"frame":1,"text":"middle"}]}]});
        let mut animator = ModelAnimator::from_editable(&value).unwrap();
        animator.root_motion_enabled = false;
        animator.step(0.5).unwrap();
        let mesh = animator.skinned_meshes().unwrap().remove(0);
        assert!((mesh.vertices[0].position[0] - 1.).abs() < 1e-5);
        animator.step(0.6).unwrap();
        assert_eq!(animator.pose.events[0].payload, "middle");
        value["clips"][0]["boneMatrices"][0]
            .as_array_mut()
            .unwrap()
            .pop();
        assert!(
            ModelAnimator::from_editable(&value)
                .err()
                .unwrap()
                .to_string()
                .contains("N+1")
        );
    }
    #[test]
    fn scalar_blend_map_and_auxiliary_texture_channel_stream_execute() {
        let mut model = owned_skinned_model();
        if let SectionData::Animation(clips) = &mut model
            .sections
            .iter_mut()
            .find(|s| matches!(s.data, SectionData::Animation(_)))
            .unwrap()
            .data
        {
            clips[0].scalar_tracks = vec![constant_scalar(0.75, clips[0].frames)];
        }
        let mut channel = model.meshes[0].clone();
        channel.flags = 2;
        channel.extra_flags = Some(1);
        channel.vertex_format = 0x800021;
        channel.vertex_stride = 44;
        channel.skin = None;
        channel.vertices.clear();
        for i in 0..channel.vertex_count {
            for value in [i as f32, 0., 0.] {
                channel.vertices.extend(value.to_le_bytes())
            }
            for value in [3_u32, 2, 1, 0] {
                channel.vertices.extend(value.to_le_bytes())
            }
            for value in [0.25_f32, 0.5, 0.75, 1.] {
                channel.vertices.extend(value.to_le_bytes())
            }
        }
        channel.materials = vec!["materials/channelmap.json".into()];
        model.meshes.push(channel);
        let mut animator = ModelAnimator::new(model).unwrap();
        animator.step(0.1).unwrap();
        assert_eq!(animator.pose.blend_map[0], 0.75);
        assert!(!animator.issues.iter().any(|s| s.contains("secondary-pose")));
        let channels = animator
            .skinned_meshes()
            .unwrap()
            .remove(1)
            .texture_channels
            .unwrap();
        assert_eq!(channels.row_count, 1);
        assert_eq!(channels.tex_coords[0], [0.25, 0.5, 0.75, 1.]);
        assert_eq!(channels.blend_indices[0], [3, 2, 1, 0]);
    }

    fn morph_model(target_count: u16) -> Model {
        let mut bytes = b"MDLV0023\0".to_vec();
        for word in [3_u32, 0, 1, 0x400] {
            bytes.extend(word.to_le_bytes());
        }
        for value in [0_f32, 0., 0., 1., 1., 1.] {
            bytes.extend(value.to_le_bytes());
        }
        bytes.extend(3_u32.to_le_bytes());
        bytes.extend(24_u32.to_le_bytes());
        for value in [0_f32, 0., 0., 0., 0., 1.] {
            bytes.extend(value.to_le_bytes());
        }
        bytes.extend(0_u32.to_le_bytes());
        bytes.extend([0, 0]);
        bytes.extend(0_u32.to_le_bytes());
        bytes.extend(b"MDMP0001\0");
        let end_at = bytes.len();
        bytes.extend(0_u32.to_le_bytes());
        bytes.extend(target_count.to_le_bytes());
        bytes.extend(0.5_f32.to_le_bytes());
        bytes.extend(1_u32.to_le_bytes());
        for index in 0..target_count {
            bytes.extend((9000_u64 + u64::from(index)).to_le_bytes());
            bytes.extend(format!("target-{index}\0").bytes());
            for _ in 0..2 {
                bytes.extend(6_u32.to_le_bytes());
                for value in [1_f32, 0., 0.] {
                    bytes.extend(half::f16::from_f32(value).to_bits().to_le_bytes());
                }
            }
        }
        let end = bytes.len() as u32;
        bytes[end_at..end_at + 4].copy_from_slice(&end.to_le_bytes());
        bytes.push(0);
        Model::parse(&bytes).unwrap()
    }
    fn clip() -> Clip {
        Clip {
            offset: 0,
            end: 0,
            id: 3,
            name: "clip".into(),
            mode: "loop".into(),
            fps: 2.,
            fps_bits: 2_f32.to_bits(),
            frames: 2,
            flags: 0,
            tracks: Vec::new(),
            secondary_tracks: Vec::new(),
            constraint_tracks: Vec::new(),
            scalar_tracks: Vec::new(),
            bone_scalar_group_a: None,
            mesh_channels: None,
            metadata_words: Vec::new(),
            bone_scalar_group_b: None,
            transition: None,
            events: Vec::new(),
        }
    }
    fn owned_skinned_model() -> Model {
        Model::parse(include_bytes!(
            "../../wpe-editor/tests/fixtures/editable-skinned-triangle.mdl"
        ))
        .unwrap()
    }
    #[test]
    fn actual_puppet_uv_alpha_discrete_depth_and_pose_channels_execute() {
        let mut animator = ModelAnimator::new(owned_skinned_model()).unwrap();
        animator.step(0.25).unwrap();
        assert_eq!(animator.pose.local[0].translation, Vec3::new(0.5, 0., 0.));
        assert_eq!(animator.pose.bone_alpha, [0.75]);
        assert_eq!(animator.pose.draw_depth, [0.]);
        let vertex = &animator.skinned_meshes().unwrap()[0].vertices[0];
        assert_eq!(vertex.position, [-4.5, -5., 0.]);
        assert_eq!(vertex.uv, [0., 1.]);
        assert_eq!(vertex.color[3], 0.75);
        animator.step(0.25).unwrap();
        assert_eq!(animator.pose.bone_alpha, [0.5]);
        assert_eq!(animator.pose.draw_depth, [1.]);
    }
    #[test]
    fn root_translation_is_extracted_removed_and_compensated_across_loop() {
        let mut model = owned_skinned_model();
        for section in &mut model.sections {
            if let SectionData::Animation(clips) = &mut section.data {
                clips[0].flags = 0xc01;
                clips[0].transition = Some(wpe_formats::mdl::Transition {
                    reference: 0,
                    words: [0, 2, 0, 0],
                });
            }
        }
        let mut animator = ModelAnimator::new(model).unwrap();
        animator.step(0.25).unwrap();
        assert_eq!(animator.pose.local[0].translation, Vec3::ZERO);
        assert_eq!(
            animator.pose.root_motion.translation,
            Vec3::new(0.5, 0., 0.)
        );
        animator.step(0.25).unwrap();
        assert_eq!(
            animator.pose.root_motion.translation,
            Vec3::new(0.5, 0., 0.)
        );
        animator.step(0.6).unwrap();
        assert!((animator.pose.root_motion.translation.0[0] - 1.2).abs() < 1e-5);
        assert!(animator.pose.local[0].translation.length() < 1e-5);
        animator.root_motion_enabled = false;
        animator.step(0.1).unwrap();
        assert_eq!(animator.pose.root_motion.translation, Vec3::ZERO);
        assert!(animator.pose.local[0].translation.0[0] > 0.);
    }
    #[test]
    fn checked_editable_overlay_changes_geometry_and_retains_source_model() {
        let model = owned_skinned_model();
        let raw = model.meshes[0].vertices.clone();
        let mut animator = ModelAnimator::new(model).unwrap();
        animator.layers.clear();
        let value = serde_json::json!({"shapes":[{"sourceMesh":0,"vertexFormat":["position","uv"],
            "vertexBuffer":[-3,-5,0,0,1,7,-5,0,1,1,2,5,0,0.5,0],"indexBuffer":[0,1,2],
            "weights":[[1],[1],[1]]}],"boneDrawDepth":[2]});
        animator.apply_editable_shapes(&value).unwrap();
        animator.step(0.).unwrap();
        assert_eq!(
            animator.skinned_meshes().unwrap()[0].vertices[0].position,
            [-3., -5., 0.]
        );
        assert_eq!(animator.model.meshes[0].vertices, raw);
        assert_eq!(animator.clips.len(), 1);
        let mut bad = value;
        bad["shapes"][0]["indexBuffer"] = serde_json::json!([0, 1, 300]);
        assert!(animator.apply_editable_shapes(&bad).is_err());
        // Failed updates are atomic and retain the valid previous overlay.
        assert_eq!(
            animator.skinned_meshes().unwrap()[0].vertices[0].position,
            [-3., -5., 0.]
        );
    }
    #[test]
    fn owned_draw_order_uses_dominant_bones_without_renumbering_skin() {
        let mut model = owned_skinned_model();
        if let SectionData::Skeleton(s) = &mut model.sections[0].data {
            let mut bone = s.bones[0].clone();
            bone.name = "second".into();
            s.bones.push(bone);
        }
        let mesh = &mut model.meshes[0];
        mesh.vertices.extend_from_within(..);
        mesh.vertex_count = 6;
        mesh.vertex_bytes = mesh.vertices.len();
        mesh.indices = vec![0, 1, 2, 3, 4, 5];
        mesh.index_count = 6;
        mesh.index_bytes = 12;
        let weights = mesh.skin.as_mut().unwrap();
        weights.extend((0..3).map(|_| SkinVertex {
            bone_indices: [1, 0, 0, 0],
            weights: [1., 0., 0., 0.],
        }));
        let mut animator = ModelAnimator::new(model).unwrap();
        animator.layers.clear();
        animator.step(0.).unwrap();
        animator.draw_depth_overrides.insert(0, 2.);
        assert_eq!(
            animator.skinned_meshes().unwrap()[0].indices,
            [3, 4, 5, 0, 1, 2]
        );
        assert_eq!(
            animator.model.meshes[0].skin.as_ref().unwrap()[3].bone_indices[0],
            1
        );
    }
    #[test]
    fn compact_limits_keep_radian_ranges_and_scalar_degree_ratio_limit_distinct() {
        let mut transform = Transform {
            rotation: Quat::euler_xyz(Vec3::new(0.8, 0., 0.)),
            ..Default::default()
        };
        apply_limits(
            &mut transform,
            &serde_json::json!({"la":true,"lamin":"-0.5 -1 -1","lamax":"0.5 1 1"}),
        )
        .unwrap();
        assert!((transform.rotation.0[1] - 0.25_f32.sin()).abs() < 1e-5);
        apply_limits(&mut transform, &serde_json::json!({"lt":true,"ltmax":10})).unwrap();
        assert!((transform.rotation.0[1] - 5_f32.to_radians().sin()).abs() < 1e-5);
        assert!(
            apply_limits(
                &mut transform,
                &serde_json::json!({"la":true,"lamin":"2 0 0","lamax":"1 0 0"})
            )
            .is_err()
        );
    }
    fn editable_shape() -> Value {
        serde_json::json!({"shapes":[{"sourceMesh":0,"vertexFormat":["position","uv"],
            "vertexBuffer":[-5,-5,0,0,1,5,-5,0,1,1,0,5,0,0.5,0],"indexBuffer":[0,1,2],"weights":[[1],[1],[1]]}]})
    }
    #[test]
    fn editable_bone_clip_event_attachment_metadata_replays_and_cycles_fail() {
        let mut animator = ModelAnimator::new(owned_skinned_model()).unwrap();
        let mut editable = editable_shape();
        editable["bones"] = serde_json::json!([{"sourceBone":0,"name":"edited-root","parent":-1,"transform":Mat4::IDENTITY.0,"properties":{"custom":4}}]);
        editable["clips"] = serde_json::json!([{"sourceClip":0,"name":"edited-clip","fps":4,"frames":2,"mode":"loop","events":[{"frame":1,"name":"edited-event"}]}]);
        let mut attachment = Mat4::IDENTITY;
        attachment.0[12] = 3.;
        editable["attachments"] =
            serde_json::json!([{"name":"socket","bone":0,"transform":attachment.0}]);
        animator.apply_editable_shapes(&editable).unwrap();
        assert_eq!(animator.bones[0].name, "edited-root");
        assert_eq!(animator.bones[0].properties["custom"], 4);
        assert_eq!(animator.clips[0].name, "edited-clip");
        animator.step(0.3).unwrap();
        assert_eq!(animator.pose.events[0].payload["name"], "edited-event");
        assert_eq!(animator.pose.events[0].frame, 1.);
        assert!((animator.attachment("socket").unwrap().translation().0[0] - 4.2).abs() < 1e-5);
        let mut bad = editable.clone();
        bad["bones"][0]["parent"] = serde_json::json!(0);
        assert!(animator.apply_editable_shapes(&bad).is_err());
        assert_eq!(animator.bones[0].parent, None);
        let mut bad = editable;
        bad["constraints"] = serde_json::json!([{"bone":0,"float_words":[1,2]}]);
        assert!(
            animator
                .apply_editable_shapes(&bad)
                .unwrap_err()
                .to_string()
                .contains("native-constraint-table")
        );
    }
    #[test]
    fn unchanged_editable_document_does_not_reset_pose_or_spring_state() {
        let mut animator = ModelAnimator::new(owned_skinned_model()).unwrap();
        let editable = editable_shape();
        animator.apply_editable_shapes(&editable).unwrap();
        animator.step(0.25).unwrap();
        animator.spring_velocities[0] = Vec3::new(12., 0., 0.);
        let position = animator.pose.local[0].translation;
        animator.apply_editable_shapes(&editable).unwrap();
        assert_eq!(animator.pose.local[0].translation, position);
        assert_eq!(animator.spring_velocities[0], Vec3::new(12., 0., 0.));
    }
    #[test]
    fn external_clock_keeps_playing_blend_attenuation_and_does_not_double_tick() {
        let mut model = owned_skinned_model();
        for section in &mut model.sections {
            if let SectionData::Animation(clips) = &mut section.data {
                let other = clips[0].clone();
                clips[0].flags = 0xc01;
                clips[0].transition = Some(wpe_formats::mdl::Transition {
                    reference: 0,
                    words: [0, 2, 0, 0],
                });
                clips.push(other);
            }
        }
        let mut animator = ModelAnimator::new(model).unwrap();
        let mut other = AnimationLayer::new(1);
        other.weight = 0.5;
        other.target_weight = 0.5;
        animator.layers.push(other);
        for layer in &mut animator.layers {
            layer.set_external_time(0.25).unwrap();
        }
        animator.step(0.25).unwrap();
        assert_eq!(animator.layers[0].time, 0.25);
        assert_eq!(
            animator.pose.root_motion.translation,
            Vec3::new(0.25, 0., 0.)
        );
        for layer in &mut animator.layers {
            layer.set_external_time(0.5).unwrap();
        }
        // A paused visible pose still attenuates root motion; native d0 bit0
        // is visibility, distinct from the clock pause bits at104.
        animator.layers[1].playing = false;
        animator.step(0.25).unwrap();
        assert_eq!(animator.layers[0].time, 0.5);
        assert_eq!(
            animator.pose.root_motion.translation,
            Vec3::new(0.25, 0., 0.)
        );
        animator.layers[1].visible = false;
        animator.layers[0].set_external_time(0.75).unwrap();
        animator.step(0.25).unwrap();
        assert_eq!(
            animator.pose.root_motion.translation,
            Vec3::new(0.5, 0., 0.)
        );
    }
    #[test]
    fn material_skin_variants_clamp_and_edited_materials_override() {
        let mut model = owned_skinned_model();
        model.meshes[0].materials = vec!["first".into(), "second".into()];
        let mut animator = ModelAnimator::new(model).unwrap();
        animator.material_variant = 100;
        assert_eq!(animator.skinned_meshes().unwrap()[0].material, "second");
        let mut editable = editable_shape();
        editable["shapes"][0]["material"] = serde_json::json!("first");
        animator.apply_editable_shapes(&editable).unwrap();
        assert_eq!(animator.skinned_meshes().unwrap()[0].material, "second");
        editable["shapes"][0]["material"] = serde_json::json!("edited");
        animator.apply_editable_shapes(&editable).unwrap();
        assert_eq!(animator.skinned_meshes().unwrap()[0].material, "edited");
    }
    #[test]
    fn root_yaw_is_extracted_and_removed_from_the_animated_root() {
        let mut model = owned_skinned_model();
        for section in &mut model.sections {
            if let SectionData::Animation(clips) = &mut section.data {
                let clip = &mut clips[0];
                clip.flags = 0x8401;
                clip.transition = Some(wpe_formats::mdl::Transition {
                    reference: 0,
                    words: [0, 2, 0, 0],
                });
                for (index, key) in clip.tracks[0].values.chunks_mut(9).enumerate() {
                    key[0] = 0.;
                    key[4] = index as f32 * std::f32::consts::FRAC_PI_2;
                }
            }
        }
        let mut animator = ModelAnimator::new(model).unwrap();
        animator.step(0.25).unwrap();
        assert!((animator.pose.root_motion.yaw - std::f32::consts::FRAC_PI_4).abs() < 1e-5);
        assert!(animator.pose.local[0].rotation.0[0].abs() > 0.99999);
        assert_eq!(animator.pose.root_motion.translation, Vec3::ZERO);
    }
    #[test]
    fn half_morph_position_normal_then_skin_and_explicit_ordinal() {
        let mut model = morph_model(1);
        model.meshes[0].skin = Some(vec![SkinVertex {
            bone_indices: [0; 4],
            weights: [1., 0., 0., 0.],
        }]);
        let mut animator = ModelAnimator::new(model).unwrap();
        animator.pose.skin = vec![Mat4::trs(Vec3::new(2., 3., 4.), Quat::IDENTITY, Vec3::ONE)];
        assert_eq!(animator.morph_names, ["target-0"]);
        animator.morph_weights.insert(0, 0.5);
        let vertex = &animator.skinned_meshes().unwrap()[0].vertices[0];
        assert_eq!(vertex.position, [2.25, 3., 4.]);
        let normal = Vec3::new(0.5 * 3.465, 0., 1.).normalize();
        for (actual, expected) in vertex.normal.iter().zip(normal.0) {
            assert!((actual - expected).abs() < 1e-6);
        }
    }
    #[test]
    fn native_first_eleven_active_targets_then_stable_weight_sort() {
        let mut animator = ModelAnimator::new(morph_model(12)).unwrap();
        for ordinal in 0..12 {
            animator.morph_weights.insert(ordinal, 1.);
        }
        animator.morph_weights.insert(11, 1000.);
        animator.morph_weights.insert(3, 2.);
        let active = animator.active_morph_targets(0).unwrap();
        assert_eq!(active.len(), 11);
        assert_eq!(active[0], (3, 2.));
        assert_eq!(active[1], (0, 1.));
        assert_eq!(active.last(), Some(&(10, 1.)));
        assert!(!active.iter().any(|(index, _)| *index == 11));
        animator.morph_weights.insert(0, f32::NAN);
        assert!(animator.skinned_meshes().is_err());
    }
    #[test]
    fn animation_morph_channel_is_target_ordinal_not_opaque_id() {
        let mut model = morph_model(1);
        let mut animation = clip();
        animation.mesh_channels = Some(vec![MeshChannels {
            flags: 1,
            weight_word: Some(0),
            channels: vec![MeshChannel {
                target_id: 0,
                offset: 0,
                bytes: 12,
                record_count: 3,
                dense_frame_count_matches: true,
                values: vec![0., 1., 0.],
                raw_data: Vec::new(),
            }],
        }]);
        model.sections.push(Section {
            magic: "MDLA0006".into(),
            offset: 0,
            body_offset: 0,
            end: 0,
            data: SectionData::Animation(vec![animation]),
        });
        let mut animator = ModelAnimator::new(model).unwrap();
        animator.step(0.25).unwrap();
        assert_eq!(animator.pose.mesh_weights[0][&0], 0.5);
        assert_eq!(
            animator.skinned_meshes().unwrap()[0].vertices[0].position,
            [0.25, 0., 0.]
        );
    }
    #[test]
    fn mirror_preserves_reverse_and_events_use_stored_seconds() {
        let mut layer = AnimationLayer::new(0);
        layer.mode = PlaybackMode::PingPong;
        layer.time = 0.9;
        advance_layer_clock(&mut layer, 0.2, 1.).unwrap();
        assert!((layer.time - 0.9).abs() < 1e-6);
        assert_eq!(layer.direction, -1.);
        advance_layer_clock(&mut layer, 0.2, 1.).unwrap();
        assert!((layer.time - 0.7).abs() < 1e-6);
        assert_eq!(layer.direction, -1.);
        let mut model = morph_model(1);
        let mut animation = clip();
        animation.events.push(AnimationEvent {
            frame_word: 0.25_f32.to_bits(),
            text: "{\"name\":\"quarter-second\"}".into(),
        });
        model.sections.push(Section {
            magic: "MDLA0006".into(),
            offset: 0,
            body_offset: 0,
            end: 0,
            data: SectionData::Animation(vec![animation]),
        });
        let mut animator = ModelAnimator::new(model).unwrap();
        animator.step(0.3).unwrap();
        assert_eq!(animator.pose.events.len(), 1);
        assert_eq!(animator.pose.events[0].frame, 0.5);
        assert_eq!(animator.pose.events[0].payload["name"], "quarter-second");
    }
    #[test]
    fn transform_track_requires_real_sentinel_instead_of_duplicating_last_key() {
        let track = DenseTrack {
            flags: 0,
            offset: 0,
            bytes: 36,
            stride: 36,
            record_count: 1,
            dense_frame_count_matches: false,
            values: vec![0., 0., 0., 0., 0., 0., 1., 1., 1.],
            raw_data: Vec::new(),
        };
        assert!(sample_transform(&track, (0, 1, 0.5)).is_err());
    }
    #[test]
    fn endpoint_sentinel_and_fractional_frames() {
        let f = frame_sample(0.25, 10., 3);
        assert_eq!((f.0, f.1), (2, 3));
        assert!((f.2 - 0.5).abs() < 1e-6);
    }
    #[test]
    fn hierarchy_and_inverse_bind() {
        let bones = vec![
            BoneState {
                name: "parent".into(),
                parent: None,
                bind: Transform {
                    translation: Vec3::new(2., 0., 0.),
                    ..Transform::default()
                },
                properties: Value::Null,
            },
            BoneState {
                name: "child".into(),
                parent: Some(0),
                bind: Transform {
                    translation: Vec3::new(3., 0., 0.),
                    ..Transform::default()
                },
                properties: Value::Null,
            },
        ];
        let local = bones.iter().map(|b| b.bind).collect::<Vec<_>>();
        let w = world_matrices(&bones, &local).unwrap();
        assert_eq!(w[1].translation(), Vec3::new(5., 0., 0.));
        let mut broken = bones.clone();
        broken[0].parent = Some(1);
        assert!(world_matrices(&broken, &local).is_err());
    }
    #[test]
    fn transform_decomposition_roundtrip() {
        let m = Mat4::trs(
            Vec3::new(3., 4., 5.),
            Quat::euler_xyz(Vec3::new(0.1, 0.2, 0.3)),
            Vec3::new(2., 3., 4.),
        );
        let result = Transform::from_matrix(m).matrix();
        for (a, b) in m.0.iter().zip(result.0) {
            assert!((a - b).abs() < 1e-5)
        }
        let mirrored = Mat4::trs(
            Vec3::new(1., 2., 3.),
            Quat::euler_xyz(Vec3::new(0.2, -0.1, 0.3)),
            Vec3::new(-2., 3., 4.),
        );
        let result = Transform::from_matrix(mirrored).matrix();
        for (a, b) in mirrored.0.iter().zip(result.0) {
            assert!((a - b).abs() < 1e-5);
        }
    }
}
