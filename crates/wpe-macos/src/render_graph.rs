//! Lower framebuffer blend operations to an explicit destination snapshot.
//! Covered fragments retain the original draw's depth, cull, discard and masks.
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::hash::{Hash, Hasher};
use wpe_render::{
    BlendState, RenderFrame, RenderPass, Target, Texture, TextureFormat, effect::Effect,
};
#[derive(Default)]
pub struct GraphLowerer {
    materials: HashMap<u64, wpe_render::Material>,
}
impl GraphLowerer {
    pub fn lower(&mut self, input: &RenderFrame) -> Result<RenderFrame, String> {
        let separated = wpe_render::effect::lower_separable(input).map_err(|e| e.to_string())?;
        let input = &separated;
        input.validate().map_err(|e| e.to_string())?;
        if !input
            .draws
            .iter()
            .any(|draw| matches!(draw.material.state.blend, BlendState::Custom(_)))
        {
            return Ok(input.clone());
        }
        let mut frame = input.clone();
        let mut next = 0u64;
        let mut used = frame.textures.keys().copied().collect::<BTreeSet<_>>();
        let mut allocate = || -> Result<u64, String> {
            loop {
                next = next.checked_add(1).ok_or("Texture ID space exhausted")?;
                if used.insert(next) {
                    return Ok(next);
                }
            }
        };
        let screen = allocate()?;
        frame.textures.insert(
            screen,
            Texture::target_with_format(
                "native-screen-composition",
                frame.width,
                frame.height,
                TextureFormat::Rgba16Float,
            ),
        );
        let passes = if input.passes.is_empty() {
            vec![RenderPass {
                name: "scene".into(),
                clear: Some(frame.clear),
                draws: (0..frame.draws.len()).collect(),
                ..Default::default()
            }]
        } else {
            input.passes.clone()
        };
        let mut lowered = vec![RenderPass {
            name: "native-initialize-composition".into(),
            target: Target::Texture(screen),
            clear: Some(frame.clear),
            ..Default::default()
        }];
        let mut snapshots = BTreeMap::<u64, u64>::new();
        for mut pass in passes {
            let target = match pass.target {
                Target::Screen => screen,
                Target::Texture(id) => id,
            };
            pass.target = Target::Texture(target);
            if pass.effect.is_some() {
                lowered.push(pass);
                continue;
            }
            let indices = std::mem::take(&mut pass.draws);
            let mut clear = pass.clear.take();
            let mut segment = Vec::new();
            for index in indices {
                if matches!(
                    frame.draws[index].material.state.blend,
                    BlendState::Custom(_)
                ) {
                    if !segment.is_empty() || clear.is_some() {
                        let mut before = pass.clone();
                        before.name.push_str("/before-blend");
                        before.clear = clear.take();
                        before.draws = std::mem::take(&mut segment);
                        lowered.push(before);
                    }
                    let [w, h, _] = frame.textures[&target].data.dimensions();
                    let snapshot = if let Some(id) = snapshots.get(&target) {
                        *id
                    } else {
                        let id = allocate()?;
                        let format = frame.textures[&target].format;
                        frame.textures.insert(
                            id,
                            Texture::target_with_format(
                                "native-destination-snapshot",
                                w,
                                h,
                                format,
                            ),
                        );
                        snapshots.insert(target, id);
                        id
                    };
                    let mut copy =
                        RenderPass::new("native-destination-snapshot", Target::Texture(snapshot));
                    copy.effect = Some(Effect::Copy { input: target });
                    lowered.push(copy);
                    let material = self.material(&frame.draws[index].material, snapshot, [w, h])?;
                    let mut draw = frame.draws[index].clone();
                    draw.material = material;
                    let new_index = frame.draws.len();
                    frame.draws.push(draw);
                    let mut blend = pass.clone();
                    blend.name.push_str("/covered-fragment-blend");
                    blend.draws = vec![new_index];
                    blend.clear = None;
                    lowered.push(blend);
                } else {
                    segment.push(index);
                }
            }
            if !segment.is_empty() || clear.is_some() {
                pass.draws = segment;
                pass.clear = clear;
                lowered.push(pass);
            }
        }
        let mut present = RenderPass::new("native-present-composition", Target::Screen);
        present.clear = Some(frame.clear);
        present.effect = Some(Effect::Copy { input: screen });
        lowered.push(present);
        frame.passes = lowered;
        frame.validate().map_err(|e| e.to_string())?;
        Ok(frame)
    }
    fn material(
        &mut self,
        input: &wpe_render::Material,
        base: u64,
        size: [u32; 2],
    ) -> Result<wpe_render::Material, String> {
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        size.hash(&mut hash);
        format!("{:?}", input.cpu).hash(&mut hash);
        format!("{:?}", input.state.blend).hash(&mut hash);
        for shader in [&input.vertex, &input.fragment].into_iter().flatten() {
            shader.msl.hash(&mut hash);
            format!("{:?}", shader.default_uniforms).hash(&mut hash);
        }
        for texture in &input.textures {
            texture.name.hash(&mut hash);
            texture.slot.hash(&mut hash);
        }
        let key = hash.finish();
        if let Some(cached) = self.materials.get(&key) {
            let mut material = cached.clone();
            material.uniforms = input.uniforms.clone();
            material.textures = input.textures.clone();
            let mut destination = cached
                .textures
                .last()
                .ok_or("Cached destination binding missing")?
                .clone();
            destination.texture = base;
            let destination_sampler = cached.samplers.get(&destination.slot).cloned();
            material.samplers = input.samplers.clone();
            if let Some(sampler) = destination_sampler {
                material.samplers.insert(destination.slot, sampler);
            }
            material.textures.push(destination);
            material.state = input.state.clone();
            material.state.blend = BlendState::Replace;
            return Ok(material);
        }
        let material = wpe_render::shader::with_destination_blend(input, base, size)
            .map_err(|e| e.to_string())?;
        if self.materials.len() >= 256 {
            self.materials.clear();
        }
        self.materials.insert(key, material.clone());
        Ok(material)
    }
}
pub fn lower_destination_blends(input: &RenderFrame) -> Result<RenderFrame, String> {
    GraphLowerer::default().lower(input)
}
#[cfg(test)]
mod tests {
    use super::*;
    use wpe_render::*;
    #[test]
    fn custom_blend_snapshots_before_covered_draw_without_feedback() {
        let mut f = RenderFrame::new(4, 4);
        f.draws.push(DrawCall {
            material: Material {
                state: PipelineState {
                    blend: BlendState::Custom(5),
                    ..Default::default()
                },
                cpu: CpuProgram::Solid {
                    color: [0.4, 0.2, 0.6, 0.5],
                },
                ..Default::default()
            },
            ..Default::default()
        });
        let lowered = lower_destination_blends(&f).unwrap();
        assert_eq!(lowered.passes.len(), 5);
        assert!(matches!(
            lowered.passes[2].effect,
            Some(Effect::Copy { .. })
        ));
        let draw = &lowered.draws[lowered.passes[3].draws[0]];
        assert_eq!(draw.material.state.blend, BlendState::Replace);
        let Target::Texture(target) = lowered.passes[3].target else {
            panic!()
        };
        assert!(draw.material.textures.iter().all(|t| t.texture != target));
        assert!(
            draw.material
                .fragment
                .as_ref()
                .unwrap()
                .msl
                .contains("fragment")
        );
    }
}

