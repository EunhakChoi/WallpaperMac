//! Metal render graph: indexed meshes, translated MSL, explicit samplers,
//! stage-specific uniform layouts, volume textures, offscreen/depth targets.
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_foundation::{MainThreadMarker, NSObject, NSObjectProtocol, NSRect, NSSize, NSString};
use objc2_metal::*;
use objc2_metal_kit::{MTKView, MTKViewDelegate};
use objc2_quartz_core::CAMetalDrawable;
use std::result::Result;
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet, HashMap},
    ffi::c_void,
    hash::{Hash, Hasher},
    ptr::NonNull,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use wpe_render::{
    shader::{CompiledShader, ShaderAttribute},
    *,
};

pub mod offscreen;
pub mod smoke;

type MetalTexture = Retained<ProtocolObject<dyn MTLTexture>>;
type MetalBuffer = Retained<ProtocolObject<dyn MTLBuffer>>;
const _: () =
    assert!(std::mem::size_of::<GpuVertex>() == 64 && std::mem::align_of::<GpuVertex>() == 16);
const BUILTIN: &str = r#"
#include <metal_stdlib>
using namespace metal;
struct Input {float4 position;float4 normal;float4 uv;float4 color;};
struct Varying {float4 position [[position]];float3 normal;float3 uv;float4 color;};
struct Params {float4 tint;float4 light;float4 ambient;float4 lightColor;uint mode;uint slot;float alphaTest;float padding;};
float3 safeNormalize(float3 v){float n=dot(v,v);return n>0.0?v*rsqrt(n):float3(0.0);}
vertex Varying native_vertex(constant Input* vertices [[buffer(30)]],uint id [[vertex_id]]) {Input v=vertices[id];Varying o;o.position=v.position;o.normal=v.normal.xyz;o.uv=v.uv.xyz;o.color=v.color;return o;}
fragment float4 native_fragment(Varying in [[stage_in]],constant Params& p [[buffer(0)]],texture2d<float> image [[texture(0)]],texture3d<float> volume [[texture(1)]],texturecube<float> cube [[texture(2)]],sampler s [[sampler(0)]]) {
float4 c=p.tint*in.color;
if(p.mode==0||p.mode==3||p.mode==6)c*=image.sample(s,in.uv.xy);
if(p.mode==2)c*=volume.sample(s,in.uv);
if(p.mode==5)c*=cube.sample(s,in.uv);
if(p.mode==3||p.mode==4)c.rgb*=p.ambient.rgb+p.lightColor.rgb*max(0.0,dot(safeNormalize(in.normal),safeNormalize(p.light.xyz)));
if(p.alphaTest>=0&&c.a<p.alphaTest)discard_fragment();if(p.mode==6||p.mode==7)return float4(in.position.z,0.0,0.0,1.0);return c;
}
"#;
#[repr(C)]
struct Params {
    tint: [f32; 4],
    light: [f32; 4],
    ambient: [f32; 4],
    light_color: [f32; 4],
    mode: u32,
    slot: u32,
    alpha_test: f32,
    padding: f32,
}
const _: () = assert!(std::mem::size_of::<Params>() == 80);
struct CachedTexture {
    // Keep all source Arcs alive while their pointer identity keys this GPU data.
    _source: Texture,
    key: u64,
    texture: MetalTexture,
}
struct Pipeline {
    state: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    depth: Retained<ProtocolObject<dyn MTLDepthStencilState>>,
}
struct Renderer {
    device: Retained<ProtocolObject<dyn MTLDevice>>,
    queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
    frame: RenderFrame,
    textures: BTreeMap<TextureId, CachedTexture>,
    pipelines: HashMap<u64, Pipeline>,
    sampler_cache: HashMap<u64, Retained<ProtocolObject<dyn MTLSamplerState>>>,
    depths: HashMap<(bool, u64, u32, u32), MetalTexture>,
    fresh_targets: BTreeSet<u64>,
    fresh_depths: BTreeSet<(bool, u64, u32, u32)>,
    gpu_error: Arc<Mutex<Option<String>>>,
    builtin: Retained<ProtocolObject<dyn MTLLibrary>>,
    white2d: MetalTexture,
    white3d: MetalTexture,
    whitecube: MetalTexture,
    default_sampler: Retained<ProtocolObject<dyn MTLSamplerState>>,
    inflight: Arc<AtomicUsize>,
    error: Option<String>,
    hdr_requested: bool,
    lowerer: crate::render_graph::GraphLowerer,
}

define_class!(
 #[unsafe(super=NSObject)]#[thread_kind=MainThreadOnly]#[ivars=RefCell<Renderer>]
 struct MetalDelegate;
 unsafe impl NSObjectProtocol for MetalDelegate{}
 unsafe impl MTKViewDelegate for MetalDelegate{
  #[unsafe(method(drawInMTKView:))]fn draw_frame(&self,view:&MTKView){let hdr=self.ivars().borrow().hdr_requested;configure_hdr_view(view,hdr);let mut renderer=self.ivars().borrow_mut();if let Err(error)=renderer.draw(view){if renderer.error.as_ref()!=Some(&error){eprintln!("Metal: {error}");}renderer.error=Some(error);}}
  #[unsafe(method(mtkView:drawableSizeWillChange:))]fn size_changed(&self,view:&MTKView,_size:NSSize){view.setNeedsDisplay(true);}
 }
);

