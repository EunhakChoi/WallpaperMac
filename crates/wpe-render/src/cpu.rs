//! Perspective-correct indexed triangles, homogeneous clipping, samplers and ordered passes.
use crate::*;
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub(crate) struct Surface {
    pub width: u32,
    pub height: u32,
    pub format: TextureFormat,
    pub pixels: Vec<[f32; 4]>,
    depth: Vec<f32>,
}
impl Surface {
    pub(crate) fn new(width: u32, height: u32, color: [f32; 4]) -> Result<Self> {
        Self::with_format(width, height, color, TextureFormat::Rgba8Unorm)
    }
    fn with_format(
        width: u32,
        height: u32,
        color: [f32; 4],
        format: TextureFormat,
    ) -> Result<Self> {
        let n = (width as usize)
            .checked_mul(height as usize)
            .ok_or_else(|| Error::limit(0, "CPU surface dimensions overflow"))?;
        if n == 0 || n > wpe_formats::MAX_ALLOCATION / 20 {
            return Err(Error::limit(
                0,
                "CPU float surface exceeds 512 MiB allocation policy",
            ));
        }
        Ok(Self {
            width,
            height,
            format,
            pixels: vec![quantize_pixel(format, color); n],
            depth: vec![
                if format == TextureFormat::Depth32Float {
                    color[0]
                } else {
                    1.
                };
                n
            ],
        })
    }
    fn clear(&mut self, color: [f32; 4]) {
        self.pixels.fill(quantize_pixel(self.format, color));
        self.depth
            .fill(if self.format == TextureFormat::Depth32Float {
                color[0]
            } else {
                1.
            });
    }
    fn rgba(&self) -> Result<RgbaImage> {
        let rgba = self
            .pixels
            .iter()
            .flat_map(|p| p.iter().map(|c| (c.clamp(0., 1.) * 255.).round() as u8))
            .collect();
        RgbaImage::new(self.width, self.height, rgba)
    }
}