/// Convert the canonical top-down RGBA8 source into the declared GPU storage.
pub fn texture_upload(
    format: wpe_render::TextureFormat,
    rgba: &[u8],
) -> Result<(Vec<u8>, usize), String> {
    use wpe_render::TextureFormat as F;
    if !rgba.len().is_multiple_of(4) {
        return Err("RGBA source byte count is not divisible by4".into());
    }
    if format == F::Depth32Float {
        return Err("Depth upload requires native f32 plane data".into());
    }
    let pixel_bytes = match format {
        F::Rgba8Unorm | F::Bgra8Unorm | F::R32Float => 4,
        F::Rgba16Float => 8,
        F::Rgba32Float => 16,
        F::Rg16Float => 4,
        F::R8Unorm => 1,
        F::R16Float => 2,
        F::Depth32Float => unreachable!(),
    };
    let mut out = Vec::with_capacity(rgba.len() / 4 * pixel_bytes);
    for p in rgba.chunks_exact(4) {
        match format {
            F::Rgba8Unorm => out.extend_from_slice(p),
            F::Bgra8Unorm => out.extend_from_slice(&[p[2], p[1], p[0], p[3]]),
            F::R8Unorm => out.push(p[0]),
            F::Depth32Float => unreachable!(),
            F::Rgba32Float | F::R32Float => {
                for value in p.iter().take(if format == F::R32Float { 1 } else { 4 }) {
                    out.extend_from_slice(&(*value as f32 / 255.).to_ne_bytes());
                }
            }
            F::Rgba16Float | F::Rg16Float | F::R16Float => {
                for value in p.iter().take(if format == F::Rg16Float {
                    2
                } else if format == F::R16Float {
                    1
                } else {
                    4
                }) {
                    out.extend_from_slice(&unorm8_half(*value).to_ne_bytes());
                }
            }
        }
    }
    Ok((out, pixel_bytes))
}
fn unorm8_half(byte: u8) -> u16 {
    let value = byte as f32 / 255.;
    let bits = value.to_bits();
    if byte == 0 {
        return 0;
    }
    let exponent = ((bits >> 23) & 255) as i32 - 127 + 15;
    let mantissa = bits & 0x7fffff;
    let base = ((exponent as u32) << 10) | (mantissa >> 13);
    let remainder = mantissa & 0x1fff;
    (base + u32::from(remainder > 0x1000 || (remainder == 0x1000 && base & 1 != 0))) as u16
}
#[cfg(test)]
mod upload_tests {
    use super::*;
    use wpe_render::TextureFormat as F;
    #[test]
    fn gpu_storage_conversion_swizzles_and_packs_unorm() {
        let rgba = [255, 128, 0, 255];
        assert_eq!(
            texture_upload(F::Bgra8Unorm, &rgba).unwrap().0,
            [0, 128, 255, 255]
        );
        assert_eq!(
            texture_upload(F::Rgba16Float, &rgba).unwrap().0[..2],
            f32_to_half_fixture()
        );
        let (p, bytes) = texture_upload(F::R32Float, &rgba).unwrap();
        assert_eq!(bytes, 4);
        assert_eq!(f32::from_ne_bytes(p.try_into().unwrap()), 1.);
        assert!(texture_upload(F::Rgba8Unorm, &[1, 2, 3]).is_err());
    }
    fn f32_to_half_fixture() -> [u8; 2] {
        0x3c00u16.to_ne_bytes()
    }
}