impl Renderer {
    fn new(frame: RenderFrame) -> Result<Self, String> {
        frame.validate().map_err(|e| e.to_string())?;
        let device = MTLCreateSystemDefaultDevice().ok_or("No Metal device available")?;
        let queue = device
            .newCommandQueue()
            .ok_or("Metal command queue unavailable")?;
        let builtin = device
            .newLibraryWithSource_options_error(
                &NSString::from_str(BUILTIN),
                Some(&compiler_options()),
            )
            .map_err(|e| e.localizedDescription().to_string())?;
        let white2d = white_texture(&device, false)?;
        let white3d = white_texture(&device, true)?;
        let whitecube = white_cube(&device)?;
        let sampler = MTLSamplerDescriptor::new();
        sampler.setMinFilter(MTLSamplerMinMagFilter::Nearest);
        sampler.setMagFilter(MTLSamplerMinMagFilter::Nearest);
        let default_sampler = device
            .newSamplerStateWithDescriptor(&sampler)
            .ok_or("Default Metal sampler allocation failed")?;
        let mut lowerer = crate::render_graph::GraphLowerer::default();
        let frame = lowerer.lower(&frame)?;
        Ok(Renderer {
            device: device.clone(),
            queue,
            frame,
            textures: BTreeMap::new(),
            pipelines: HashMap::new(),
            sampler_cache: HashMap::new(),
            depths: HashMap::new(),
            fresh_targets: BTreeSet::new(),
            fresh_depths: BTreeSet::new(),
            gpu_error: Arc::new(Mutex::new(None)),
            builtin,
            white2d,
            white3d,
            whitecube,
            default_sampler,
            inflight: Arc::new(AtomicUsize::new(0)),
            error: None,
            hdr_requested: false,
            lowerer,
        })
    }
    fn buffer<T>(&self, data: &[T]) -> Result<MetalBuffer, String> {
        if data.is_empty() {
            return Err("Empty GPU buffer".into());
        }
        unsafe {
            self.device.newBufferWithBytes_length_options(
                NonNull::new_unchecked(data.as_ptr().cast_mut().cast::<c_void>()),
                std::mem::size_of_val(data),
                MTLResourceOptions::StorageModeShared,
            )
        }
        .ok_or_else(|| "Metal buffer allocation failed".into())
    }
    fn upload_texture(&self, texture: &Texture) -> Result<MetalTexture, String> {
        texture.validate().map_err(|e| e.to_string())?;
        let [w, h, d] = texture.data.dimensions();
        let descriptor = MTLTextureDescriptor::new();
        descriptor.setPixelFormat(pixel_format(texture.format));
        unsafe {
            descriptor.setWidth(w as usize);
            descriptor.setHeight(h as usize);
            descriptor.setDepth(d as usize);
            descriptor.setMipmapLevelCount(texture.mips.len() + 1);
        }
        descriptor.setTextureType(match texture.data {
            TextureData::Volume3D { .. } => MTLTextureType::Type3D,
            TextureData::CubeMap { .. } => MTLTextureType::TypeCube,
            _ => MTLTextureType::Type2D,
        });
        let target = matches!(texture.data, TextureData::RenderTarget { .. });
        let depth = texture.format == TextureFormat::Depth32Float;
        descriptor.setUsage(if target {
            MTLTextureUsage::ShaderRead | MTLTextureUsage::RenderTarget
        } else {
            MTLTextureUsage::ShaderRead
        });
        descriptor.setStorageMode(if target || depth {
            MTLStorageMode::Private
        } else if self.device.hasUnifiedMemory() {
            MTLStorageMode::Shared
        } else {
            MTLStorageMode::Managed
        });
        if texture.mips.len() > (32 - w.max(h).max(d).leading_zeros()) as usize - 1 {
            return Err("Too many mip levels".into());
        }
        let gpu = self
            .device
            .newTextureWithDescriptor(&descriptor)
            .ok_or("Metal texture allocation failed")?;
        if !target {
            for (level, data) in std::iter::once(&texture.data)
                .chain(&texture.mips)
                .enumerate()
            {
                let [mw, mh, md] = data.dimensions();
                if [mw, mh, md]
                    != [
                        (w >> level).max(1),
                        (h >> level).max(1),
                        (d >> level).max(1),
                    ]
                    || std::mem::discriminant(data) != std::mem::discriminant(&texture.data)
                {
                    return Err("Mip dimensions or texture kind differ from base".into());
                }
                match data {
                    TextureData::Image2D(image) => {
                        self.upload_rgba(&gpu, texture.format, level, 0, [mw, mh, 1], &image.rgba)?
                    }
                    TextureData::Float2D { rgba, .. } => {
                        let (bytes, bpp) =
                            crate::render_graph::texture_upload_float(texture.format, rgba)?;
                        self.upload_bytes(&gpu, level, 0, [mw, mh, 1], &bytes, bpp)?;
                    }
                    TextureData::Volume3D { rgba, .. } => {
                        self.upload_rgba(&gpu, texture.format, level, 0, [mw, mh, md], rgba)?
                    }
                    TextureData::CubeMap { faces } => {
                        for (slice, image) in faces.iter().enumerate() {
                            self.upload_rgba(
                                &gpu,
                                texture.format,
                                level,
                                slice,
                                [mw, mh, 1],
                                &image.rgba,
                            )?;
                        }
                    }
                    TextureData::Depth2D { depth, .. } => {
                        self.upload_depth(&gpu, level, mw, mh, depth)?
                    }
                    TextureData::RenderTarget { .. } => {
                        return Err("Render target used as mip data".into());
                    }
                }
            }
        }
        Ok(gpu)
    }
    fn upload_rgba(
        &self,
        gpu: &ProtocolObject<dyn MTLTexture>,
        format: TextureFormat,
        level: usize,
        slice: usize,
        dims: [u32; 3],
        rgba: &[u8],
    ) -> Result<(), String> {
        let (bytes, pixel_bytes) = crate::render_graph::texture_upload(format, rgba)?;
        self.upload_bytes(gpu, level, slice, dims, &bytes, pixel_bytes)
    }
    fn upload_bytes(
        &self,
        gpu: &ProtocolObject<dyn MTLTexture>,
        level: usize,
        slice: usize,
        dims: [u32; 3],
        bytes: &[u8],
        pixel_bytes: usize,
    ) -> Result<(), String> {
        let [w, h, d] = dims;
        unsafe {
            gpu.replaceRegion_mipmapLevel_slice_withBytes_bytesPerRow_bytesPerImage(
                MTLRegion {
                    origin: MTLOrigin { x: 0, y: 0, z: 0 },
                    size: MTLSize {
                        width: w as usize,
                        height: h as usize,
                        depth: d as usize,
                    },
                },
                level,
                slice,
                NonNull::new_unchecked(bytes.as_ptr().cast_mut().cast()),
                w as usize * pixel_bytes,
                w as usize * h as usize * pixel_bytes,
            );
        }
        Ok(())
    }
    fn upload_depth(
        &self,
        gpu: &ProtocolObject<dyn MTLTexture>,
        level: usize,
        w: u32,
        h: u32,
        depth: &[f32],
    ) -> Result<(), String> {
        let pitch = (w as usize * 4).div_ceil(256) * 256;
        let mut bytes = vec![0u8; pitch * h as usize];
        for y in 0..h as usize {
            for x in 0..w as usize {
                bytes[y * pitch + x * 4..y * pitch + x * 4 + 4]
                    .copy_from_slice(&depth[y * w as usize + x].to_ne_bytes());
            }
        }
        let buffer = self.buffer(&bytes)?;
        let command = self
            .queue
            .commandBuffer()
            .ok_or("Depth upload command allocation failed")?;
        let blit = command
            .blitCommandEncoder()
            .ok_or("Depth upload blit encoder allocation failed")?;
        unsafe {
            blit.copyFromBuffer_sourceOffset_sourceBytesPerRow_sourceBytesPerImage_sourceSize_toTexture_destinationSlice_destinationLevel_destinationOrigin(&buffer,0,pitch,pitch*h as usize,MTLSize{width:w as usize,height:h as usize,depth:1},gpu,0,level,MTLOrigin{x:0,y:0,z:0});
        }
        blit.endEncoding();
        command.commit();
        Ok(())
    }
    fn sync_textures(&mut self) -> Result<(), String> {
        self.textures
            .retain(|id, _| self.frame.textures.contains_key(id));
        for (id, texture) in &self.frame.textures {
            let key = texture_key(texture);
            if self
                .textures
                .get(id)
                .is_some_and(|cached| cached.key == key)
            {
                continue;
            }
            let gpu = self.upload_texture(texture)?;
            if matches!(texture.data, TextureData::RenderTarget { .. }) {
                self.fresh_targets.insert(*id);
                let [w, h, _] = texture.data.dimensions();
                self.fresh_depths.insert((false, *id, w, h));
            }
            self.textures.insert(
                *id,
                CachedTexture {
                    _source: texture.clone(),
                    key,
                    texture: gpu,
                },
            );
        }
        Ok(())
    }
    fn sampler(
        &mut self,
        state: &SamplerState,
    ) -> Result<Retained<ProtocolObject<dyn MTLSamplerState>>, String> {
        if !state.lod_min.is_finite()
            || !state.lod_max.is_finite()
            || state.lod_min < 0.
            || state.lod_max < state.lod_min
            || state.border_color.iter().any(|v| !v.is_finite())
        {
            return Err("Invalid sampler LOD/border values".into());
        }
        let key = hash_debug(state);
        if let Some(sampler) = self.sampler_cache.get(&key) {
            return Ok(sampler.clone());
        }
        let d = MTLSamplerDescriptor::new();
        d.setSAddressMode(address(state.address_u));
        d.setTAddressMode(address(state.address_v));
        d.setRAddressMode(address(state.address_w));
        d.setMinFilter(filter(state.min_filter));
        d.setMagFilter(filter(state.mag_filter));
        d.setMipFilter(match state.mip_filter {
            FilterMode::Nearest => MTLSamplerMipFilter::Nearest,
            FilterMode::Linear => MTLSamplerMipFilter::Linear,
        });
        d.setLodMinClamp(state.lod_min);
        d.setLodMaxClamp(state.lod_max);
        d.setCompareFunction(
            state
                .compare
                .map(compare)
                .unwrap_or(MTLCompareFunction::Never),
        );
        if [state.address_u, state.address_v, state.address_w].contains(&AddressMode::Border) {
            d.setBorderColor(if state.border_color == [0.; 4] {
                MTLSamplerBorderColor::TransparentBlack
            } else if state.border_color == [0., 0., 0., 1.] {
                MTLSamplerBorderColor::OpaqueBlack
            } else if state.border_color == [1.; 4] {
                MTLSamplerBorderColor::OpaqueWhite
            } else {
                return Err("Noncanonical Metal sampler border color".into());
            });
        }
        let sampler = self
            .device
            .newSamplerStateWithDescriptor(&d)
            .ok_or("Metal sampler allocation failed")?;
        if self.sampler_cache.len() >= 512 {
            self.sampler_cache.clear();
        }
        self.sampler_cache.insert(key, sampler.clone());
        Ok(sampler)
    }
    fn pipeline(&mut self, draw: &DrawCall, format: MTLPixelFormat) -> Result<u64, String> {
        let material = &draw.material;
        let key = pipeline_key(draw, format);
        if self.pipelines.contains_key(&key) {
            return Ok(key);
        }
        let function = |shader: &Option<Arc<CompiledShader>>,
                        fallback: &str|
         -> Result<Retained<ProtocolObject<dyn MTLFunction>>, String> {
            let (library, entry) = if let Some(shader) = shader {
                (
                    self.device
                        .newLibraryWithSource_options_error(
                            &NSString::from_str(&shader.msl),
                            Some(&compiler_options()),
                        )
                        .map_err(|e| format!("{}: {}", shader.name, e.localizedDescription()))?,
                    shader.entry_point.as_str(),
                )
            } else {
                (self.builtin.clone(), fallback)
            };
            library
                .newFunctionWithName(&NSString::from_str(entry))
                .ok_or_else(|| format!("MSL entry point missing: {entry}"))
        };
        let descriptor = MTLRenderPipelineDescriptor::new();
        let vertex_function = function(&material.vertex, "native_vertex")?;
        let fragment_function = function(&material.fragment, "native_fragment")?;
        descriptor.setVertexFunction(Some(&vertex_function));
        descriptor.setFragmentFunction(Some(&fragment_function));
        descriptor.setDepthAttachmentPixelFormat(MTLPixelFormat::Depth32Float);
        if let Some(shader) = &material.vertex {
            let vertex = MTLVertexDescriptor::new();
            let mut extra_slot = 2;
            for attribute in &shader.attributes {
                if attribute.location >= 31 {
                    return Err("Vertex attribute location exceeds30".into());
                }
                let attribute_descriptor = unsafe {
                    vertex
                        .attributes()
                        .objectAtIndexedSubscript(attribute.location as usize)
                };
                if let Some((stream, instance)) = stream(draw, &attribute.name) {
                    if extra_slot >= 30 {
                        return Err("More than28 additional vertex streams".into());
                    }
                    if stream.components != attribute.components {
                        return Err(format!(
                            "{} component count differs from shader",
                            attribute.name
                        ));
                    }
                    attribute_descriptor.setFormat(stream_format(stream, attribute.integer)?);
                    unsafe {
                        attribute_descriptor.setOffset(0);
                        attribute_descriptor.setBufferIndex(extra_slot);
                    }
                    let layout = unsafe { vertex.layouts().objectAtIndexedSubscript(extra_slot) };
                    unsafe {
                        layout.setStride(stream.components as usize * 4);
                    }
                    layout.setStepFunction(if instance {
                        MTLVertexStepFunction::PerInstance
                    } else {
                        MTLVertexStepFunction::PerVertex
                    });
                    extra_slot += 1;
                } else {
                    if attribute.location > 3 {
                        return Err(format!(
                            "{} needs missing vertex stream {}",
                            shader.name, attribute.name
                        ));
                    }
                    attribute_descriptor.setFormat(vertex_format(attribute)?);
                    unsafe {
                        attribute_descriptor.setOffset(attribute.location as usize * 16);
                        attribute_descriptor.setBufferIndex(30);
                    }
                }
            }
            let layout = unsafe { vertex.layouts().objectAtIndexedSubscript(30) };
            unsafe {
                layout.setStride(64);
            }
            layout.setStepFunction(MTLVertexStepFunction::PerVertex);
            descriptor.setVertexDescriptor(Some(&vertex));
        }
        let attachment = unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) };
        attachment.setPixelFormat(if format == MTLPixelFormat::Depth32Float {
            MTLPixelFormat::Invalid
        } else {
            format
        });
        let mask = material.state.color_write_mask;
        attachment.setWriteMask(MTLColorWriteMask(
            ((mask & 1) << 3 | (mask & 2) << 1 | (mask & 4) >> 1 | (mask & 8) >> 3) as usize,
        ));
        blend(&attachment, material.state.blend)?;
        let state = self
            .device
            .newRenderPipelineStateWithDescriptor_error(&descriptor)
            .map_err(|e| e.localizedDescription().to_string())?;
        let depth = MTLDepthStencilDescriptor::new();
        depth.setDepthWriteEnabled(material.state.depth_write);
        depth.setDepthCompareFunction(if material.state.depth_test {
            compare(material.state.depth_compare)
        } else {
            MTLCompareFunction::Always
        });
        let depth = self
            .device
            .newDepthStencilStateWithDescriptor(&depth)
            .ok_or("Metal depth-state allocation failed")?;
        self.pipelines.insert(key, Pipeline { state, depth });
        Ok(key)
    }
    fn depth(&mut self, w: u32, h: u32, target: Target) -> Result<(MetalTexture, bool), String> {
        let key = match target {
            Target::Screen => (true, 0, w, h),
            Target::Texture(id) => (false, id, w, h),
        };
        if let Some(texture) = self.depths.get(&key) {
            return Ok((texture.clone(), self.fresh_depths.contains(&key)));
        }
        let descriptor = unsafe {
            MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
                MTLPixelFormat::Depth32Float,
                w as usize,
                h as usize,
                false,
            )
        };
        descriptor.setUsage(MTLTextureUsage::RenderTarget);
        descriptor.setStorageMode(MTLStorageMode::Private);
        let texture = self
            .device
            .newTextureWithDescriptor(&descriptor)
            .ok_or("Metal depth texture allocation failed")?;
        self.depths.insert(key, texture.clone());
        self.fresh_depths.insert(key);
        Ok((texture, true))
    }
    fn encode_frame(
        &mut self,
        command: &ProtocolObject<dyn MTLCommandBuffer>,
        screen_texture: &MetalTexture,
        dimensions: [u32; 2],
    ) -> Result<(), String> {
        let size = NSSize {
            width: dimensions[0] as f64,
            height: dimensions[1] as f64,
        };
        self.frame.validate().map_err(|e| e.to_string())?;
        self.depths.retain(|(screen,id,w,h),_|if *screen{*w==size.width as u32&&*h==size.height as u32}else{self.frame.textures.get(id).is_some_and(|t|matches!(t.data,TextureData::RenderTarget{width,height}if width==*w&&height==*h))});
        self.fresh_targets
            .retain(|id| self.frame.textures.contains_key(id));
        self.fresh_depths
            .retain(|key| self.depths.contains_key(key));
        self.sync_textures()?;
        // New private RT storage is undefined until cleared. This also gives
        // first-frame history reads the CPU reference's transparent-zero value.
        for id in &self.fresh_targets {
            if let Some(texture) = self.textures.get(id) {
                let descriptor = MTLRenderPassDescriptor::renderPassDescriptor();
                if texture.texture.pixelFormat() == MTLPixelFormat::Depth32Float {
                    let depth = descriptor.depthAttachment();
                    depth.setTexture(Some(&texture.texture));
                    depth.setLoadAction(MTLLoadAction::Clear);
                    depth.setStoreAction(MTLStoreAction::Store);
                    depth.setClearDepth(1.);
                } else {
                    let color =
                        unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) };
                    color.setTexture(Some(&texture.texture));
                    color.setLoadAction(MTLLoadAction::Clear);
                    color.setStoreAction(MTLStoreAction::Store);
                    color.setClearColor(MTLClearColor {
                        red: 0.,
                        green: 0.,
                        blue: 0.,
                        alpha: 0.,
                    });
                }
                let encoder = command
                    .renderCommandEncoderWithDescriptor(&descriptor)
                    .ok_or("RT initialization encoder allocation failed")?;
                encoder.endEncoding();
                generate_mips(command, &texture.texture)?;
            }
        }
        let passes = if self.frame.passes.is_empty() {
            vec![RenderPass {
                name: "screen".into(),
                target: Target::Screen,
                clear: Some(self.frame.clear),
                draws: (0..self.frame.draws.len()).collect(),
                effect: None,
                ..Default::default()
            }]
        } else {
            self.frame.passes.clone()
        };
        if !passes.iter().any(|pass| pass.target == Target::Screen) {
            let descriptor = MTLRenderPassDescriptor::renderPassDescriptor();
            let color = unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) };
            color.setTexture(Some(screen_texture));
            color.setLoadAction(MTLLoadAction::Clear);
            color.setStoreAction(MTLStoreAction::Store);
            color.setClearColor(MTLClearColor {
                red: self.frame.clear[0] as f64,
                green: self.frame.clear[1] as f64,
                blue: self.frame.clear[2] as f64,
                alpha: self.frame.clear[3] as f64,
            });
            let encoder = command
                .renderCommandEncoderWithDescriptor(&descriptor)
                .ok_or("Screen initialization encoder unavailable")?;
            encoder.endEncoding();
        }
        let mut screen_seen = false;
        let mut depth_seen = BTreeSet::new();
        for pass in passes {
            let (target, w, h, screen) = match pass.target {
                Target::Screen => (
                    screen_texture.clone(),
                    size.width as u32,
                    size.height as u32,
                    true,
                ),
                Target::Texture(id) => {
                    let texture = self
                        .textures
                        .get(&id)
                        .ok_or("Missing render target")?
                        .texture
                        .clone();
                    let w = texture.width() as u32;
                    let h = texture.height() as u32;
                    (texture, w, h, false)
                }
            };
            let descriptor = MTLRenderPassDescriptor::renderPassDescriptor();
            let clear = pass.clear.or(if screen && !screen_seen {
                Some(self.frame.clear)
            } else {
                None
            });
            let depth_only = target.pixelFormat() == MTLPixelFormat::Depth32Float;
            if !depth_only {
                let attachment =
                    unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) };
                attachment.setTexture(Some(&target));
                attachment.setStoreAction(MTLStoreAction::Store);
                if let Some(clear) = clear {
                    attachment.setLoadAction(MTLLoadAction::Clear);
                    attachment.setClearColor(MTLClearColor {
                        red: clear[0] as f64,
                        green: clear[1] as f64,
                        blue: clear[2] as f64,
                        alpha: clear[3] as f64,
                    });
                } else {
                    attachment.setLoadAction(MTLLoadAction::Load);
                }
            }
            let (depth, fresh_depth) = if depth_only {
                (target.clone(), false)
            } else {
                self.depth(w, h, pass.target)?
            };
            let depth_key = match pass.target {
                Target::Screen => (true, 0, w, h),
                Target::Texture(id) => (false, id, w, h),
            };
            let fresh_depth = fresh_depth && !depth_seen.contains(&depth_key);
            let attachment = descriptor.depthAttachment();
            attachment.setTexture(Some(&depth));
            attachment.setLoadAction(if clear.is_some() || fresh_depth {
                MTLLoadAction::Clear
            } else {
                MTLLoadAction::Load
            });
            attachment.setStoreAction(MTLStoreAction::Store);
            attachment.setClearDepth(if depth_only {
                clear.map(|c| c[0] as f64).unwrap_or(1.)
            } else {
                1.
            });
            let Some(encoder) = command.renderCommandEncoderWithDescriptor(&descriptor) else {
                return Err("Metal render encoder allocation failed".into());
            };
            let (vw, vh) = if screen {
                let scale =
                    (w as f64 / self.frame.width as f64).min(h as f64 / self.frame.height as f64);
                (
                    self.frame.width as f64 * scale,
                    self.frame.height as f64 * scale,
                )
            } else {
                (w as f64, h as f64)
            };
            let fit = MTLViewport {
                originX: (w as f64 - vw) * 0.5,
                originY: (h as f64 - vh) * 0.5,
                width: vw,
                height: vh,
                znear: 0.,
                zfar: 1.,
            };
            if pass.viewports.is_empty() {
                encoder.setViewport(fit);
            } else {
                if pass.viewports.len() > 16 {
                    encoder.endEncoding();
                    return Err("Metal viewport arrays support at most16 viewports".into());
                }
                let mut viewports = pass
                    .viewports
                    .iter()
                    .map(|v| MTLViewport {
                        originX: v.x as f64,
                        originY: v.y as f64,
                        width: v.width as f64,
                        height: v.height as f64,
                        znear: v.z_near as f64,
                        zfar: v.z_far as f64,
                    })
                    .collect::<Vec<_>>();
                unsafe {
                    encoder.setViewports_count(
                        NonNull::new_unchecked(viewports.as_mut_ptr()),
                        viewports.len(),
                    );
                }
            }
            let [x, y, sw, sh] = pass.scissor.unwrap_or([0, 0, w, h]);
            encoder.setScissorRect(MTLScissorRect {
                x: x as usize,
                y: y as usize,
                width: sw as usize,
                height: sh as usize,
            });
            let mut draws = pass
                .draws
                .iter()
                .map(|i| self.frame.draws[*i].clone())
                .collect::<Vec<_>>();
            if let Some(effect) = &pass.effect {
                match wpe_render::effect::draw_call_sized(effect, &self.frame, w, h) {
                    Ok(draw) => draws.insert(0, draw),
                    Err(error) => {
                        encoder.endEncoding();
                        return Err(error.to_string());
                    }
                }
            }
            for draw in draws {
                if let Target::Texture(id) = pass.target
                    && draw.material.textures.iter().any(|t| t.texture == id)
                {
                    encoder.endEncoding();
                    return Err(format!("{} samples its current render target", pass.name));
                }
                if let Err(error) = self.encode_draw(&encoder, &draw, target.pixelFormat()) {
                    encoder.endEncoding();
                    return Err(error);
                }
            }
            encoder.endEncoding();
            depth_seen.insert(depth_key);
            if !screen {
                generate_mips(command, &target)?;
            }
            if screen {
                screen_seen = true;
            }
        }
        Ok(())
    }
    fn draw(&mut self, view: &MTKView) -> Result<(), String> {
        let size = view.drawableSize();
        if !size.width.is_finite()
            || !size.height.is_finite()
            || size.width <= 0.
            || size.height <= 0.
            || self.inflight.load(Ordering::Acquire) >= 3
        {
            return Ok(());
        }
        let Some(drawable) = view.currentDrawable() else {
            return Ok(());
        };
        let command = self
            .queue
            .commandBuffer()
            .ok_or("Metal command allocation failed")?;
        self.encode_frame(
            &command,
            &drawable.texture(),
            [size.width as u32, size.height as u32],
        )?;
        self.inflight.fetch_add(1, Ordering::Release);
        let inflight = self.inflight.clone();
        let gpu_error = self.gpu_error.clone();
        let completion = block2::RcBlock::new(
            move |buffer: NonNull<ProtocolObject<dyn MTLCommandBuffer>>| {
                let buffer = unsafe { buffer.as_ref() };
                if let Some(error) = buffer.error() {
                    let message = format!("Metal command failed: {}", error.localizedDescription());
                    eprintln!("{message}");
                    if let Ok(mut status) = gpu_error.lock() {
                        *status = Some(message);
                    }
                }
                inflight.fetch_sub(1, Ordering::Release);
            },
        );
        unsafe {
            command.addCompletedHandler(block2::RcBlock::as_ptr(&completion));
        }
        command.presentDrawable(ProtocolObject::from_ref(&*drawable));
        command.commit();
        self.fresh_targets.clear();
        self.fresh_depths.clear();
        self.error = None;
        Ok(())
    }
    fn encode_draw(
        &mut self,
        encoder: &ProtocolObject<dyn MTLRenderCommandEncoder>,
        draw: &DrawCall,
        format: MTLPixelFormat,
    ) -> Result<(), String> {
        if draw.indices.is_empty() || draw.instances == 0 {
            return Ok(());
        }
        let depth_draw = if format == MTLPixelFormat::Depth32Float {
            let material = if draw.material.fragment.as_ref().is_some_and(|s| {
                !s.preprocessed_glsl
                    .contains("layout(location=0) out vec4 _wpe_FragColor;")
            }) {
                draw.material.clone()
            } else {
                wpe_render::shader::depth_only_material(&draw.material)
                    .map_err(|e| e.to_string())?
            };
            DrawCall {
                material,
                ..draw.clone()
            }
        } else {
            draw.clone()
        };
        let draw = &depth_draw;
        let key = self.pipeline(draw, format)?;
        let pipeline = &self.pipelines[&key];
        encoder.setRenderPipelineState(&pipeline.state);
        encoder.setDepthStencilState(Some(&pipeline.depth));
        encoder.setCullMode(match draw.material.state.cull {
            CullMode::None => MTLCullMode::None,
            CullMode::Front => MTLCullMode::Front,
            CullMode::Back => MTLCullMode::Back,
        });
        encoder.setFrontFacingWinding(MTLWinding::CounterClockwise);
        let mut vertices = unsafe {
            std::slice::from_raw_parts(
                draw.vertices.as_ptr().cast::<u8>(),
                std::mem::size_of_val(draw.vertices.as_slice()),
            )
        }
        .to_vec();
        if let Some(shader) = &draw.material.vertex {
            for attribute in &shader.attributes {
                if attribute.integer && stream(draw, &attribute.name).is_none() {
                    for (i, vertex) in draw.vertices.iter().enumerate() {
                        let fields = [vertex.position, vertex.normal, vertex.uv, vertex.color];
                        let values = fields
                            .get(attribute.location as usize)
                            .ok_or("Noncanonical integer attribute")?;
                        for (c, value) in values
                            .iter()
                            .take(attribute.components as usize)
                            .enumerate()
                        {
                            if !value.is_finite()
                                || *value < 0.0
                                || *value as f64 > u32::MAX as f64
                                || value.fract() != 0.0
                            {
                                return Err("Integer vertex attribute contains a non-integral/out-of-range value".into());
                            }
                            let offset = i * 64 + attribute.location as usize * 16 + c * 4;
                            vertices[offset..offset + 4]
                                .copy_from_slice(&(*value as u32).to_ne_bytes());
                        }
                    }
                }
            }
        }
        let vertex = self.buffer(&vertices)?;
        let index = self.buffer(&draw.indices)?;
        unsafe {
            encoder.setVertexBuffer_offset_atIndex(Some(&vertex), 0, 30);
        }
        if let Some(shader) = &draw.material.vertex {
            let mut slot = 2;
            for attribute in &shader.attributes {
                if let Some((stream, _)) = stream(draw, &attribute.name) {
                    let buffer = match &stream.data {
                        VertexStreamData::Float32(v) => self.buffer(v)?,
                        VertexStreamData::Uint32(v) => self.buffer(v)?,
                        VertexStreamData::Sint32(v) => self.buffer(v)?,
                    };
                    unsafe {
                        encoder.setVertexBuffer_offset_atIndex(Some(&buffer), 0, slot);
                    }
                    slot += 1;
                }
            }
        }
        for (shader, vertex_stage) in [
            (&draw.material.vertex, true),
            (&draw.material.fragment, false),
        ] {
            if let Some(shader) = shader {
                let uniform = if draw.material.uniforms.bytes.is_empty() {
                    shader
                        .pack_uniforms(&draw.material.uniforms.values)
                        .map_err(|e| e.to_string())?
                } else {
                    draw.material.uniforms.bytes.clone()
                };
                if !uniform.is_empty() {
                    if uniform.len() < shader.uniform_size as usize {
                        return Err("Uniform block is shorter than shader layout".into());
                    }
                    let gpu = self.buffer(&uniform)?;
                    for binding in &shader.bindings {
                        if let Some(slot) = binding.buffer_slot {
                            if slot != 1 {
                                return Err(
                                    "Uniform block collides with vertex buffer slot30".into()
                                );
                            }
                            unsafe {
                                if vertex_stage {
                                    encoder.setVertexBuffer_offset_atIndex(
                                        Some(&gpu),
                                        0,
                                        slot as usize,
                                    );
                                } else {
                                    encoder.setFragmentBuffer_offset_atIndex(
                                        Some(&gpu),
                                        0,
                                        slot as usize,
                                    );
                                }
                            }
                        }
                    }
                }
                for binding in &shader.bindings {
                    let Some(texture) =
                        crate::render_graph::resolve_binding(binding, &draw.material)?
                    else {
                        continue;
                    };
                    let source = self
                        .frame
                        .textures
                        .get(&texture.texture)
                        .ok_or("Missing frame texture")?;
                    validate_binding(binding, &source.data, source.format)?;
                    let gpu = self
                        .textures
                        .get(&texture.texture)
                        .ok_or("Missing GPU texture")?
                        .texture
                        .clone();
                    let mut state = draw
                        .material
                        .samplers
                        .get(&texture.slot)
                        .unwrap_or(&source.sampler)
                        .clone();
                    if binding.comparison && state.compare.is_none() {
                        state.compare = Some(CompareFunction::LessEqual);
                    }
                    let sampler = self.sampler(&state)?;
                    unsafe {
                        if let Some(slot) = binding.texture_slot {
                            if vertex_stage {
                                encoder.setVertexTexture_atIndex(Some(&gpu), slot as usize);
                            } else {
                                encoder.setFragmentTexture_atIndex(Some(&gpu), slot as usize);
                            }
                        }
                        if let Some(slot) = binding.sampler_slot {
                            if vertex_stage {
                                encoder
                                    .setVertexSamplerState_atIndex(Some(&sampler), slot as usize);
                            } else {
                                encoder
                                    .setFragmentSamplerState_atIndex(Some(&sampler), slot as usize);
                            }
                        }
                    }
                }
            }
        }
        if draw.material.fragment.is_none() {
            unsafe {
                encoder.setFragmentTexture_atIndex(Some(&self.white2d), 0);
                encoder.setFragmentTexture_atIndex(Some(&self.white3d), 1);
                encoder.setFragmentTexture_atIndex(Some(&self.whitecube), 2);
                encoder.setFragmentSamplerState_atIndex(Some(&self.default_sampler), 0);
            }
            let (params, slot) = params(&draw.material.cpu)?;
            unsafe {
                encoder.setFragmentBytes_length_atIndex(
                    NonNull::from(&params).cast(),
                    std::mem::size_of::<Params>(),
                    0,
                );
                if let Some(slot) = slot {
                    let binding = draw
                        .material
                        .textures
                        .iter()
                        .find(|t| t.slot == slot)
                        .ok_or("Missing builtin texture slot")?;
                    let gpu = self.textures[&binding.texture].texture.clone();
                    let state = draw
                        .material
                        .samplers
                        .get(&binding.slot)
                        .unwrap_or(&self.frame.textures[&binding.texture].sampler)
                        .clone();
                    let sampler = self.sampler(&state)?;
                    let slot = if params.mode == 2 {
                        1
                    } else if params.mode == 5 {
                        2
                    } else {
                        0
                    };
                    encoder.setFragmentTexture_atIndex(Some(&gpu), slot);
                    encoder.setFragmentSamplerState_atIndex(Some(&sampler), 0);
                }
            }
        }
        unsafe {
            encoder.drawIndexedPrimitives_indexCount_indexType_indexBuffer_indexBufferOffset_instanceCount_baseVertex_baseInstance(MTLPrimitiveType::Triangle,draw.indices.len(),MTLIndexType::UInt32,&index,0,draw.instances as usize,0,draw.first_instance as usize);
        }
        Ok(())
    }
}

