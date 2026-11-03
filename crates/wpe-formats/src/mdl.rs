//! Typed byte ownership for observed MDLV4/14/17/23 mesh containers.
//! Skeleton/animation data is decoded structurally; this module does not execute poses.
use crate::{Error, MAX_ALLOCATION, Result, reader::Reader};
use serde::Serialize;

const WIDTHS: [(u32, usize); 26] = [
    (1, 12),
    (2, 12),
    (4, 16),
    (8, 8),
    (0x10, 12),
    (0x20, 16),
    (0x40, 8),
    (0x80, 12),
    (0x100, 16),
    (0x200, 8),
    (0x400, 12),
    (0x800, 16),
    (0x1000, 8),
    (0x2000, 12),
    (0x4000, 16),
    (0x8000, 16),
    (0x10000, 16),
    (0x20000, 8),
    (0x40000, 12),
    (0x80000, 16),
    (0x100000, 8),
    (0x200000, 12),
    (0x400000, 16),
    (0x800000, 16),
    (0x1000000, 16),
    (0x2000000, 12),
];

#[derive(Clone, Debug, Serialize)]
pub struct ExtraBuffer {
    pub kind: Option<u32>,
    pub offset: usize,
    pub bytes: usize,
    #[serde(skip)]
    pub data: Vec<u8>,
}
#[derive(Clone, Debug, Serialize)]
pub struct Batch {
    pub pair: [u32; 2],
    pub name: String,
    pub flags: u32,
    pub indices_a: Vec<u32>,
    pub indices_b: Vec<u32>,
}
#[derive(Clone, Debug, Serialize)]
pub struct SkinVertex {
    pub bone_indices: [u32; 4],
    pub weights: [f32; 4],
}
#[derive(Clone, Debug, Serialize)]
pub struct Mesh {
    pub offset: usize,
    pub end: usize,
    pub materials: Vec<String>,
    pub flags: u32,
    pub extra_flags: Option<u32>,
    pub bounds: Option<[f32; 6]>,
    pub vertex_format: u32,
    pub vertex_stride: usize,
    pub vertex_count: usize,
    pub vertex_offset: usize,
    pub vertex_bytes: usize,
    #[serde(skip)]
    pub vertices: Vec<u8>,
    pub index_offset: usize,
    pub index_bytes: usize,
    #[serde(skip)]
    pub indices: Vec<u16>,
    pub index_count: usize,
    pub extra_buffers: Vec<ExtraBuffer>,
    pub batches: Vec<Batch>,
    /// Exact observed 0x180000f layout; weights are never normalized.
    #[serde(skip)]
    pub skin: Option<Vec<SkinVertex>>,
    pub active_skin_indices_in_bounds: Option<bool>,
    pub skin_weights_finite: Option<bool>,
}
impl Mesh {
    pub fn position(&self, index: usize) -> Option<[f32; 3]> {
        if self.vertex_format & 1 == 0 || index >= self.vertex_count {
            return None;
        }
        let b = &self.vertices[index * self.vertex_stride..];
        Some(std::array::from_fn(|i| {
            f32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().unwrap())
        }))
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct Bone {
    pub name: String,
    pub flags: u32,
    pub parent: i32,
    pub matrix: Option<[f32; 16]>,
    #[serde(skip)]
    pub matrix_data: Vec<u8>,
    pub matrix_bytes: usize,
    pub properties_json: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct SecondaryBone {
    pub name: String,
    pub bone: u32,
    pub type_word: u32,
    pub matrix: [f32; 16],
}
#[derive(Clone, Debug, Serialize)]
pub struct Constraint {
    pub bone: u32,
    pub float_words: [f32; 2],
    pub flags: u32,
    pub extra: Option<[f32; 2]>,
}
#[derive(Clone, Debug, Serialize)]
pub struct TableAEntry {
    pub id_word: u32,
    pub float_words: [f32; 3],
}
#[derive(Clone, Debug, Serialize)]
pub struct TableA {
    pub id: u32,
    pub entries: Vec<TableAEntry>,
}
#[derive(Clone, Debug, Serialize)]
pub struct RelatedSettings {
    pub words: [u32; 4],
    pub related_bones: Vec<u32>,
}
#[derive(Clone, Debug, Serialize)]
pub struct ConstraintTarget {
    pub bone: u32,
    pub settings: Vec<RelatedSettings>,
}
#[derive(Clone, Debug, Serialize)]
pub struct TableB {
    pub bone: u32,
    pub secondary_refs: Vec<u32>,
    pub targets: Vec<ConstraintTarget>,
}
#[derive(Clone, Debug, Serialize)]
pub struct BonePoseRecord {
    pub float3: [f32; 3],
    pub matrix: [f32; 16],
}
#[derive(Clone, Debug, Serialize)]
pub struct Skeleton {
    pub bones: Vec<Bone>,
    pub secondary: Vec<SecondaryBone>,
    pub cached_matrices: Option<Vec<[f32; 16]>>,
    pub constraints: Vec<Constraint>,
    pub table_a: Vec<TableA>,
    pub table_b: Vec<TableB>,
    pub pose_records: Option<Vec<BonePoseRecord>>,
    pub mapping: Option<Vec<u32>>,
    pub bone_ids: Option<Vec<u32>>,
}
#[derive(Clone, Debug, Serialize)]
pub struct Attachment {
    pub bone: u16,
    pub name: String,
    pub matrix: [f32; 16],
}
#[derive(Clone, Debug, Serialize)]
pub struct DenseTrack {
    pub flags: u32,
    pub offset: usize,
    pub bytes: usize,
    pub stride: usize,
    pub record_count: usize,
    pub dense_frame_count_matches: bool,
    /// Float values are accompanied by exact raw bits, preserving NaNs and unnamed fields.
    #[serde(skip)]
    pub values: Vec<f32>,
    #[serde(skip)]
    pub raw_data: Vec<u8>,
}
#[derive(Clone, Debug, Serialize)]
pub struct MeshChannel {
    pub target_id: u16,
    pub offset: usize,
    pub bytes: usize,
    pub record_count: usize,
    pub dense_frame_count_matches: bool,
    #[serde(skip)]
    pub values: Vec<f32>,
    #[serde(skip)]
    pub raw_data: Vec<u8>,
}
#[derive(Clone, Debug, Serialize)]
pub struct MeshChannels {
    pub flags: u32,
    pub weight_word: Option<u32>,
    pub channels: Vec<MeshChannel>,
}
#[derive(Clone, Debug, Serialize)]
pub struct Transition {
    pub reference: u16,
    pub words: [u32; 4],
}
#[derive(Clone, Debug, Serialize)]
pub struct AnimationEvent {
    pub frame_word: u32,
    pub text: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct Clip {
    pub offset: usize,
    pub end: usize,
    pub id: u64,
    pub name: String,
    pub mode: String,
    pub fps: f32,
    pub fps_bits: u32,
    pub frames: u32,
    pub flags: u32,
    pub tracks: Vec<DenseTrack>,
    pub secondary_tracks: Vec<DenseTrack>,
    pub constraint_tracks: Vec<DenseTrack>,
    pub scalar_tracks: Vec<DenseTrack>,
    pub bone_scalar_group_a: Option<Vec<DenseTrack>>,
    pub mesh_channels: Option<Vec<MeshChannels>>,
    pub metadata_words: Vec<u32>,
    pub bone_scalar_group_b: Option<Vec<DenseTrack>>,
    pub transition: Option<Transition>,
    pub events: Vec<AnimationEvent>,
}
/// HALF payloads are preserved in the raw section and decoded without guessing a
/// target ID domain. MDLA mesh-channel target_id indexes the MDMP target order.
#[derive(Clone, Debug, Serialize)]
pub struct HalfChannel {
    pub offset: usize,
    pub bytes: usize,
    pub components: usize,
    pub records: usize,
    pub all_values_finite: bool,
    #[serde(skip)]
    pub values: Vec<f32>,
}
#[derive(Clone, Debug, Serialize)]
pub struct MorphBindingRule {
    pub bone: u32,
    pub rule: u32,
    pub range: [f32; 2],
}
#[derive(Clone, Debug, Serialize)]
pub struct MorphTarget {
    pub id: u64,
    pub name: String,
    pub offset: usize,
    pub end: usize,
    pub positions: HalfChannel,
    pub normals: Option<HalfChannel>,
    pub extra_vectors: Option<HalfChannel>,
    pub extra_scalars: Option<HalfChannel>,
    pub binding_rule: Option<MorphBindingRule>,
}
#[derive(Clone, Debug, Serialize)]
pub struct MorphMesh {
    pub mesh: usize,
    pub flags: u32,
    pub base_influence: f32,
    pub declared_vertex_count: u32,
    pub effective_vertex_count: u32,
    pub targets: Vec<MorphTarget>,
}
#[derive(Clone, Debug, Serialize)]
pub struct MorphSection {
    pub offset: usize,
    pub bytes: usize,
    pub meshes: Vec<MorphMesh>,
    #[serde(skip)]
    pub raw_body: Vec<u8>,
}
fn half_channel(r: &mut Reader<'_>, components: usize) -> Result<HalfChannel> {
    let (offset, bytes) = r.blob()?;
    let width = components * 2;
    if bytes.len() % width != 0 {
        return Err(Error::parse(
            offset,
            "MDMP HALF payload does not match component stride",
        ));
    }
    let values = bytes
        .chunks_exact(2)
        .map(|b| half::f16::from_bits(u16::from_le_bytes([b[0], b[1]])).to_f32())
        .collect::<Vec<_>>();
    Ok(HalfChannel {
        offset,
        bytes: bytes.len(),
        components,
        records: bytes.len() / width,
        all_values_finite: values.iter().all(|v| v.is_finite()),
        values,
    })
}
fn morph_section(
    r: &mut Reader<'_>,
    meshes: &[Mesh],
    bone_count: Option<usize>,
) -> Result<MorphSection> {
    let offset = r.pos;
    let raw_body = r.bytes[offset..r.end].to_vec();
    let mut output = Vec::with_capacity(meshes.len());
    for (mesh_index, mesh) in meshes.iter().enumerate() {
        let count = r.u16()? as usize;
        let mut base_influence = 0.0;
        let mut declared_vertex_count = 0;
        let mut effective_vertex_count = 0;
        let mut targets = Vec::new();
        if count > 0 {
            base_influence = r.f32()?;
            declared_vertex_count = r.u32()?;
            effective_vertex_count = declared_vertex_count;
            for _ in 0..count {
                let start = r.pos;
                let id = r.u64()?;
                let name = r.zstr(1024 * 1024)?;
                let positions = half_channel(r, 3)?;
                effective_vertex_count = effective_vertex_count.min(positions.records as u32);
                if positions.records != effective_vertex_count as usize {
                    return Err(Error::parse(
                        start,
                        "MDMP position count differs from effective declared count",
                    ));
                }
                let normals = if mesh.flags & 0x400 != 0 {
                    Some(half_channel(r, 3)?)
                } else {
                    None
                };
                let extra_vectors = if mesh.flags & 0x800 != 0 {
                    Some(half_channel(r, 3)?)
                } else {
                    None
                };
                let extra_scalars = if mesh.flags & 0x1000 != 0 {
                    Some(half_channel(r, 1)?)
                } else {
                    None
                };
                for channel in [&normals, &extra_vectors, &extra_scalars]
                    .into_iter()
                    .flatten()
                {
                    if channel.records != effective_vertex_count as usize {
                        return Err(Error::parse(
                            channel.offset,
                            "MDMP optional channel vertex count mismatch",
                        ));
                    }
                }
                let binding_rule = if mesh.flags & 0x2000 != 0 {
                    let bone = r.u32()?;
                    let rule = r.u32()?;
                    let range = array_f32(r)?;
                    if bone_count.is_some_and(|n| bone as usize >= n) {
                        return Err(Error::parse(
                            r.pos - 16,
                            "MDMP binding bone outside skeleton",
                        ));
                    }
                    Some(MorphBindingRule { bone, rule, range })
                } else {
                    None
                };
                targets.push(MorphTarget {
                    id,
                    name,
                    offset: start,
                    end: r.pos,
                    positions,
                    normals,
                    extra_vectors,
                    extra_scalars,
                    binding_rule,
                });
            }
        }
        output.push(MorphMesh {
            mesh: mesh_index,
            flags: mesh.flags,
            base_influence,
            declared_vertex_count,
            effective_vertex_count,
            targets,
        });
    }
    Ok(MorphSection {
        offset,
        bytes: r.pos - offset,
        meshes: output,
        raw_body,
    })
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", content = "data")]
pub enum SectionData {
    Skeleton(Skeleton),
    Attachments(Vec<Attachment>),
    Matrices(Vec<[f32; 16]>),
    Animation(Vec<Clip>),
    Morph(MorphSection),
    /// Unknown sections are retained and exposed in summary.unsupported_sections.
    Opaque(#[serde(skip_serializing)] Vec<u8>),
}
#[derive(Clone, Debug, Serialize)]
pub struct Section {
    pub magic: String,
    pub offset: usize,
    pub body_offset: usize,
    pub end: usize,
    pub data: SectionData,
}
#[derive(Clone, Debug, Serialize)]
pub struct Model {
    pub version: u32,
    pub global_vertex_format: u32,
    pub material_slots: usize,
    pub meshes: Vec<Mesh>,
    pub sections: Vec<Section>,
    pub file_bytes: usize,
    pub parsed_bytes: usize,
    pub unsupported_sections: Vec<String>,
}

fn array_f32<const N: usize>(r: &mut Reader<'_>) -> Result<[f32; N]> {
    let mut values = [0.; N];
    for v in &mut values {
        *v = r.f32()?;
    }
    Ok(values)
}
fn array_u32<const N: usize>(r: &mut Reader<'_>) -> Result<[u32; N]> {
    let mut values = [0; N];
    for v in &mut values {
        *v = r.u32()?;
    }
    Ok(values)
}
fn words(r: &mut Reader<'_>, n: usize) -> Result<Vec<u32>> {
    if n > r.remaining() / 4 {
        return Err(Error::parse(r.pos, "word count exceeds section bytes"));
    }
    (0..n).map(|_| r.u32()).collect()
}
fn matrix_data(bytes: &[u8]) -> Option<[f32; 16]> {
    if bytes.len() != 64 {
        return None;
    }
    Some(std::array::from_fn(|i| {
        f32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap())
    }))
}
fn skeleton(r: &mut Reader<'_>) -> Result<Skeleton> {
    let n = r.count(128, "bones")?;
    let mut bones = Vec::with_capacity(n);
    for _ in 0..n {
        let name = r.zstr(1024 * 1024)?;
        let flags = r.u32()?;
        let parent = r.i32()?;
        let (_, matrix_data) = r.blob()?;
        let matrix = matrix_data_fn(&matrix_data);
        let properties_json = r.zstr(1024 * 1024)?;
        if parent < -1 || parent >= n as i32 {
            return Err(Error::parse(r.pos, "bone parent index outside skeleton"));
        }
        bones.push(Bone {
            name,
            flags,
            parent,
            matrix,
            matrix_bytes: matrix_data.len(),
            matrix_data,
            properties_json,
        });
    }
    for index in 0..n {
        let mut seen = [false; 128];
        let mut j = index as i32;
        while j >= 0 {
            if seen[j as usize] {
                return Err(Error::parse(r.pos, "cyclic bone parent path"));
            }
            seen[j as usize] = true;
            j = bones[j as usize].parent;
        }
    }
    let ns = r.u16()? as usize;
    let mut secondary = Vec::with_capacity(ns);
    for _ in 0..ns {
        secondary.push(SecondaryBone {
            name: r.zstr(1024 * 1024)?,
            bone: r.u32()?,
            type_word: r.u32()?,
            matrix: array_f32(r)?,
        });
    }
    let cached_matrices = if r.u8()? != 0 {
        Some((0..n + ns).map(|_| array_f32(r)).collect::<Result<_>>()?)
    } else {
        None
    };
    let nc = r.count(1_000_000, "constraints")?;
    let mut constraints = Vec::with_capacity(nc.min(r.remaining() / 16));
    for _ in 0..nc {
        let bone = r.u32()?;
        let float_words = array_f32(r)?;
        let flags = r.u32()?;
        let extra = if flags & 2 != 0 {
            Some(array_f32(r)?)
        } else {
            None
        };
        constraints.push(Constraint {
            bone,
            float_words,
            flags,
            extra,
        });
    }
    let na = r.u16()? as usize;
    let ids = words(r, na)?;
    let mut table_a = Vec::with_capacity(na);
    for id in ids {
        let ni = r.u16()? as usize;
        let mut entries = Vec::with_capacity(ni.min(r.remaining() / 16));
        for _ in 0..ni {
            entries.push(TableAEntry {
                id_word: r.u32()?,
                float_words: array_f32(r)?,
            });
        }
        table_a.push(TableA { id, entries });
    }
    let nb = r.u16()? as usize;
    let mut table_b = Vec::with_capacity(nb);
    for _ in 0..nb {
        let bone = r.u32()?;
        let nr = r.count(1_000_000, "secondary refs")?;
        let secondary_refs = words(r, nr)?;
        let nt = r.u16()? as usize;
        let mut targets = Vec::with_capacity(nt.min(r.remaining() / 6));
        for _ in 0..nt {
            let bone = r.u32()?;
            let nsettings = r.u16()? as usize;
            let mut settings = Vec::with_capacity(nsettings.min(r.remaining() / 18));
            for _ in 0..nsettings {
                let values = array_u32(r)?;
                let nrelated = r.u16()? as usize;
                let related_bones = words(r, nrelated)?;
                settings.push(RelatedSettings {
                    words: values,
                    related_bones,
                });
            }
            targets.push(ConstraintTarget { bone, settings });
        }
        table_b.push(TableB {
            bone,
            secondary_refs,
            targets,
        });
    }
    let pose_records = if r.u8()? != 0 {
        Some(
            (0..n)
                .map(|_| {
                    Ok(BonePoseRecord {
                        float3: array_f32(r)?,
                        matrix: array_f32(r)?,
                    })
                })
                .collect::<Result<_>>()?,
        )
    } else {
        None
    };
    let mapping = if r.u8()? != 0 {
        Some(words(r, n)?)
    } else {
        None
    };
    if mapping
        .as_ref()
        .is_some_and(|m| m.iter().any(|v| *v as usize >= n))
    {
        return Err(Error::parse(r.pos, "skeleton mapping index outside bones"));
    }
    let bone_ids = if r.u8()? != 0 {
        Some(words(r, n)?)
    } else {
        None
    };
    Ok(Skeleton {
        bones,
        secondary,
        cached_matrices,
        constraints,
        table_a,
        table_b,
        pose_records,
        mapping,
        bone_ids,
    })
}
// Avoid shadowing the helper with the retained byte field's name.
fn matrix_data_fn(b: &[u8]) -> Option<[f32; 16]> {
    matrix_data(b)
}

fn floats_from_bytes(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
        .collect()
}
fn track(r: &mut Reader<'_>, frames: u32, stride: usize) -> Result<DenseTrack> {
    let flags = r.u32()?;
    let (offset, raw_data) = r.blob()?;
    let bytes = raw_data.len();
    if bytes % stride != 0 {
        return Err(Error::parse(
            offset,
            "animation channel size is not a whole record",
        ));
    }
    Ok(DenseTrack {
        flags,
        offset,
        bytes,
        stride,
        record_count: bytes / stride,
        dense_frame_count_matches: bytes as u64 == (frames as u64 + 1) * stride as u64,
        values: floats_from_bytes(&raw_data),
        raw_data,
    })
}
fn tracks(r: &mut Reader<'_>, n: usize, frames: u32, stride: usize) -> Result<Vec<DenseTrack>> {
    if n > r.remaining() / 8 {
        return Err(Error::parse(
            r.pos,
            "animation track count exceeds section bytes",
        ));
    }
    (0..n).map(|_| track(r, frames, stride)).collect()
}
fn clips(
    r: &mut Reader<'_>,
    bone_count: usize,
    secondary_count: usize,
    constraint_count: usize,
    mesh_count: usize,
) -> Result<Vec<Clip>> {
    let n = r.count(100_000, "clips")?;
    let mut clips = Vec::with_capacity(n.min(r.remaining() / 32));
    for _ in 0..n {
        let offset = r.pos;
        let id = r.u64()?;
        let name = r.zstr(1024 * 1024)?;
        let mode = r.zstr(1024 * 1024)?;
        let fps_bits = r.u32()?;
        let fps = f32::from_bits(fps_bits);
        let frames = r.u32()?;
        let flags = r.u32()?;
        let nt = r.count(1_000_000, "bone tracks")?;
        let main_tracks = tracks(r, nt, frames, 36)?;
        let secondary_tracks = tracks(r, secondary_count, frames, 36)?;
        let constraint_tracks = tracks(r, constraint_count, frames, 4)?;
        let nscalar = r.count(1_000_000, "scalar tracks")?;
        let scalar_tracks = tracks(r, nscalar, frames, 4)?;
        let bone_scalar_group_a = if r.u8()? != 0 {
            Some(tracks(r, bone_count, frames, 4)?)
        } else {
            None
        };
        let mesh_channels = if r.u8()? != 0 {
            let mut groups = Vec::with_capacity(mesh_count);
            for _ in 0..mesh_count {
                let flags = r.u32()?;
                let mut weight_word = None;
                let mut channels = Vec::new();
                if flags & 1 != 0 {
                    weight_word = Some(r.u32()?);
                    let nc = r.u16()? as usize;
                    channels.reserve(nc.min(r.remaining() / 6));
                    for _ in 0..nc {
                        let target_id = r.u16()?;
                        let (offset, raw_data) = r.blob()?;
                        let bytes = raw_data.len();
                        if bytes % 4 != 0 {
                            return Err(Error::parse(
                                offset,
                                "mesh channel byte size is not divisible by four",
                            ));
                        }
                        channels.push(MeshChannel {
                            target_id,
                            offset,
                            bytes,
                            record_count: bytes / 4,
                            dense_frame_count_matches: bytes as u64 == (frames as u64 + 1) * 4,
                            values: floats_from_bytes(&raw_data),
                            raw_data,
                        });
                    }
                }
                groups.push(MeshChannels {
                    flags,
                    weight_word,
                    channels,
                });
            }
            Some(groups)
        } else {
            None
        };
        let metadata_words = words(r, 6)?;
        let bone_scalar_group_b = if r.u8()? != 0 {
            Some(tracks(r, bone_count, frames, 4)?)
        } else {
            None
        };
        let transition = if flags & 1 != 0 {
            Some(Transition {
                reference: r.u16()?,
                words: array_u32(r)?,
            })
        } else {
            None
        };
        let ne = r.count(1_000_000, "animation events")?;
        let mut events = Vec::with_capacity(ne.min(r.remaining() / 5));
        for _ in 0..ne {
            events.push(AnimationEvent {
                frame_word: r.u32()?,
                text: r.zstr(1024 * 1024)?,
            });
        }
        clips.push(Clip {
            offset,
            end: r.pos,
            id,
            name,
            mode,
            fps,
            fps_bits,
            frames,
            flags,
            tracks: main_tracks,
            secondary_tracks,
            constraint_tracks,
            scalar_tracks,
            bone_scalar_group_a,
            mesh_channels,
            metadata_words,
            bone_scalar_group_b,
            transition,
            events,
        });
    }
    Ok(clips)
}

impl Model {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_ALLOCATION {
            return Err(Error::limit(0, "model exceeds 512 MiB input limit"));
        }
        let mut r = Reader::new(bytes);
        let magic = r.zstr(16)?;
        let version = magic
            .strip_prefix("MDLV")
            .filter(|v| v.len() == 4 && v.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|v| v.parse::<u32>().ok())
            .ok_or_else(|| Error::unsupported(0, format!("model magic {magic:?}")))?;
        if ![4, 14, 17, 23].contains(&version) {
            return Err(Error::unsupported(
                0,
                format!("MDLV version {version}; observed versions are4/14/17/23"),
            ));
        }
        let global_vertex_format = r.u32()?;
        let material_slots = r.count(10_000, "material slots")?;
        let nmesh = r.count(10_000, "meshes")?;
        if material_slots
            .checked_mul(nmesh)
            .is_none_or(|n| n > 1_000_000)
        {
            return Err(Error::limit(
                r.pos,
                "model material-reference count exceeds one million",
            ));
        }
        let mut meshes = Vec::with_capacity(nmesh.min(r.remaining() / 16));
        for _ in 0..nmesh {
            let offset = r.pos;
            let mut materials = Vec::with_capacity(material_slots);
            for _ in 0..material_slots {
                materials.push(r.zstr(1024 * 1024)?);
            }
            let flags = r.u32()?;
            let extra_flags = if flags & 2 != 0 { Some(r.u32()?) } else { None };
            let bounds = if version >= 17 {
                Some(array_f32(&mut r)?)
            } else {
                None
            };
            let vertex_format = if version >= 15 {
                r.u32()?
            } else {
                global_vertex_format
            };
            let known = WIDTHS.iter().fold(0, |mask, (bit, _)| mask | *bit);
            if vertex_format & !known != 0 {
                return Err(Error::unsupported(
                    r.pos,
                    "unknown model vertex format bits",
                ));
            }
            let vertex_stride = WIDTHS
                .iter()
                .filter(|(bit, _)| vertex_format & bit != 0)
                .map(|(_, n)| n)
                .sum::<usize>();
            let (vertex_offset, vertices) = r.blob()?;
            if vertex_stride == 0 || vertices.len() % vertex_stride != 0 {
                return Err(Error::parse(
                    vertex_offset,
                    "vertex byte size does not divide by known stride",
                ));
            }
            let vertex_count = vertices.len() / vertex_stride;
            let (index_offset, index_data) = r.blob()?;
            if index_data.len() % 2 != 0 {
                return Err(Error::parse(index_offset, "index byte size must be even"));
            }
            let indices = index_data
                .chunks_exact(2)
                .map(|b| u16::from_le_bytes(b.try_into().unwrap()))
                .collect::<Vec<_>>();
            if indices.iter().any(|v| *v as usize >= vertex_count) {
                return Err(Error::parse(
                    index_offset,
                    "mesh u16 index exceeds vertex count",
                ));
            }
            let mut extra_buffers = Vec::new();
            if version >= 21 {
                if r.u8()? != 0 {
                    let kind = Some(r.u32()?);
                    let (offset, data) = r.blob()?;
                    extra_buffers.push(ExtraBuffer {
                        kind,
                        offset,
                        bytes: data.len(),
                        data,
                    });
                }
                if r.u8()? != 0 {
                    let (offset, data) = r.blob()?;
                    extra_buffers.push(ExtraBuffer {
                        kind: None,
                        offset,
                        bytes: data.len(),
                        data,
                    });
                }
            }
            let mut batches = Vec::new();
            if version >= 23 {
                let nb = r.count(100_000, "mesh batches")?;
                batches.reserve(nb.min(r.remaining() / 21));
                for _ in 0..nb {
                    let pair = array_u32(&mut r)?;
                    let name = r.zstr(1024 * 1024)?;
                    let flags = r.u32()?;
                    let na = r.count(1_000_000, "batch indices a")?;
                    let indices_a = words(&mut r, na)?;
                    let nb = r.count(1_000_000, "batch indices b")?;
                    let indices_b = words(&mut r, nb)?;
                    batches.push(Batch {
                        pair,
                        name,
                        flags,
                        indices_a,
                        indices_b,
                    });
                }
            }
            let skin = if vertex_format == 0x180000f {
                Some(
                    vertices
                        .chunks_exact(vertex_stride)
                        .map(|v| SkinVertex {
                            bone_indices: std::array::from_fn(|i| {
                                u32::from_le_bytes(v[40 + i * 4..44 + i * 4].try_into().unwrap())
                            }),
                            weights: std::array::from_fn(|i| {
                                f32::from_le_bytes(v[56 + i * 4..60 + i * 4].try_into().unwrap())
                            }),
                        })
                        .collect::<Vec<_>>(),
                )
            } else {
                None
            };
            let skin_weights_finite = skin
                .as_ref()
                .map(|s| s.iter().all(|v| v.weights.iter().all(|v| v.is_finite())));
            meshes.push(Mesh {
                offset,
                end: r.pos,
                materials,
                flags,
                extra_flags,
                bounds,
                vertex_format,
                vertex_stride,
                vertex_count,
                vertex_offset,
                vertex_bytes: vertices.len(),
                vertices,
                index_offset,
                index_bytes: index_data.len(),
                index_count: indices.len(),
                indices,
                extra_buffers,
                batches,
                skin,
                active_skin_indices_in_bounds: None,
                skin_weights_finite,
            });
        }
        let mut sections = Vec::new();
        let mut unsupported_sections = Vec::new();
        let mut counts = None;
        while r.pos < bytes.len() {
            let offset = r.pos;
            let magic = r.zstr(16)?;
            if magic.is_empty() {
                if r.pos != bytes.len() {
                    return Err(Error::parse(r.pos, "model terminator precedes more bytes"));
                }
                break;
            }
            let end = r.u32()? as usize;
            let body_offset = r.pos;
            if end < r.pos || end >= bytes.len() {
                return Err(Error::parse(
                    r.pos - 4,
                    "model section absolute end outside file",
                ));
            }
            r.end = end;
            let data = match magic.as_str() {
                "MDLS0004" => {
                    let s = skeleton(&mut r)?;
                    counts = Some((s.bones.len(), s.secondary.len(), s.constraints.len()));
                    SectionData::Skeleton(s)
                }
                "MDAT0001" => {
                    let n = r.u16()? as usize;
                    let mut entries = Vec::with_capacity(n.min(r.remaining() / 67));
                    for _ in 0..n {
                        let bone = r.u16()?;
                        let name = r.zstr(1024 * 1024)?;
                        let matrix = array_f32(&mut r)?;
                        if counts.is_some_and(|(n, _, _)| bone as usize >= n) {
                            return Err(Error::parse(r.pos, "attachment bone outside skeleton"));
                        }
                        entries.push(Attachment { bone, name, matrix });
                    }
                    SectionData::Attachments(entries)
                }
                "MDLE0002" => {
                    let n = r.count(MAX_ALLOCATION, "matrix bytes")?;
                    if n % 64 != 0 {
                        return Err(Error::parse(
                            r.pos - 4,
                            "matrix section byte size is not divisible by64",
                        ));
                    }
                    if n > r.remaining() {
                        return Err(Error::parse(r.pos, "matrix bytes exceed section"));
                    }
                    SectionData::Matrices(
                        (0..n / 64)
                            .map(|_| array_f32(&mut r))
                            .collect::<Result<_>>()?,
                    )
                }
                "MDMP0001" => SectionData::Morph(morph_section(
                    &mut r,
                    &meshes,
                    counts.map(|(n, s, _)| n + s),
                )?),
                "MDLA0006" => {
                    let (n, s, c) = counts.ok_or_else(|| {
                        Error::parse(offset, "animation requires prior skeleton section")
                    })?;
                    SectionData::Animation(clips(&mut r, n, s, c, meshes.len())?)
                }
                _ => {
                    unsupported_sections.push(magic.clone());
                    let n = r.remaining();
                    SectionData::Opaque(r.take(n)?.to_vec())
                }
            };
            if r.pos != end {
                return Err(Error::parse(
                    r.pos,
                    format!("unparsed bytes in typed section {magic}: end {end:#x}"),
                ));
            }
            r.end = bytes.len();
            sections.push(Section {
                magic,
                offset,
                body_offset,
                end,
                data,
            });
        }
        if let Some((n, _, _)) = counts {
            for mesh in &mut meshes {
                mesh.active_skin_indices_in_bounds = mesh.skin.as_ref().map(|s| {
                    s.iter().all(|v| {
                        (0..4).all(|i| v.weights[i] == 0. || (v.bone_indices[i] as usize) < n)
                    })
                });
            }
        }
        Ok(Self {
            version,
            global_vertex_format,
            material_slots,
            meshes,
            sections,
            file_bytes: bytes.len(),
            parsed_bytes: r.pos,
            unsupported_sections,
        })
    }
    pub fn summary(&self) -> serde_json::Value {
        let mut value = serde_json::to_value(self).expect("model metadata serializes");
        value["mesh_count"] = self.meshes.len().into();
        value["fully_typed"] = self.unsupported_sections.is_empty().into();
        let morphs = self
            .sections
            .iter()
            .filter_map(|s| {
                if let SectionData::Morph(m) = &s.data {
                    Some(m)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        value["morph_target_count"] = morphs
            .iter()
            .flat_map(|m| &m.meshes)
            .map(|m| m.targets.len())
            .sum::<usize>()
            .into();
        value["morph_extra_channel_meshes"] = serde_json::json!(
            morphs
                .iter()
                .flat_map(|m| &m.meshes)
                .filter(|m| m.flags & 0x3800 != 0)
                .map(|m| m.mesh)
                .collect::<Vec<_>>()
        );
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn triangle() -> Vec<u8> {
        let mut out = b"MDLV0023\0".to_vec();
        for v in [9u32, 0, 1, 0] {
            out.extend(v.to_le_bytes());
        }
        for v in [0f32, 0., 0., 1., 1., 0.] {
            out.extend(v.to_le_bytes());
        }
        out.extend(9u32.to_le_bytes());
        out.extend(60u32.to_le_bytes());
        for v in [0f32, 0., 0., 0., 1., 1., 0., 0., 1., 1., 0., 1., 0., 0., 0.] {
            out.extend(v.to_le_bytes());
        }
        out.extend(6u32.to_le_bytes());
        for i in [0u16, 1, 2] {
            out.extend(i.to_le_bytes());
        }
        out.extend([0, 0]);
        out.extend(0u32.to_le_bytes());
        out.push(0);
        out
    }
    #[test]
    fn triangle_mesh_and_bad_index() {
        let bytes = triangle();
        let m = Model::parse(&bytes).unwrap();
        assert_eq!(m.meshes[0].vertex_count, 3);
        assert_eq!(m.meshes[0].indices, [0, 1, 2]);
        assert_eq!(m.meshes[0].position(1), Some([1., 0., 0.]));
        let p = m.meshes[0].index_offset;
        let mut bad = bytes;
        bad[p..p + 2].copy_from_slice(&3u16.to_le_bytes());
        assert!(Model::parse(&bad).is_err());
    }
    #[test]
    fn malformed_model_sizes_fail() {
        let bytes = triangle();
        for n in 0..bytes.len() - 1 {
            assert!(Model::parse(&bytes[..n]).is_err(), "prefix {n}");
        }
        let mut bad = bytes;
        bad[13..17].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(Model::parse(&bad).is_err());
    }
    fn morph_fixture(flags: u32) -> Vec<u8> {
        let mut bytes = triangle();
        bytes.pop();
        bytes[21..25].copy_from_slice(&flags.to_le_bytes());
        bytes.extend(b"MDMP0001\0");
        let end_at = bytes.len();
        bytes.extend(0u32.to_le_bytes());
        bytes.extend(1u16.to_le_bytes());
        bytes.extend(0.5f32.to_le_bytes());
        bytes.extend(3u32.to_le_bytes());
        bytes.extend(9u64.to_le_bytes());
        bytes.extend(b"raise\0");
        let append = |bytes: &mut Vec<u8>, components: usize| {
            bytes.extend(((3 * components * 2) as u32).to_le_bytes());
            for _ in 0..3 {
                for component in 0..components {
                    bytes.extend(
                        half::f16::from_f32(if component == 0 { 1.0 } else { 0.0 })
                            .to_bits()
                            .to_le_bytes(),
                    );
                }
            }
        };
        append(&mut bytes, 3);
        if flags & 0x400 != 0 {
            append(&mut bytes, 3)
        }
        if flags & 0x800 != 0 {
            append(&mut bytes, 3)
        }
        if flags & 0x1000 != 0 {
            append(&mut bytes, 1)
        }
        if flags & 0x2000 != 0 {
            bytes.extend(0u32.to_le_bytes());
            bytes.extend(2u32.to_le_bytes());
            bytes.extend(0.1f32.to_le_bytes());
            bytes.extend(0.9f32.to_le_bytes());
        }
        let end = bytes.len() as u32;
        bytes[end_at..end_at + 4].copy_from_slice(&end.to_le_bytes());
        bytes.push(0);
        bytes
    }
    #[test]
    fn mdmp_half_payloads_are_typed_and_raw_body_is_preserved() {
        let bytes = morph_fixture(0x3c00);
        let model = Model::parse(&bytes).unwrap();
        let section = model
            .sections
            .iter()
            .find(|s| s.magic == "MDMP0001")
            .unwrap();
        let SectionData::Morph(morph) = &section.data else {
            panic!("typed morph")
        };
        assert_eq!(morph.raw_body, bytes[section.body_offset..section.end]);
        let mesh = &morph.meshes[0];
        assert_eq!(mesh.base_influence, 0.5);
        assert_eq!(mesh.targets[0].id, 9);
        assert_eq!(mesh.targets[0].name, "raise");
        assert_eq!(
            mesh.targets[0].positions.values,
            [1., 0., 0., 1., 0., 0., 1., 0., 0.]
        );
        assert!(mesh.targets[0].normals.is_some());
        assert!(mesh.targets[0].extra_vectors.is_some());
        assert!(mesh.targets[0].extra_scalars.is_some());
        assert_eq!(mesh.targets[0].binding_rule.as_ref().unwrap().rule, 2);
        assert!(model.unsupported_sections.is_empty());
        assert_eq!(model.summary()["morph_target_count"], 1);
    }
    #[test]
    fn mdmp_truncated_blob_and_mismatched_optional_stride_fail() {
        let bytes = morph_fixture(0x400);
        for end in bytes.len() - 20..bytes.len() - 1 {
            assert!(Model::parse(&bytes[..end]).is_err());
        }
        let model = Model::parse(&bytes).unwrap();
        let SectionData::Morph(morph) = &model.sections[0].data else {
            panic!("morph")
        };
        let normal = morph.meshes[0].targets[0].normals.as_ref().unwrap();
        let mut bad = bytes;
        bad[normal.offset - 4..normal.offset].copy_from_slice(&16u32.to_le_bytes());
        assert!(Model::parse(&bad).is_err());
    }
}