/// Convert a pointer in the physical view to the scene's aspect-fit canvas.
pub fn fit_pointer(cursor: [f32; 2], view: [f32; 2], canvas: [f32; 2]) -> ([f32; 2], bool) {
    if view
        .iter()
        .chain(&canvas)
        .any(|v| !v.is_finite() || *v <= 0.)
        || cursor.iter().any(|v| !v.is_finite())
    {
        return ([0.; 2], false);
    }
    let scale = (view[0] / canvas[0]).min(view[1] / canvas[1]);
    let size = [canvas[0] * scale, canvas[1] * scale];
    let point = [
        (cursor[0] * view[0] - (view[0] - size[0]) * 0.5) / size[0],
        (cursor[1] * view[1] - (view[1] - size[1]) * 0.5) / size[1],
    ];
    (point, point.iter().all(|v| (0. ..=1.).contains(v)))
}
#[cfg(test)]
mod pointer_tests {
    #[test]
    fn aspect_fit_keeps_letterbox_inactive() {
        let (center, active) = super::fit_pointer([0.5, 0.5], [100., 100.], [200., 100.]);
        assert_eq!(center, [0.5, 0.5]);
        assert!(active);
        assert!(!super::fit_pointer([0.5, 0.1], [100., 100.], [200., 100.]).1);
        assert_eq!(
            super::fit_pointer([0.25, 0.5], [100., 100.], [200., 100.]).0,
            [0.25, 0.5]
        );
        assert!(!super::fit_pointer([0., 0.], [0., 100.], [200., 100.]).1);
    }
}