pub(super) struct SceneView {
    pub view: Retained<MTKView>,
    delegate: Retained<MetalDelegate>,
    hdr: Cell<bool>,
}
impl SceneView {
    pub fn from_frame(
        frame: RenderFrame,
        rect: NSRect,
        mtm: MainThreadMarker,
    ) -> Result<Self, String> {
        let renderer = Renderer::new(frame)?;
        let device = renderer.device.clone();
        let delegate = MetalDelegate::alloc(mtm).set_ivars(RefCell::new(renderer));
        let delegate: Retained<MetalDelegate> = unsafe { msg_send![super(delegate), init] };
        let view = MTKView::initWithFrame_device(MTKView::alloc(mtm), rect, Some(&device));
        view.setColorPixelFormat(MTLPixelFormat::BGRA8Unorm);
        view.setAutoResizeDrawable(true);
        view.setPaused(true);
        view.setEnableSetNeedsDisplay(true);
        view.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        Ok(Self {
            view,
            delegate,
            hdr: Cell::new(false),
        })
    }
    pub fn set_hdr(&self, requested: bool) {
        self.hdr.set(requested);
        self.delegate.ivars().borrow_mut().hdr_requested = requested;
        self.configure_presentation();
    }
    fn configure_presentation(&self) {
        configure_hdr_view(&self.view, self.hdr.get());
    }
    pub fn presentation_profile(&self) -> &'static str {
        if self.view.colorPixelFormat() == MTLPixelFormat::RGBA16Float {
            "EDR RGBA16Float / extended-linear sRGB"
        } else if self.hdr.get() {
            "SDR BGRA8Unorm clamp fallback: current screen has no potential EDR headroom"
        } else {
            "SDR BGRA8Unorm"
        }
    }
    pub fn submit(&self, frame: RenderFrame) -> Result<(), String> {
        frame.validate().map_err(|e| e.to_string())?;
        {
            let mut renderer = self.delegate.ivars().borrow_mut();
            renderer.frame = renderer.lowerer.lower(&frame)?;
        }
        self.redraw();
        Ok(())
    }
    pub fn canvas_size(&self) -> [f32; 2] {
        let r = self.delegate.ivars().borrow();
        [r.frame.width as f32, r.frame.height as f32]
    }
    pub fn failure(&self) -> Option<String> {
        let renderer = self.delegate.ivars().borrow();
        renderer
            .error
            .clone()
            .or_else(|| renderer.gpu_error.lock().ok().and_then(|e| e.clone()))
    }
    pub fn redraw(&self) {
        self.configure_presentation();
        self.view.setNeedsDisplay(true);
    }
}
impl Drop for SceneView {
    fn drop(&mut self) {
        self.view.setDelegate(None);
    }
}
fn texture_key(t: &Texture) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    std::mem::discriminant(&t.format).hash(&mut h);
    for data in std::iter::once(&t.data).chain(&t.mips) {
        data.dimensions().hash(&mut h);
        match data {
            TextureData::Image2D(i) => (Arc::as_ptr(i) as usize).hash(&mut h),
            TextureData::Volume3D { rgba, .. } => (Arc::as_ptr(rgba) as usize).hash(&mut h),
            TextureData::Float2D { rgba, .. } => (Arc::as_ptr(rgba) as usize).hash(&mut h),
            TextureData::RenderTarget { .. } => 0usize.hash(&mut h),
            TextureData::CubeMap { faces } => {
                for face in faces {
                    (Arc::as_ptr(face) as usize).hash(&mut h);
                }
            }
            TextureData::Depth2D { depth, .. } => (Arc::as_ptr(depth) as usize).hash(&mut h),
        }
    }
    h.finish()
}
fn pipeline_key(draw: &DrawCall, format: MTLPixelFormat) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let m = &draw.material;
    format.0.hash(&mut h);
    for (name, s) in draw.vertex_streams.iter().chain(&draw.instance_streams) {
        name.hash(&mut h);
        s.components.hash(&mut h);
        std::mem::discriminant(&s.data).hash(&mut h);
        draw.instance_streams.contains_key(name).hash(&mut h);
    }
    format!("{:?}", m.state).hash(&mut h);
    for shader in [&m.vertex, &m.fragment].into_iter().flatten() {
        shader.msl.hash(&mut h);
        shader.entry_point.hash(&mut h);
        format!("{:?}", shader.attributes).hash(&mut h);
    }
    h.finish()
}
fn vertex_format(a: &ShaderAttribute) -> Result<MTLVertexFormat, String> {
    match (a.integer, a.components) {
        (false, 1) => Ok(MTLVertexFormat::Float),
        (false, 2) => Ok(MTLVertexFormat::Float2),
        (false, 3) => Ok(MTLVertexFormat::Float3),
        (false, 4) => Ok(MTLVertexFormat::Float4),
        (true, 1) => Ok(MTLVertexFormat::UInt),
        (true, 2) => Ok(MTLVertexFormat::UInt2),
        (true, 3) => Ok(MTLVertexFormat::UInt3),
        (true, 4) => Ok(MTLVertexFormat::UInt4),
        _ => Err("Invalid vertex attribute component count".into()),
    }
}
fn address(a: AddressMode) -> MTLSamplerAddressMode {
    match a {
        AddressMode::Clamp => MTLSamplerAddressMode::ClampToEdge,
        AddressMode::Repeat => MTLSamplerAddressMode::Repeat,
        AddressMode::Mirror => MTLSamplerAddressMode::MirrorRepeat,
        AddressMode::Border => MTLSamplerAddressMode::ClampToBorderColor,
    }
}
fn filter(f: FilterMode) -> MTLSamplerMinMagFilter {
    match f {
        FilterMode::Nearest => MTLSamplerMinMagFilter::Nearest,
        FilterMode::Linear => MTLSamplerMinMagFilter::Linear,
    }
}
fn compare(c: CompareFunction) -> MTLCompareFunction {
    match c {
        CompareFunction::Never => MTLCompareFunction::Never,
        CompareFunction::Less => MTLCompareFunction::Less,
        CompareFunction::Equal => MTLCompareFunction::Equal,
        CompareFunction::LessEqual => MTLCompareFunction::LessEqual,
        CompareFunction::Greater => MTLCompareFunction::Greater,
        CompareFunction::NotEqual => MTLCompareFunction::NotEqual,
        CompareFunction::GreaterEqual => MTLCompareFunction::GreaterEqual,
        CompareFunction::Always => MTLCompareFunction::Always,
    }
}
fn blend(a: &MTLRenderPipelineColorAttachmentDescriptor, state: BlendState) -> Result<(), String> {
    a.setBlendingEnabled(state != BlendState::Replace);
    let (src, dst) = match state {
        BlendState::Replace => (MTLBlendFactor::One, MTLBlendFactor::Zero),
        BlendState::Alpha => (
            MTLBlendFactor::SourceAlpha,
            MTLBlendFactor::OneMinusSourceAlpha,
        ),
        BlendState::Premultiplied => (MTLBlendFactor::One, MTLBlendFactor::OneMinusSourceAlpha),
        BlendState::Additive => (MTLBlendFactor::SourceAlpha, MTLBlendFactor::One),
        BlendState::Multiply => (
            MTLBlendFactor::DestinationColor,
            MTLBlendFactor::OneMinusSourceAlpha,
        ),
        BlendState::Screen => (MTLBlendFactor::One, MTLBlendFactor::OneMinusSourceColor),
        BlendState::Custom(id) => return Err(format!("Unknown Metal blend state{id}")),
    };
    a.setSourceRGBBlendFactor(src);
    a.setDestinationRGBBlendFactor(dst);
    a.setSourceAlphaBlendFactor(MTLBlendFactor::One);
    a.setDestinationAlphaBlendFactor(if state == BlendState::Additive {
        MTLBlendFactor::One
    } else {
        MTLBlendFactor::OneMinusSourceAlpha
    });
    Ok(())
}
fn params(cpu: &CpuProgram) -> Result<(Params, Option<u32>), String> {
    let mut p = Params {
        tint: [1.; 4],
        light: [0., 0., -1., 0.],
        ambient: [1.; 4],
        light_color: [0.; 4],
        mode: 1,
        slot: 0,
        alpha_test: -1.,
        padding: 0.,
    };
    let slot = match cpu {
        CpuProgram::Textured {
            texture_slot,
            color,
            alpha_test,
        } => {
            p.mode = 0;
            p.tint = *color;
            p.alpha_test = alpha_test.unwrap_or(-1.);
            Some(*texture_slot)
        }
        CpuProgram::Solid { color } => {
            p.tint = *color;
            None
        }
        CpuProgram::Volume {
            texture_slot,
            color,
        } => {
            p.mode = 2;
            p.tint = *color;
            Some(*texture_slot)
        }
        CpuProgram::Cube {
            texture_slot,
            color,
        } => {
            p.mode = 5;
            p.tint = *color;
            Some(*texture_slot)
        }
        CpuProgram::Lambert {
            texture_slot,
            light_direction,
            ambient,
            light_color,
        } => {
            p.mode = if texture_slot.is_some() { 3 } else { 4 };
            p.light[..3].copy_from_slice(light_direction);
            p.ambient[..3].copy_from_slice(ambient);
            p.light_color[..3].copy_from_slice(light_color);
            *texture_slot
        }
        CpuProgram::ShadowDepth {
            texture_slot,
            alpha_test,
        } => {
            p.mode = if texture_slot.is_some() { 6 } else { 7 };
            p.alpha_test = alpha_test.unwrap_or(-1.);
            *texture_slot
        }
        CpuProgram::GpuOnly { reason } => {
            return Err(format!("Missing compiled GPU shader: {reason}"));
        }
    };
    Ok((p, slot))
}