#[derive(Default)]
pub struct Renderer {
    targets: BTreeMap<TextureId, Surface>,
    target_mips: BTreeMap<TextureId, Vec<Surface>>,
}
impl Renderer {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn reset(&mut self) {
        self.targets.clear();
        self.target_mips.clear();
    }
    pub fn render(&mut self, frame: &RenderFrame) -> Result<RgbaImage> {
        frame.validate()?;
        self.targets.retain(|id,s|frame.textures.get(id).is_some_and(|t|matches!(&t.data,TextureData::RenderTarget{width,height}if *width==s.width&&*height==s.height)&&t.format==s.format));
        self.target_mips
            .retain(|id, _| self.targets.contains_key(id));
        let mut screen = Some(Surface::new(frame.width, frame.height, frame.clear)?);
        if frame.passes.is_empty() {
            for draw in &frame.draws {
                self.draw(
                    frame,
                    draw,
                    screen.as_mut().unwrap(),
                    Viewport::full(frame.width, frame.height),
                    None,
                )?;
            }
        } else {
            for pass in &frame.passes {
                let mut target = match pass.target {
                    Target::Screen => screen.take().unwrap(),
                    Target::Texture(id) => {
                        let [w, h, _] = frame.textures[&id].data.dimensions();
                        let format = frame.textures[&id].format;
                        self.targets.remove(&id).map(Ok).unwrap_or_else(|| {
                            Surface::with_format(
                                w,
                                h,
                                if format == TextureFormat::Depth32Float {
                                    [1., 0., 0., 1.]
                                } else {
                                    [0.; 4]
                                },
                                format,
                            )
                        })?
                    }
                };
                if let Some(color) = pass.clear {
                    target.clear(color);
                }
                if let Some(effect) = &pass.effect {
                    if pass.viewports.iter().any(|v| {
                        v.x != 0.
                            || v.y != 0.
                            || v.width != target.width as f32
                            || v.height != target.height as f32
                    }) {
                        return Err(Error::unsupported(
                            0,
                            "CPU effect profile requires a full-target viewport",
                        ));
                    }
                    let result = crate::effect::evaluate(
                        effect,
                        target.width,
                        target.height,
                        &|id, uv| self.sample_validated(frame, id, uv, 0., None),
                        &|id| frame.textures.get(&id).map(|t| t.data.dimensions()),
                    )?;
                    let [x, y, w, h] = pass.scissor.unwrap_or([0, 0, target.width, target.height]);
                    for yy in y..y + h {
                        for xx in x..x + w {
                            let i = yy as usize * target.width as usize + xx as usize;
                            target.pixels[i] = quantize_pixel(target.format, result[i]);
                        }
                    }
                }
                let viewport = pass
                    .viewports
                    .first()
                    .copied()
                    .unwrap_or_else(|| Viewport::full(target.width, target.height));
                for index in &pass.draws {
                    self.draw(
                        frame,
                        &frame.draws[*index],
                        &mut target,
                        viewport,
                        pass.scissor,
                    )?;
                }
                match pass.target {
                    Target::Screen => screen = Some(target),
                    Target::Texture(id) => {
                        let mut mips = Vec::new();
                        let mut source = &target;
                        for data in &frame.textures[&id].mips {
                            let [w, h, _] = data.dimensions();
                            let mut mip = Surface::with_format(w, h, [0.; 4], target.format)?;
                            for y in 0..h {
                                for x in 0..w {
                                    let mut color = [0.; 4];
                                    for dy in 0..2 {
                                        for dx in 0..2 {
                                            let pixel = source.pixels[((y * 2 + dy)
                                                .min(source.height - 1)
                                                as usize
                                                * source.width as usize)
                                                + (x * 2 + dx).min(source.width - 1) as usize];
                                            for c in 0..4 {
                                                color[c] += pixel[c] * 0.25;
                                            }
                                        }
                                    }
                                    mip.pixels[y as usize * w as usize + x as usize] =
                                        quantize_pixel(mip.format, color);
                                }
                            }
                            mips.push(mip);
                            source = mips.last().unwrap();
                        }
                        self.target_mips.insert(id, mips);
                        self.targets.insert(id, target);
                    }
                }
            }
        }
        screen.unwrap().rgba()
    }
    pub fn target_rgba(&self, id: TextureId) -> Result<RgbaImage> {
        self.targets
            .get(&id)
            .ok_or_else(|| Error::parse(0, "render target has not been written"))?
            .rgba()
    }
    pub fn sample(
        &self,
        frame: &RenderFrame,
        id: TextureId,
        uv: [f32; 3],
        lod: f32,
    ) -> Result<[f32; 4]> {
        let texture = frame
            .textures
            .get(&id)
            .ok_or_else(|| Error::parse(0, format!("missing texture {id}")))?;
        texture.validate()?;
        self.sample_validated(frame, id, uv, lod, None)
    }
    pub fn sample_with_sampler(
        &self,
        frame: &RenderFrame,
        id: TextureId,
        uv: [f32; 3],
        lod: f32,
        sampler: &SamplerState,
    ) -> Result<[f32; 4]> {
        frame
            .textures
            .get(&id)
            .ok_or_else(|| Error::parse(0, "missing texture"))?
            .validate()?;
        sampler.validate()?;
        self.sample_validated(frame, id, uv, lod, Some(sampler))
    }
    fn sample_validated(
        &self,
        frame: &RenderFrame,
        id: TextureId,
        uv: [f32; 3],
        lod: f32,
        override_sampler: Option<&SamplerState>,
    ) -> Result<[f32; 4]> {
        let texture = frame
            .textures
            .get(&id)
            .ok_or_else(|| Error::parse(0, format!("missing texture {id}")))?;
        let sampler = override_sampler.unwrap_or(&texture.sampler);
        let minifying = lod > 0.;
        let max_level = texture.mips.len();
        let lod = if lod.is_finite() {
            lod.max(sampler.lod_min)
                .min(sampler.lod_max)
                .clamp(0., max_level as f32)
        } else {
            0.
        };
        let filter = if minifying {
            sampler.min_filter
        } else {
            sampler.mag_filter
        };
        let level = |i: usize| -> Result<[f32; 4]> {
            let data = if i == 0 {
                &texture.data
            } else {
                &texture.mips[i - 1]
            };
            match data {
                TextureData::Float2D {
                    width,
                    height,
                    rgba,
                } => Ok(sample_grid(
                    [*width, *height, 1],
                    sampler,
                    uv,
                    filter,
                    |x, y, _| {
                        let at = (y as usize * *width as usize + x as usize) * 4;
                        quantize_pixel(texture.format, std::array::from_fn(|c| rgba[at + c]))
                    },
                )),
                TextureData::Image2D(image) => Ok(sample_grid(
                    [image.width, image.height, 1],
                    sampler,
                    uv,
                    filter,
                    |x, y, _| {
                        let n = (y as usize * image.width as usize + x as usize) * 4;
                        quantize_pixel(
                            texture.format,
                            std::array::from_fn(|c| image.rgba[n + c] as f32 / 255.),
                        )
                    },
                )),
                TextureData::Volume3D {
                    width,
                    height,
                    depth,
                    rgba,
                } => Ok(sample_grid(
                    [*width, *height, *depth],
                    sampler,
                    uv,
                    filter,
                    |x, y, z| {
                        let n = ((z as usize * *height as usize + y as usize) * *width as usize
                            + x as usize)
                            * 4;
                        quantize_pixel(
                            texture.format,
                            std::array::from_fn(|c| rgba[n + c] as f32 / 255.),
                        )
                    },
                )),
                TextureData::CubeMap { faces } => {
                    let (face, coord) = cube_coordinates(uv)?;
                    let image = &faces[face];
                    let mut sampler = sampler.clone();
                    sampler.address_u = AddressMode::Clamp;
                    sampler.address_v = AddressMode::Clamp;
                    Ok(sample_grid(
                        [image.width, image.height, 1],
                        &sampler,
                        [coord[0], coord[1], 0.],
                        filter,
                        |x, y, _| {
                            let n = (y as usize * image.width as usize + x as usize) * 4;
                            quantize_pixel(
                                texture.format,
                                std::array::from_fn(|c| image.rgba[n + c] as f32 / 255.),
                            )
                        },
                    ))
                }
                TextureData::Depth2D {
                    width,
                    height,
                    depth,
                } => Ok(sample_grid(
                    [*width, *height, 1],
                    sampler,
                    uv,
                    filter,
                    |x, y, _| [depth[y as usize * *width as usize + x as usize], 0., 0., 1.],
                )),
                TextureData::RenderTarget { width, height } => Ok(
                    if let Some(surface) = if i == 0 {
                        self.targets.get(&id)
                    } else {
                        self.target_mips
                            .get(&id)
                            .and_then(|levels| levels.get(i - 1))
                    } {
                        sample_grid([*width, *height, 1], sampler, uv, filter, |x, y, _| {
                            surface.pixels[y as usize * *width as usize + x as usize]
                        })
                    } else if texture.format == TextureFormat::Depth32Float {
                        [1., 0., 0., 1.]
                    } else {
                        [0.; 4]
                    },
                ),
            }
        };
        if sampler.mip_filter == FilterMode::Nearest {
            return level((lod + 0.5).floor() as usize);
        }
        let low = lod.floor() as usize;
        let high = (low + 1).min(max_level);
        let a = level(low)?;
        if high == low {
            return Ok(a);
        }
        let b = level(high)?;
        Ok(std::array::from_fn(|i| a[i] + (b[i] - a[i]) * lod.fract()))
    }
    /// Compare reference depth before bilinear filtering (PCF), matching a comparison sampler.
    pub fn sample_compare(
        &self,
        frame: &RenderFrame,
        id: TextureId,
        uv: [f32; 2],
        reference: f32,
        lod: usize,
    ) -> Result<f32> {
        if !reference.is_finite() {
            return Err(Error::parse(0, "nonfinite comparison reference"));
        }
        let texture = frame
            .textures
            .get(&id)
            .ok_or_else(|| Error::parse(0, "missing comparison texture"))?;
        texture.validate()?;
        if texture.format != TextureFormat::Depth32Float {
            return Err(Error::parse(0, "comparison sampling requires Depth32Float"));
        }
        let data = if lod == 0 {
            &texture.data
        } else {
            texture
                .mips
                .get(lod - 1)
                .ok_or_else(|| Error::parse(0, "depth mip outside texture"))?
        };
        let [w, h, _] = data.dimensions();
        let comparison = texture
            .sampler
            .compare
            .unwrap_or(CompareFunction::LessEqual);
        let fetch = |x: u32, y: u32| -> Result<f32> {
            match data {
                TextureData::Depth2D { depth, .. } => {
                    Ok(depth[y as usize * w as usize + x as usize])
                }
                TextureData::RenderTarget { .. } if lod == 0 => Ok(self
                    .targets
                    .get(&id)
                    .map(|s| s.depth[y as usize * w as usize + x as usize])
                    .unwrap_or(1.)),
                _ => Err(Error::unsupported(
                    0,
                    "comparison source must be depth data or a depth render target",
                )),
            }
        };
        let mut sampler = texture.sampler.clone();
        let border = if compare(comparison, reference, sampler.border_color[0]) {
            1.
        } else {
            0.
        };
        sampler.border_color = [border; 4];
        // Validate the source kind before entering the infallible texel callback.
        let _ = fetch(0, 0)?;
        Ok(sample_grid(
            [w, h, 1],
            &sampler,
            [uv[0], uv[1], 0.],
            if lod == 0 {
                sampler.mag_filter
            } else {
                sampler.min_filter
            },
            |x, y, _| {
                [if compare(comparison, reference, fetch(x, y).unwrap()) {
                    1.
                } else {
                    0.
                }; 4]
            },
        )[0])
    }
    fn draw(
        &self,
        frame: &RenderFrame,
        draw: &DrawCall,
        target: &mut Surface,
        viewport: Viewport,
        scissor: Option<[u32; 4]>,
    ) -> Result<()> {
        if draw.instances == 0 {
            return Ok(());
        }
        if let CpuProgram::GpuOnly { reason } = &draw.material.cpu {
            return Err(Error::unsupported(
                0,
                format!("{} has no CPU program: {reason}", draw.name),
            ));
        }
        for instance in 0..draw.instances {
            for triangle in draw.indices.chunks_exact(3) {
                let poly = clip([triangle[0], triangle[1], triangle[2]].map(|index| {
                    apply_streams(
                        draw,
                        index as usize,
                        (draw.first_instance + instance) as usize,
                    )
                }));
                for i in 1..poly.len().saturating_sub(1) {
                    self.triangle(
                        frame,
                        draw,
                        [poly[0], poly[i], poly[i + 1]],
                        target,
                        viewport,
                        scissor,
                    )?;
                }
            }
        }
        Ok(())
    }
    fn triangle(
        &self,
        frame: &RenderFrame,
        draw: &DrawCall,
        mut v: [GpuVertex; 3],
        target: &mut Surface,
        viewport: Viewport,
        scissor: Option<[u32; 4]>,
    ) -> Result<()> {
        if v.iter().any(|v| v.position[3] <= 0.) {
            return Ok(());
        }
        let screen = |v: &GpuVertex| {
            [
                viewport.x + (v.position[0] / v.position[3] * 0.5 + 0.5) * viewport.width,
                viewport.y + (0.5 - v.position[1] / v.position[3] * 0.5) * viewport.height,
            ]
        };
        let mut p = v.map(|v| screen(&v));
        let mut area = edge(p[0], p[1], p[2]);
        if area == 0. {
            return Ok(());
        }
        let front = area < 0.;
        if (draw.material.state.cull == CullMode::Front && front)
            || (draw.material.state.cull == CullMode::Back && !front)
        {
            return Ok(());
        }
        if area < 0. {
            v.swap(1, 2);
            p.swap(1, 2);
            area = -area;
        }
        let [sx, sy, sw, sh] = scissor.unwrap_or([0, 0, target.width, target.height]);
        let min_x = (p
            .iter()
            .map(|p| p[0])
            .fold(f32::INFINITY, f32::min)
            .floor()
            .max(0.) as u32)
            .max(sx);
        let max_x = (p
            .iter()
            .map(|p| p[0])
            .fold(f32::NEG_INFINITY, f32::max)
            .ceil()
            .clamp(0., target.width as f32) as u32)
            .min(sx + sw);
        let min_y = (p
            .iter()
            .map(|p| p[1])
            .fold(f32::INFINITY, f32::min)
            .floor()
            .max(0.) as u32)
            .max(sy);
        let max_y = (p
            .iter()
            .map(|p| p[1])
            .fold(f32::NEG_INFINITY, f32::max)
            .ceil()
            .clamp(0., target.height as f32) as u32)
            .min(sy + sh);
        let pairs = [(p[1], p[2]), (p[2], p[0]), (p[0], p[1])];
        let top = pairs.map(|(a, b)| b[1] < a[1] || (b[1] == a[1] && b[0] > a[0]));
        let weights = |point: [f32; 2]| pairs.map(|(a, b)| (edge(a, b, point) / area) as f32);
        let interpolate = |weights: [f32; 3]| {
            let iw = std::array::from_fn::<_, 3, _>(|i| weights[i] / v[i].position[3]);
            let sum = iw.iter().sum::<f32>();
            let field = |f: fn(&GpuVertex) -> [f32; 4]| {
                std::array::from_fn(|c| (0..3).map(|i| f(&v[i])[c] * iw[i]).sum::<f32>() / sum)
            };
            GpuVertex {
                position: [0.; 4],
                normal: field(|v| v.normal),
                uv: field(|v| v.uv),
                color: field(|v| v.color),
            }
        };
        for y in min_y..max_y {
            for x in min_x..max_x {
                let point = [x as f32 + 0.5, y as f32 + 0.5];
                let es = pairs.map(|(a, b)| edge(a, b, point));
                if (0..3).any(|i| es[i] < 0. || (es[i] == 0. && !top[i])) {
                    continue;
                }
                let w = es.map(|e| (e / area) as f32);
                let depth = viewport.z_near
                    + (0..3)
                        .map(|i| w[i] * v[i].position[2] / v[i].position[3])
                        .sum::<f32>()
                        * (viewport.z_far - viewport.z_near);
                let index = y as usize * target.width as usize + x as usize;
                if draw.material.state.depth_test
                    && !compare(
                        draw.material.state.depth_compare,
                        depth,
                        target.depth[index],
                    )
                {
                    continue;
                }
                let sample = interpolate(w);
                let dx = interpolate(weights([point[0] + 1., point[1]]));
                let dy = interpolate(weights([point[0], point[1] + 1.]));
                let texel = |slot: u32| -> Result<[f32; 4]> {
                    let id = draw
                        .material
                        .textures
                        .iter()
                        .find(|b| b.slot == slot)
                        .ok_or_else(|| {
                            Error::parse(0, format!("{} missing texture slot {slot}", draw.name))
                        })?
                        .texture;
                    let dims = frame.textures[&id].data.dimensions();
                    let rho = |other: &GpuVertex| {
                        (0..3)
                            .map(|i| ((other.uv[i] - sample.uv[i]) * dims[i] as f32).powi(2))
                            .sum::<f32>()
                            .sqrt()
                    };
                    let lod = rho(&dx).max(rho(&dy)).max(1e-8).log2();
                    self.sample_validated(
                        frame,
                        id,
                        [sample.uv[0], sample.uv[1], sample.uv[2]],
                        lod,
                        draw.material.samplers.get(&slot),
                    )
                };
                let (mut color, alpha_test) = match &draw.material.cpu {
                    CpuProgram::Textured {
                        texture_slot,
                        color,
                        alpha_test,
                    } => {
                        let s = texel(*texture_slot)?;
                        (std::array::from_fn(|i| s[i] * color[i]), *alpha_test)
                    }
                    CpuProgram::Solid { color } => (*color, None),
                    CpuProgram::ShadowDepth {
                        texture_slot,
                        alpha_test,
                    } => {
                        let alpha = if let Some(slot) = texture_slot {
                            texel(*slot)?[3]
                        } else {
                            1.
                        };
                        if alpha_test.is_some_and(|limit| alpha * sample.color[3] < limit) {
                            continue;
                        }
                        ([depth, 0., 0., 1.], None)
                    }
                    CpuProgram::Volume {
                        texture_slot,
                        color,
                    }
                    | CpuProgram::Cube {
                        texture_slot,
                        color,
                    } => {
                        let s = texel(*texture_slot)?;
                        (std::array::from_fn(|i| s[i] * color[i]), None)
                    }
                    CpuProgram::Lambert {
                        texture_slot,
                        light_direction,
                        ambient,
                        light_color,
                    } => {
                        let s = if let Some(slot) = texture_slot {
                            texel(*slot)?
                        } else {
                            [1.; 4]
                        };
                        let n = normalize([sample.normal[0], sample.normal[1], sample.normal[2]]);
                        let l = normalize(*light_direction);
                        let diffuse = (n[0] * l[0] + n[1] * l[1] + n[2] * l[2]).max(0.);
                        (
                            [
                                s[0] * (ambient[0] + light_color[0] * diffuse),
                                s[1] * (ambient[1] + light_color[1] * diffuse),
                                s[2] * (ambient[2] + light_color[2] * diffuse),
                                s[3],
                            ],
                            None,
                        )
                    }
                    CpuProgram::GpuOnly { .. } => unreachable!(),
                };
                if !matches!(draw.material.cpu, CpuProgram::ShadowDepth { .. }) {
                    for (c, vertex) in color.iter_mut().zip(sample.color) {
                        *c *= vertex;
                    }
                }
                if alpha_test.is_some_and(|limit| color[3] < limit) {
                    continue;
                }
                let blended = crate::blend::composite(
                    draw.material.state.blend,
                    target.pixels[index],
                    color,
                )?;
                if target.format != TextureFormat::Depth32Float {
                    for (c, value) in blended.iter().enumerate() {
                        if draw.material.state.color_write_mask & (1 << c) != 0 {
                            target.pixels[index][c] = *value;
                        }
                    }
                    target.pixels[index] = quantize_pixel(target.format, target.pixels[index]);
                }
                if draw.material.state.depth_write {
                    target.depth[index] = depth;
                    if target.format == TextureFormat::Depth32Float {
                        target.pixels[index] = [depth, 0., 0., 1.];
                    }
                }
            }
        }
        Ok(())
    }
}
fn apply_streams(draw: &DrawCall, index: usize, instance: usize) -> GpuVertex {
    let mut vertex = draw.vertices[index];
    for (streams, row) in [
        (&draw.vertex_streams, index),
        (&draw.instance_streams, instance),
    ] {
        for (name, stream) in streams {
            let field = match name.as_str() {
                "a_Position" => &mut vertex.position,
                "a_Normal" => &mut vertex.normal,
                "a_TexCoord" | "a_TexCoord0" | "a_TexCoordVec4" => &mut vertex.uv,
                "a_Color" => &mut vertex.color,
                _ => continue,
            };
            *field = [
                0.,
                0.,
                0.,
                if name == "a_Position" || name == "a_Color" {
                    1.
                } else {
                    0.
                },
            ];
            for (c, value) in field
                .iter_mut()
                .enumerate()
                .take(stream.components as usize)
            {
                let i = row * stream.components as usize + c;
                *value = match &stream.data {
                    VertexStreamData::Float32(v) => v[i],
                    VertexStreamData::Uint32(v) => v[i] as f32,
                    VertexStreamData::Sint32(v) => v[i] as f32,
                };
            }
        }
    }
    vertex
}
pub fn quantize_pixel(format: TextureFormat, pixel: [f32; 4]) -> [f32; 4] {
    let unorm = |x: f32| (x.clamp(0., 1.) * 255.).round() / 255.;
    match format {
        TextureFormat::Rgba8Unorm | TextureFormat::Bgra8Unorm => pixel.map(unorm),
        TextureFormat::Rgba16Float => pixel.map(half_round),
        TextureFormat::Rgba32Float => pixel,
        TextureFormat::Rg16Float => [half_round(pixel[0]), half_round(pixel[1]), 0., 1.],
        TextureFormat::R16Float => [half_round(pixel[0]), 0., 0., 1.],
        TextureFormat::R32Float | TextureFormat::Depth32Float => [pixel[0], 0., 0., 1.],
        TextureFormat::R8Unorm => [unorm(pixel[0]), 0., 0., 1.],
    }
}
fn half_round(value: f32) -> f32 {
    let bits = value.to_bits();
    let sign = (bits >> 16) & 0x8000;
    let exp = ((bits >> 23) & 255) as i32;
    let mant = bits & 0x7fffff;
    let half = if exp == 255 {
        sign | 0x7c00 | (u32::from(mant != 0) * 0x200)
    } else {
        let mut e = exp - 127 + 15;
        if e >= 31 {
            sign | 0x7c00
        } else if e <= 0 {
            if e < -10 {
                sign
            } else {
                let m = mant | 0x800000;
                let shift = (14 - e) as u32;
                let mut q = m >> shift;
                let remainder = m & ((1 << shift) - 1);
                let halfway = 1 << (shift - 1);
                if remainder > halfway || (remainder == halfway && q & 1 != 0) {
                    q += 1;
                }
                sign | q
            }
        } else {
            let mut q = mant >> 13;
            let remainder = mant & 8191;
            if remainder > 4096 || (remainder == 4096 && q & 1 != 0) {
                q += 1;
                if q == 1024 {
                    q = 0;
                    e += 1;
                }
            }
            sign | ((e as u32) << 10) | q
        }
    };
    let sign = (half & 0x8000) << 16;
    let exp = (half >> 10) & 31;
    let mut mant = half & 1023;
    f32::from_bits(if exp == 31 {
        sign | 0x7f800000 | (mant << 13)
    } else if exp == 0 {
        if mant == 0 {
            sign
        } else {
            let mut e = -14;
            while mant & 1024 == 0 {
                mant <<= 1;
                e -= 1;
            }
            sign | (((e + 127) as u32) << 23) | ((mant & 1023) << 13)
        }
    } else {
        sign | ((exp + 112) << 23) | (mant << 13)
    })
}
pub fn render(frame: &RenderFrame) -> Result<RgbaImage> {
    Renderer::new().render(frame)
}
pub fn sample_texture(
    frame: &RenderFrame,
    id: TextureId,
    uv: [f32; 3],
    lod: f32,
) -> Result<[f32; 4]> {
    Renderer::new().sample(frame, id, uv, lod)
}
pub fn sample_compare(
    frame: &RenderFrame,
    id: TextureId,
    uv: [f32; 2],
    reference: f32,
    lod: usize,
) -> Result<f32> {
    Renderer::new().sample_compare(frame, id, uv, reference, lod)
}
fn cube_coordinates(direction: [f32; 3]) -> Result<(usize, [f32; 2])> {
    let [x, y, z] = direction;
    let a = [x.abs(), y.abs(), z.abs()];
    if !a.iter().all(|x| x.is_finite()) || a.iter().all(|x| *x == 0.) {
        return Err(Error::parse(0, "cube direction must be finite and nonzero"));
    }
    let (face, s, t, major) = if a[0] >= a[1] && a[0] >= a[2] {
        if x >= 0. {
            (0, -z, -y, a[0])
        } else {
            (1, z, -y, a[0])
        }
    } else if a[1] >= a[2] {
        if y >= 0. {
            (2, x, z, a[1])
        } else {
            (3, x, -z, a[1])
        }
    } else if z >= 0. {
        (4, x, -y, a[2])
    } else {
        (5, -x, -y, a[2])
    };
    Ok((face, [s / major * 0.5 + 0.5, t / major * 0.5 + 0.5]))
}
fn normalize(v: [f32; 3]) -> [f32; 3] {
    let n = (v.iter().map(|v| v * v).sum::<f32>()).sqrt();
    if n > 0. { v.map(|x| x / n) } else { [0.; 3] }
}
fn compare(mode: CompareFunction, a: f32, b: f32) -> bool {
    match mode {
        CompareFunction::Never => false,
        CompareFunction::Less => a < b,
        CompareFunction::Equal => a == b,
        CompareFunction::LessEqual => a <= b,
        CompareFunction::Greater => a > b,
        CompareFunction::NotEqual => a != b,
        CompareFunction::GreaterEqual => a >= b,
        CompareFunction::Always => true,
    }
}
fn edge(a: [f32; 2], b: [f32; 2], p: [f32; 2]) -> f64 {
    // Coverage needs consistent signs when a shared edge is traversed in
    // opposite directions. Float32 products caused both triangles of an
    // offset quad to cover the same diagonal pixels. Widen before subtraction
    // and multiplication; interpolation remains the public float32 profile.
    let a = a.map(f64::from);
    let b = b.map(f64::from);
    let p = p.map(f64::from);
    (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
}
fn mix_vertex(a: GpuVertex, b: GpuVertex, t: f32) -> GpuVertex {
    let lerp = |a: [f32; 4], b: [f32; 4]| std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t);
    GpuVertex {
        position: lerp(a.position, b.position),
        normal: lerp(a.normal, b.normal),
        uv: lerp(a.uv, b.uv),
        color: lerp(a.color, b.color),
    }
}
fn clip(triangle: [GpuVertex; 3]) -> Vec<GpuVertex> {
    let mut polygon = triangle.to_vec();
    for plane in 0..6 {
        let distance = |v: &GpuVertex| {
            let [x, y, z, w] = v.position;
            match plane {
                0 => x + w,
                1 => w - x,
                2 => y + w,
                3 => w - y,
                4 => z,
                _ => w - z,
            }
        };
        let input = std::mem::take(&mut polygon);
        if input.is_empty() {
            break;
        }
        let mut a = *input.last().unwrap();
        let mut da = distance(&a);
        for b in input {
            let db = distance(&b);
            if (da >= 0.) != (db >= 0.) {
                polygon.push(mix_vertex(a, b, da / (da - db)));
            }
            if db >= 0. {
                polygon.push(b);
            }
            a = b;
            da = db;
        }
    }
    polygon
}
fn address(index: i64, n: u32, mode: AddressMode) -> Option<u32> {
    let n = n as i64;
    Some(match mode {
        AddressMode::Clamp => index.clamp(0, n - 1),
        AddressMode::Repeat => index.rem_euclid(n),
        AddressMode::Mirror => {
            let v = index.rem_euclid(n * 2);
            if v < n { v } else { n * 2 - 1 - v }
        }
        AddressMode::Border => {
            if index < 0 || index >= n {
                return None;
            }
            index
        }
    } as u32)
}
fn sample_grid(
    dims: [u32; 3],
    sampler: &SamplerState,
    uv: [f32; 3],
    filter: FilterMode,
    fetch: impl Fn(u32, u32, u32) -> [f32; 4],
) -> [f32; 4] {
    let modes = [sampler.address_u, sampler.address_v, sampler.address_w];
    let texel = |indices: [i64; 3]| {
        let ids = std::array::from_fn::<_, 3, _>(|i| address(indices[i], dims[i], modes[i]));
        match ids {
            [Some(x), Some(y), Some(z)] => fetch(x, y, z),
            _ => sampler.border_color,
        }
    };
    if filter == FilterMode::Nearest {
        return texel(std::array::from_fn(|i| {
            (uv[i] * dims[i] as f32).floor() as i64
        }));
    }
    let active = [true, true, dims[2] > 1];
    let p = std::array::from_fn::<_, 3, _>(|i| {
        if !active[i] {
            0.
        } else {
            uv[i] * dims[i] as f32 - 0.5
        }
    });
    let base = p.map(|p| p.floor() as i64);
    let frac = p.map(|p| p - p.floor());
    let mut out = [0.; 4];
    for z in 0..=usize::from(active[2]) {
        for y in 0..=1 {
            for x in 0..=1 {
                let bits = [x, y, z];
                let w = (0..3)
                    .map(|i| {
                        if !active[i] {
                            1.
                        } else if bits[i] == 0 {
                            1. - frac[i]
                        } else {
                            frac[i]
                        }
                    })
                    .product::<f32>();
                let value = texel(std::array::from_fn(|i| base[i] + bits[i] as i64));
                for i in 0..4 {
                    out[i] += value[i] * w;
                }
            }
        }
    }
    out
}
