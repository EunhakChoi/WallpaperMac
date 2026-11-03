//! Headless output through exactly the same render-graph encoder as MTKView.
use super::*;
use std::time::{Duration, Instant};

/// Main-thread renderer. Submit is asynchronous; poll the returned ticket.
pub struct OffscreenRenderer {
    renderer: Renderer,
    output: MetalTexture,
    width: u32,
    height: u32,
    _mtm: MainThreadMarker,
}
/// A bounded GPU submission and its private readback storage. No window is opened.
pub struct GpuReadback {
    command: Retained<ProtocolObject<dyn MTLCommandBuffer>>,
    buffer: MetalBuffer,
    width: u32,
    height: u32,
    pitch: usize,
    started: Instant,
}
impl OffscreenRenderer {
    pub fn new(width: u32, height: u32, mtm: MainThreadMarker) -> Result<Self, String> {
        RenderFrame::new(width, height)
            .validate()
            .map_err(|e| e.to_string())?;
        let renderer = Renderer::new(RenderFrame::new(width, height))?;
        let descriptor = unsafe {
            MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
                MTLPixelFormat::RGBA8Unorm,
                width as usize,
                height as usize,
                false,
            )
        };
        descriptor.setUsage(MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead);
        descriptor.setStorageMode(MTLStorageMode::Private);
        let output = renderer
            .device
            .newTextureWithDescriptor(&descriptor)
            .ok_or("Offscreen RGBA8 target allocation failed")?;
        Ok(Self {
            renderer,
            output,
            width,
            height,
            _mtm: mtm,
        })
    }
    pub fn device_name(&self) -> String {
        self.renderer.device.name().to_string()
    }
    pub fn actual_pass_count(&self) -> usize {
        self.renderer.frame.passes.len()
    }
    pub(super) fn compile_pipeline(&mut self, draw: &DrawCall) -> Result<(), String> {
        self.renderer
            .pipeline(draw, MTLPixelFormat::RGBA8Unorm)
            .map(|_| ())
    }
    pub fn submit(&mut self, frame: RenderFrame) -> Result<GpuReadback, String> {
        if frame.width != self.width || frame.height != self.height {
            return Err("Offscreen frame extent differs from the renderer extent".into());
        }
        if self.renderer.inflight.load(Ordering::Acquire) >= 3 {
            return Err(
                "Three offscreen frames are already in flight; poll a previous ticket".into(),
            );
        }
        self.renderer.frame = self.renderer.lowerer.lower(&frame)?;
        let command = self
            .renderer
            .queue
            .commandBuffer()
            .ok_or("Offscreen command buffer unavailable")?;
        self.renderer
            .encode_frame(&command, &self.output, [self.width, self.height])?;
        let pitch = (self.width as usize)
            .checked_mul(4)
            .and_then(|n| n.checked_add(255))
            .map(|n| n / 256 * 256)
            .ok_or("Readback row size overflow")?;
        let count = pitch
            .checked_mul(self.height as usize)
            .filter(|n| *n <= wpe_formats::MAX_ALLOCATION)
            .ok_or("Readback storage exceeds allocation policy")?;
        let buffer = self
            .renderer
            .device
            .newBufferWithLength_options(count, MTLResourceOptions::StorageModeShared)
            .ok_or("Readback buffer allocation failed")?;
        let blit = command
            .blitCommandEncoder()
            .ok_or("Readback blit encoder unavailable")?;
        unsafe {
            blit.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toBuffer_destinationOffset_destinationBytesPerRow_destinationBytesPerImage(&self.output,0,0,MTLOrigin{x:0,y:0,z:0},MTLSize{width:self.width as usize,height:self.height as usize,depth:1},&buffer,0,pitch,count);
        }
        blit.endEncoding();
        let inflight = self.renderer.inflight.clone();
        inflight.fetch_add(1, Ordering::Release);
        let completion =
            block2::RcBlock::new(move |_: NonNull<ProtocolObject<dyn MTLCommandBuffer>>| {
                inflight.fetch_sub(1, Ordering::Release);
            });
        unsafe {
            command.addCompletedHandler(block2::RcBlock::as_ptr(&completion));
        }
        let started = Instant::now();
        command.commit();
        self.renderer.fresh_targets.clear();
        self.renderer.fresh_depths.clear();
        Ok(GpuReadback {
            command,
            buffer,
            width: self.width,
            height: self.height,
            pitch,
            started,
        })
    }
}
impl GpuReadback {
    /// Literal top-down RGBA attachment bytes, matching the CPU reference.
    /// With opaque output these are straight RGBA. Arbitrary shaders may choose
    /// their own association; this method never guesses and changes RGB.
    pub fn poll(&self) -> Result<Option<RgbaImage>, String> {
        match self.command.status() {
            MTLCommandBufferStatus::Error => {
                return Err(self
                    .command
                    .error()
                    .map(|e| format!("Offscreen Metal error: {}", e.localizedDescription()))
                    .unwrap_or_else(|| "Offscreen Metal command failed".into()));
            }
            MTLCommandBufferStatus::Completed => {}
            _ => {
                if self.started.elapsed() > Duration::from_secs(30) {
                    return Err("Offscreen GPU readback exceeded 30 seconds".into());
                }
                return Ok(None);
            }
        }
        let bytes = self
            .pitch
            .checked_mul(self.height as usize)
            .ok_or("Readback size overflow")?;
        if bytes > self.buffer.length() {
            return Err("Readback buffer is shorter than its row layout".into());
        }
        // Shared storage is CPU-visible after command completion. The retained
        // command/buffer outlive this copy; rows discard only explicit padding.
        let source = unsafe {
            std::slice::from_raw_parts(self.buffer.contents().as_ptr().cast::<u8>(), bytes)
        };
        let mut rgba = Vec::with_capacity(self.width as usize * self.height as usize * 4);
        for row in source.chunks_exact(self.pitch) {
            rgba.extend_from_slice(&row[..self.width as usize * 4]);
        }
        RgbaImage::new(self.width, self.height, rgba)
            .map(Some)
            .map_err(|e| e.to_string())
    }
    /// Explicit conversion only when the caller knows RGB is associated alpha.
    pub fn poll_straight(&self, associated_alpha: bool) -> Result<Option<RgbaImage>, String> {
        let Some(mut image) = self.poll()? else {
            return Ok(None);
        };
        if associated_alpha {
            for pixel in image.rgba.chunks_exact_mut(4) {
                let alpha = pixel[3] as u32;
                for c in &mut pixel[..3] {
                    *c = ((*c as u32 * 255 + alpha / 2)
                        .checked_div(alpha)
                        .unwrap_or(0))
                    .min(255) as u8;
                }
            }
        }
        Ok(Some(image))
    }
}