fn white_texture(
    device: &ProtocolObject<dyn MTLDevice>,
    volume: bool,
) -> Result<MetalTexture, String> {
    let descriptor = MTLTextureDescriptor::new();
    descriptor.setPixelFormat(MTLPixelFormat::RGBA8Unorm);
    unsafe {
        descriptor.setWidth(1);
    }
    unsafe {
        descriptor.setHeight(1);
    }
    unsafe {
        descriptor.setDepth(1);
    }
    descriptor.setTextureType(if volume {
        MTLTextureType::Type3D
    } else {
        MTLTextureType::Type2D
    });
    descriptor.setUsage(MTLTextureUsage::ShaderRead);
    descriptor.setStorageMode(if device.hasUnifiedMemory() {
        MTLStorageMode::Shared
    } else {
        MTLStorageMode::Managed
    });
    let texture = device
        .newTextureWithDescriptor(&descriptor)
        .ok_or("Fallback Metal texture allocation failed")?;
    let white = [255u8; 4];
    unsafe {
        texture.replaceRegion_mipmapLevel_slice_withBytes_bytesPerRow_bytesPerImage(
            MTLRegion {
                origin: MTLOrigin { x: 0, y: 0, z: 0 },
                size: MTLSize {
                    width: 1,
                    height: 1,
                    depth: 1,
                },
            },
            0,
            0,
            NonNull::new_unchecked(white.as_ptr().cast_mut().cast()),
            4,
            4,
        );
    }
    Ok(texture)
}