/// Preserve signed/HDR RGBA components when choosing a float GPU storage format.
pub fn texture_upload_float(
    format: TextureFormat,
    rgba: &[f32],
) -> Result<(Vec<u8>, usize), String> {
    use TextureFormat as F;
    if !rgba.len().is_multiple_of(4) || rgba.iter().any(|v| !v.is_finite()) {
        return Err("Float upload requires finite RGBA components".into());
    }
    let (components, half) = match format {
        F::R16Float => (1, true),
        F::Rg16Float => (2, true),
        F::Rgba16Float => (4, true),
        F::R32Float => (1, false),
        F::Rgba32Float => (4, false),
        _ => return Err("Float planes require a float color texture format".into()),
    };
    let bpp = components * if half { 2 } else { 4 };
    let mut bytes = Vec::with_capacity(rgba.len() / 4 * bpp);
    for pixel in rgba.chunks_exact(4) {
        for v in pixel.iter().take(components) {
            if half {
                bytes.extend_from_slice(&half::f16::from_f32(*v).to_bits().to_ne_bytes());
            } else {
                bytes.extend_from_slice(&v.to_ne_bytes());
            }
        }
    }
    Ok((bytes, bpp))
}
#[cfg(test)]
mod float_upload_tests {
    use super::*;
    #[test]
    fn hdr_and_negative_values_survive_float_upload() {
        let input = [-2., 3.5, 0.33333334, 1.];
        let (bytes, bpp) = texture_upload_float(TextureFormat::Rg16Float, &input).unwrap();
        assert_eq!(bpp, 4);
        assert_eq!(u16::from_ne_bytes(bytes[..2].try_into().unwrap()), 0xc000);
        assert_eq!(u16::from_ne_bytes(bytes[2..].try_into().unwrap()), 0x4300);
        let (bytes, bpp) = texture_upload_float(TextureFormat::Rgba32Float, &input).unwrap();
        assert_eq!(bpp, 16);
        assert_eq!(
            bytes,
            input
                .iter()
                .flat_map(|v| v.to_ne_bytes())
                .collect::<Vec<_>>()
        );
        assert!(texture_upload_float(TextureFormat::Rgba8Unorm, &input).is_err());
        assert!(texture_upload_float(TextureFormat::Rgba16Float, &[f32::NAN, 0., 0., 1.]).is_err());
        assert_eq!(
            texture_upload_float(TextureFormat::R16Float, &[1., 0., 0., 1.]).unwrap(),
            (0x3c00u16.to_ne_bytes().to_vec(), 2)
        );
    }
}

/// Logical resources choose the Material entry. Physical sampler indices are
/// an independent per-stage Metal namespace, limited to0..15.
pub fn resolve_binding<'a>(
    binding: &wpe_render::shader::ShaderBinding,
    material: &'a wpe_render::Material,
) -> Result<Option<&'a wpe_render::TextureBinding>, String> {
    if binding.texture_slot.is_some_and(|slot| slot >= 128)
        || binding.sampler_slot.is_some_and(|slot| slot >= 16)
    {
        return Err("Shader resource exceeds Metal texture128/sampler16 limits".into());
    }
    if binding.texture_slot.is_none() && binding.sampler_slot.is_none() {
        return Ok(None);
    }
    let slot = binding
        .resource_slot
        .ok_or_else(|| format!("{} has no logical resource slot", binding.name))?;
    material
        .textures
        .iter()
        .find(|texture| texture.slot == slot)
        .map(Some)
        .ok_or_else(|| format!("{} is missing logical material texture{slot}", binding.name))
}
#[cfg(test)]
mod binding_tests {
    use super::*;
    use wpe_render::shader::ShaderBinding;
    use wpe_render::{AddressMode, Material, SamplerState, TextureBinding};
    #[test]
    fn high_logical_slots_bind_dense_samplers_without_aliasing_texture_zero() {
        let mut material = Material {
            textures: vec![
                TextureBinding {
                    name: "g_Texture0".into(),
                    slot: 0,
                    texture: 1,
                },
                TextureBinding {
                    name: "shadow".into(),
                    slot: 24,
                    texture: 2,
                },
                TextureBinding {
                    name: "cookie".into(),
                    slot: 25,
                    texture: 3,
                },
            ],
            ..Default::default()
        };
        let custom = SamplerState {
            address_u: AddressMode::Repeat,
            ..Default::default()
        };
        material.samplers.insert(24, custom);
        let mut binding = ShaderBinding {
            name: "g_Texture0".into(),
            group: 0,
            binding: 4,
            resource_slot: Some(24),
            texture_slot: Some(24),
            sampler_slot: Some(2),
            buffer_slot: None,
            dimension: None,
            depth: false,
            comparison: false,
            arrayed: false,
        };
        let selected = resolve_binding(&binding, &material).unwrap().unwrap();
        assert_eq!(selected.texture, 2);
        assert_eq!(
            material.samplers[&selected.slot].address_u,
            AddressMode::Repeat
        );
        binding.resource_slot = Some(25);
        binding.texture_slot = Some(25);
        binding.sampler_slot = Some(3);
        assert_eq!(
            resolve_binding(&binding, &material)
                .unwrap()
                .unwrap()
                .texture,
            3
        );
        binding.sampler_slot = Some(16);
        assert!(resolve_binding(&binding, &material).is_err());
        binding.sampler_slot = Some(3);
        binding.resource_slot = Some(31);
        assert!(resolve_binding(&binding, &material).is_err());
    }
}