fn hash_debug(value: &impl std::fmt::Debug) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{value:?}").hash(&mut h);
    h.finish()
}

fn pixel_format(format: TextureFormat) -> MTLPixelFormat {
    match format {
        TextureFormat::Rgba8Unorm => MTLPixelFormat::RGBA8Unorm,
        TextureFormat::Bgra8Unorm => MTLPixelFormat::BGRA8Unorm,
        TextureFormat::Rgba16Float => MTLPixelFormat::RGBA16Float,
        TextureFormat::Rgba32Float => MTLPixelFormat::RGBA32Float,
        TextureFormat::Rg16Float => MTLPixelFormat::RG16Float,
        TextureFormat::R32Float => MTLPixelFormat::R32Float,
        TextureFormat::R16Float => MTLPixelFormat::R16Float,
        TextureFormat::R8Unorm => MTLPixelFormat::R8Unorm,
        TextureFormat::Depth32Float => MTLPixelFormat::Depth32Float,
    }
}
fn stream<'a>(draw: &'a DrawCall, name: &str) -> Option<(&'a VertexStream, bool)> {
    draw.instance_streams
        .get(name)
        .map(|s| (s, true))
        .or_else(|| draw.vertex_streams.get(name).map(|s| (s, false)))
}
fn stream_format(stream: &VertexStream, integer: bool) -> Result<MTLVertexFormat, String> {
    let kind = match &stream.data {
        VertexStreamData::Float32(_) => 0,
        VertexStreamData::Uint32(_) => 1,
        VertexStreamData::Sint32(_) => 2,
    };
    if integer == (kind == 0) {
        return Err("Vertex stream scalar type differs from shader attribute".into());
    }
    match (kind, stream.components) {
        (0, 1) => Ok(MTLVertexFormat::Float),
        (0, 2) => Ok(MTLVertexFormat::Float2),
        (0, 3) => Ok(MTLVertexFormat::Float3),
        (0, 4) => Ok(MTLVertexFormat::Float4),
        (1, 1) => Ok(MTLVertexFormat::UInt),
        (1, 2) => Ok(MTLVertexFormat::UInt2),
        (1, 3) => Ok(MTLVertexFormat::UInt3),
        (1, 4) => Ok(MTLVertexFormat::UInt4),
        (2, 1) => Ok(MTLVertexFormat::Int),
        (2, 2) => Ok(MTLVertexFormat::Int2),
        (2, 3) => Ok(MTLVertexFormat::Int3),
        (2, 4) => Ok(MTLVertexFormat::Int4),
        _ => Err("Invalid vertex stream format".into()),
    }
}

fn white_cube(device: &ProtocolObject<dyn MTLDevice>) -> Result<MetalTexture, String> {
    let descriptor = MTLTextureDescriptor::new();
    descriptor.setPixelFormat(MTLPixelFormat::RGBA8Unorm);
    descriptor.setTextureType(MTLTextureType::TypeCube);
    unsafe {
        descriptor.setWidth(1);
        descriptor.setHeight(1);
    }
    descriptor.setUsage(MTLTextureUsage::ShaderRead);
    descriptor.setStorageMode(if device.hasUnifiedMemory() {
        MTLStorageMode::Shared
    } else {
        MTLStorageMode::Managed
    });
    let texture = device
        .newTextureWithDescriptor(&descriptor)
        .ok_or("White cube allocation failed")?;
    for slice in 0..6 {
        let bytes = [255u8; 4];
        unsafe {
            texture.replaceRegion_mipmapLevel_slice_withBytes_bytesPerRow_bytesPerImage(
                MTLRegion {
                    origin: MTLOrigin { x: 0, y: 0, z: 0 },
                    size: MTLSize {
                        width: 1,
                        height: 1,
                        depth: 1,
                    },
                },
                0,
                slice,
                NonNull::new_unchecked(bytes.as_ptr().cast_mut().cast()),
                4,
                4,
            );
        }
    }
    Ok(texture)
}

fn validate_binding(
    binding: &wpe_render::shader::ShaderBinding,
    data: &TextureData,
    format: TextureFormat,
) -> Result<(), String> {
    use wpe_render::shader::TextureDimension as D;
    if binding.arrayed {
        return Err(format!(
            "{} requires an unsupported texture array",
            binding.name
        ));
    }
    if (binding.depth || binding.comparison) && format != TextureFormat::Depth32Float {
        return Err(format!("{} expects a depth texture", binding.name));
    }
    let actual = match data {
        TextureData::CubeMap { .. } => D::Cube,
        TextureData::Volume3D { .. } => D::D3,
        _ => D::D2,
    };
    if binding.dimension.as_ref().is_some_and(|d| *d != actual) {
        return Err(format!(
            "{} texture dimensionality differs from shader",
            binding.name
        ));
    }
    Ok(())
}

fn generate_mips(
    command: &ProtocolObject<dyn MTLCommandBuffer>,
    texture: &ProtocolObject<dyn MTLTexture>,
) -> Result<(), String> {
    if texture.mipmapLevelCount() > 1 {
        if texture.pixelFormat() == MTLPixelFormat::Depth32Float {
            return Err("Depth render target mip generation is unavailable".into());
        }
        let encoder = command
            .blitCommandEncoder()
            .ok_or("Mipmap blit encoder allocation failed")?;
        encoder.generateMipmapsForTexture(texture);
        encoder.endEncoding();
    }
    Ok(())
}

fn compiler_options() -> Retained<MTLCompileOptions> {
    let options = MTLCompileOptions::new();
    options.setLanguageVersion(MTLLanguageVersion::Version2_4);
    #[allow(deprecated)]
    options.setFastMathEnabled(false);
    if options.respondsToSelector(sel!(setMathMode:)) {
        options.setMathMode(MTLMathMode::Safe);
    }
    if options.respondsToSelector(sel!(setMathFloatingPointFunctions:)) {
        options.setMathFloatingPointFunctions(MTLMathFloatingPointFunctions::Precise);
    }
    options
}

fn configure_hdr_view(view: &MTKView, requested: bool) {
    let screen = view
        .window()
        .and_then(|window| window.screen())
        .or_else(|| objc2_app_kit::NSScreen::mainScreen(view.mtm()));
    let edr = requested
        && screen.is_some_and(|screen| {
            screen.maximumPotentialExtendedDynamicRangeColorComponentValue() > 1.
        });
    let format = if edr {
        MTLPixelFormat::RGBA16Float
    } else {
        MTLPixelFormat::BGRA8Unorm
    };
    if view.colorPixelFormat() != format {
        view.setColorPixelFormat(format);
    }
    if let Some(layer) = view.layer() {
        // MTKView's documented backing layer is CAMetalLayer. Use a checked
        // downcast rather than passing layer selectors to another class.
        if let Some(layer) = layer.downcast_ref::<objc2_quartz_core::CAMetalLayer>() {
            layer.setWantsExtendedDynamicRangeContent(edr);
            let color = if edr {
                objc2_core_graphics::CGColorSpace::with_name(Some(unsafe {
                    objc2_core_graphics::kCGColorSpaceExtendedLinearSRGB
                }))
            } else {
                None
            };
            layer.setColorspace(color.as_deref());
        }
    }
}
