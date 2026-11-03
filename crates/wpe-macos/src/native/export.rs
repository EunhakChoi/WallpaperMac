//! Offline scene export through the shared Metal render graph and real media
//! writers. Encoders never produce a completion result before the file closes.
use super::metal::offscreen::{GpuReadback, OffscreenRenderer};
use super::*;
use objc2::{AnyThread, runtime::AnyObject};
use objc2_av_foundation::*;
use objc2_core_foundation::CFRetained;
use objc2_core_media::CMTime;
use objc2_core_video::*;
use objc2_foundation::{NSDictionary, NSNumber};
use std::{
    ptr::NonNull,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};
use wpe_platform::export::{ExportSettings, FfmpegEncoder, FrameAccumulator};
use wpe_render::{RgbaImage, Target};
struct AvEncoder {
    writer: Retained<AVAssetWriter>,
    input: Retained<AVAssetWriterInput>,
    adaptor: Retained<AVAssetWriterInputPixelBufferAdaptor>,
    finished: Arc<AtomicBool>,
    finishing: bool,
    path: PathBuf,
}
impl AvEncoder {
    fn new(path: &Path, settings: &ExportSettings) -> Result<Self, String> {
        unsafe {
            let url = NSURL::fileURLWithPath_isDirectory(
                &NSString::from_str(&path.to_string_lossy()),
                false,
            );
            let writer = AVAssetWriter::initWithURL_fileType_error(
                AVAssetWriter::alloc(),
                &url,
                AVFileTypeMPEG4.ok_or("MPEG4 writer type unavailable")?,
            )
            .map_err(|e| e.localizedDescription().to_string())?;
            let width = NSNumber::new_u32(settings.width);
            let height = NSNumber::new_u32(settings.height);
            let bitrate = NSNumber::new_u32(settings.bitrate);
            let rate = NSNumber::new_u32(settings.fps);
            let compression = NSDictionary::from_slices(
                &[
                    AVVideoAverageBitRateKey.ok_or("Bitrate key unavailable")?,
                    AVVideoExpectedSourceFrameRateKey.ok_or("Frame rate key unavailable")?,
                ],
                &[&*bitrate as &AnyObject, &*rate as &AnyObject],
            );
            let values: [&AnyObject; 4] = [
                AVVideoCodecTypeH264.ok_or("H264 encoder unavailable")?,
                &width,
                &height,
                &compression,
            ];
            let output = NSDictionary::from_slices(
                &[
                    AVVideoCodecKey.ok_or("Codec key unavailable")?,
                    AVVideoWidthKey.ok_or("Width key unavailable")?,
                    AVVideoHeightKey.ok_or("Height key unavailable")?,
                    AVVideoCompressionPropertiesKey.ok_or("Compression key unavailable")?,
                ],
                &values,
            );
            let media = AVMediaTypeVideo.ok_or("Video media type unavailable")?;
            if !writer.canApplyOutputSettings_forMediaType(Some(&output), media) {
                return Err(
                    "AVFoundation cannot apply the requested H264 settings on this Mac".into(),
                );
            }
            let input = AVAssetWriterInput::assetWriterInputWithMediaType_outputSettings(
                media,
                Some(&output),
            );
            input.setExpectsMediaDataInRealTime(false);
            if !writer.canAddInput(&input) {
                return Err("AVFoundation cannot add the video writer input".into());
            }
            writer.addInput(&input);
            let format = NSNumber::new_u32(kCVPixelFormatType_32BGRA);
            let key = NSString::from_str(&kCVPixelBufferPixelFormatTypeKey.to_string());
            let attributes = NSDictionary::from_slices(&[&*key], &[&*format as &AnyObject]);
            let adaptor=AVAssetWriterInputPixelBufferAdaptor::assetWriterInputPixelBufferAdaptorWithAssetWriterInput_sourcePixelBufferAttributes(&input,Some(&attributes));
            if !writer.startWriting() {
                return Err(writer
                    .error()
                    .map(|e| e.localizedDescription().to_string())
                    .unwrap_or("Video writer did not start".into()));
            }
            writer.startSessionAtSourceTime(CMTime::with_seconds(0., 60000));
            Ok(Self {
                writer,
                input,
                adaptor,
                finished: Arc::new(AtomicBool::new(false)),
                finishing: false,
                path: path.to_owned(),
            })
        }
    }
    fn append(&self, image: &RgbaImage, index: u32, fps: u32) -> Result<bool, String> {
        unsafe {
            if let Some(error) = self.writer.error() {
                return Err(error.localizedDescription().to_string());
            }
            if !self.input.isReadyForMoreMediaData() {
                return Ok(false);
            }
            let mut pointer = std::ptr::null_mut();
            let status = CVPixelBufferCreate(
                None,
                image.width as usize,
                image.height as usize,
                kCVPixelFormatType_32BGRA,
                None,
                NonNull::from(&mut pointer),
            );
            if status != 0 {
                return Err(format!("CVPixelBuffer allocation failed ({status})"));
            }
            let buffer = NonNull::new(pointer)
                .map(|p| CFRetained::from_raw(p))
                .ok_or("Encoder pixel buffer is null")?;
            if CVPixelBufferLockBaseAddress(&buffer, CVPixelBufferLockFlags::empty()) != 0 {
                return Err("Encoder pixel buffer lock failed".into());
            }
            struct Unlock<'a>(&'a CVPixelBuffer);
            impl Drop for Unlock<'_> {
                fn drop(&mut self) {
                    unsafe {
                        CVPixelBufferUnlockBaseAddress(self.0, CVPixelBufferLockFlags::empty());
                    }
                }
            }
            let _unlock = Unlock(&buffer);
            let stride = CVPixelBufferGetBytesPerRow(&buffer);
            let base = CVPixelBufferGetBaseAddress(&buffer).cast::<u8>();
            if base.is_null() || stride < image.width as usize * 4 {
                return Err("Encoder pixel buffer layout is invalid".into());
            }
            for y in 0..image.height as usize {
                let row =
                    std::slice::from_raw_parts_mut(base.add(y * stride), image.width as usize * 4);
                for (p, q) in image.rgba
                    [y * image.width as usize * 4..(y + 1) * image.width as usize * 4]
                    .chunks_exact(4)
                    .zip(row.chunks_exact_mut(4))
                {
                    q.copy_from_slice(&[p[2], p[1], p[0], 255]);
                }
            }
            let time = CMTime {
                value: index as i64,
                timescale: fps as i32,
                flags: objc2_core_media::CMTimeFlags::Valid,
                epoch: 0,
            };
            if !self
                .adaptor
                .appendPixelBuffer_withPresentationTime(&buffer, time)
            {
                return Err(self
                    .writer
                    .error()
                    .map(|e| e.localizedDescription().to_string())
                    .unwrap_or("Video frame append failed".into()));
            }
            Ok(true)
        }
    }
    fn finish(&mut self, frames: u32, fps: u32) {
        if self.finishing {
            return;
        }
        self.finishing = true;
        unsafe {
            self.input.markAsFinished();
            self.writer.endSessionAtSourceTime(CMTime {
                value: frames as i64,
                timescale: fps as i32,
                flags: objc2_core_media::CMTimeFlags::Valid,
                epoch: 0,
            });
            let flag = self.finished.clone();
            let writer = self.writer.clone();
            let complete = block2::RcBlock::new(move || {
                let _ = &writer;
                flag.store(true, Ordering::Release);
            });
            self.writer.finishWritingWithCompletionHandler(&complete);
        }
    }
    fn complete(&self) -> Result<bool, String> {
        unsafe {
            if let Some(error) = self.writer.error() {
                return Err(error.localizedDescription().to_string());
            }
            Ok(self.finished.load(Ordering::Acquire)
                && self.writer.status() == AVAssetWriterStatus::Completed)
        }
    }
    fn cancel(&self) {
        unsafe {
            if self.writer.status() == AVAssetWriterStatus::Writing {
                self.writer.cancelWriting();
            }
        }
        let _ = std::fs::remove_file(&self.path);
    }
}
impl Drop for AvEncoder {
    fn drop(&mut self) {
        if !self.complete().unwrap_or(false) {
            self.cancel();
        }
    }
}
enum Encoder {
    Av(AvEncoder),
    Ffmpeg(FfmpegEncoder),
    Png,
}
pub(super) struct ExportJob {
    settings: ExportSettings,
    player: wpe_core::ScenePlayer,
    text: super::text::NativeText,
    videos: super::export_video::OfflineVideos,
    renderer: OffscreenRenderer,
    pending: Option<GpuReadback>,
    ready: Option<RgbaImage>,
    accumulator: FrameAccumulator,
    encoder: Encoder,
    frame: u32,
    sample: u64,
    partial: PathBuf,
    owned_directory: PathBuf,
    output: PathBuf,
    finishing: Option<Instant>,
    done: bool,
    cancelled: bool,
    error: Option<String>,
    started: Instant,
    input: PlayerInput,
}
impl ExportJob {
    pub fn new(
        project: wpe_core::Project,
        settings: ExportSettings,
        ffmpeg: Option<&Path>,
        mut initial_input: PlayerInput,
        mtm: MainThreadMarker,
    ) -> Result<Self, String> {
        settings.validate().map_err(|e| e.to_string())?;
        let output = settings
            .output
            .clone()
            .ok_or("Choose an export output path")?;
        if output.exists() {
            return Err("Choose a new output path; existing files are preserved".into());
        }
        let parent = output.parent().ok_or("Output has no parent directory")?;
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let mut reservation = None;
        for _ in 0..32 {
            let dir = parent.join(format!(
                ".wallpaper-export-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match std::fs::create_dir(&dir) {
                Ok(()) => {
                    reservation = Some(ExportDirectory(Some(dir)));
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.to_string()),
            }
        }
        let mut reservation =
            reservation.ok_or("Could not reserve a new export staging directory")?;
        let owned_directory = reservation
            .0
            .as_ref()
            .ok_or("Export directory reservation failed")?
            .clone();
        let partial = owned_directory.join(format!("render.{}", settings.extension()));
        let renderer = OffscreenRenderer::new(settings.width, settings.height, mtm)?;
        initial_input.screen_resolution = [settings.width as f32, settings.height as f32];
        initial_input.pointer_active = false;
        initial_input.buttons = 0;
        initial_input.keys.clear();
        initial_input.general_settings = json!({"fps":settings.fps});
        if !settings.live_inputs {
            initial_input.audio_left.clear();
            initial_input.audio_right.clear();
            initial_input.media = wpe_platform::media::unavailable(false);
        }
        let mut player = wpe_core::ScenePlayer::new_with_context(
            project,
            wpe_core::ScenePlayerOptions {
                is_editor: true,
                is_wallpaper: false,
                is_desktop: false,
            },
            wpe_core::ScenePlayerInitialState {
                input: initial_input.clone(),
                ..Default::default()
            },
        )
        .map_err(|e| e.to_string())?;
        player.set_native_text_provider(true);
        let encoder = match settings.format.as_str() {
            "mp4_h264" => Encoder::Av(AvEncoder::new(&partial, &settings)?),
            "png" => {
                std::fs::create_dir(&partial).map_err(|e| e.to_string())?;
                Encoder::Png
            }
            _ => Encoder::Ffmpeg(
                FfmpegEncoder::new(
                    ffmpeg
                        .ok_or("Configure an external FFmpeg executable for this export format")?,
                    &partial,
                    &settings,
                )
                .map_err(|e| e.to_string())?,
            ),
        };
        let accumulator = FrameAccumulator::new(&settings).map_err(|e| e.to_string())?;
        reservation.0.take();
        Ok(Self {
            settings,
            player,
            text: Default::default(),
            videos: Default::default(),
            renderer,
            pending: None,
            ready: None,
            accumulator,
            encoder,
            frame: 0,
            sample: 0,
            partial,
            owned_directory,
            output,
            finishing: None,
            done: false,
            cancelled: false,
            error: None,
            started: Instant::now(),
            input: initial_input,
        })
    }
    pub fn is_active(&self) -> bool {
        !self.done && !self.cancelled && self.error.is_none()
    }
    pub fn status(&self) -> Value {
        json!({"progress":self.frame as f64/self.settings.frames().max(1) as f64,"frames":self.frame,"total":self.settings.frames(),"samples":self.sample,"complete":self.done,"cancelled":self.cancelled,"error":self.error,"output":self.output,"profile":"opaque fixed-time video/PNG, no sound playback or media audio track","external_inputs":if self.settings.live_inputs{"sampled live system input at export wall-clock speed"}else{"frozen start-time context; silence / no media"},"device":self.renderer.device_name(),"elapsed":self.started.elapsed().as_secs_f64()})
    }
    pub fn uses_live_inputs(&self) -> bool {
        self.is_active() && self.settings.live_inputs
    }
    pub fn tick(&mut self, live: &PlayerInput) {
        if self.settings.live_inputs {
            self.input.audio_left = live.audio_left.clone();
            self.input.audio_right = live.audio_right.clone();
            self.input.media = live.media.clone();
            self.input.time_of_day = live.time_of_day;
        }

        if self.done || self.error.is_some() || self.cancelled {
            return;
        }
        if let Err(error) = self.step() {
            self.error = Some(error);
            self.cancel_resources();
        }
    }
    fn step(&mut self) -> Result<(), String> {
        if let Some(start) = self.finishing {
            let complete = match &self.encoder {
                Encoder::Av(av) => av.complete()?,
                Encoder::Ffmpeg(ff) => {
                    let status = ff.status();
                    if let Some(error) = status.error {
                        return Err(error);
                    }
                    status.finished
                }
                Encoder::Png => true,
            };
            if complete {
                if matches!(self.encoder, Encoder::Png) {
                    if self.output.exists() {
                        return Err("Output directory appeared while exporting".into());
                    }
                    std::fs::rename(&self.partial, &self.output).map_err(|e| e.to_string())?;
                } else {
                    std::fs::hard_link(&self.partial,&self.output).map_err(|e|format!("Could not publish completed video without replacing an existing file: {e}"))?;
                    std::fs::remove_file(&self.partial).map_err(|e| e.to_string())?;
                }
                self.done = true;
            } else if start.elapsed().as_secs() > 30 {
                return Err("Encoder finalization exceeded30s".into());
            }
            return Ok(());
        }
        if let Some(mut image) = self.ready.take() {
            let accepted = match &self.encoder {
                Encoder::Av(av) => av.append(&image, self.frame, self.settings.fps)?,
                Encoder::Ffmpeg(ff) => {
                    if let Some(bytes) = ff
                        .submit(std::mem::take(&mut image.rgba))
                        .map_err(|e| e.to_string())?
                    {
                        self.ready = Some(RgbaImage {
                            rgba: bytes,
                            ..image
                        });
                        return Ok(());
                    }
                    true
                }
                Encoder::Png => {
                    image::save_buffer_with_format(
                        self.partial.join(format!("frame-{:06}.png", self.frame)),
                        &image.rgba,
                        image.width,
                        image.height,
                        image::ColorType::Rgba8,
                        image::ImageFormat::Png,
                    )
                    .map_err(|e| e.to_string())?;
                    true
                }
            };
            if !accepted {
                self.ready = Some(image);
                return Ok(());
            }
            self.frame += 1;
            if self.frame >= self.settings.frames() {
                match &mut self.encoder {
                    Encoder::Av(av) => av.finish(self.frame, self.settings.fps),
                    Encoder::Ffmpeg(ff) => ff.finish(),
                    Encoder::Png => {}
                }
                self.finishing = Some(Instant::now());
            }
            return Ok(());
        }
        if let Some(readback) = &self.pending {
            if let Some(image) = readback.poll()? {
                self.pending.take();
                self.ready = self.accumulator.push(&image).map_err(|e| e.to_string())?;
            }
            return Ok(());
        }
        let rate = self.settings.fps as f32 * self.settings.blend as f32;
        let input = self.input.clone();
        let mut frame = self
            .player
            .advance(if self.sample == 0 { 0. } else { 1. / rate }, &input)
            .map_err(|e| e.to_string())?;
        self.text.update(&mut self.player, &mut frame)?;
        self.player.drain_host_commands();
        if self.videos.update(
            &mut self.player,
            if self.sample == 0 { 0. } else { 1. / rate },
        )? {
            frame = self
                .player
                .render_current(&input)
                .map_err(|e| e.to_string())?;
            self.text.update(&mut self.player, &mut frame)?;
        }
        frame.clear[3] = 1.;
        for pass in &mut frame.passes {
            if pass.target == Target::Screen
                && let Some(clear) = &mut pass.clear
            {
                clear[3] = 1.;
            }
        }
        self.pending = Some(self.renderer.submit(frame)?);
        self.sample += 1;
        Ok(())
    }
    pub fn cancel(&mut self) {
        if self.done {
            return;
        }
        self.cancelled = true;
        self.cancel_resources();
    }
    fn cancel_resources(&mut self) {
        self.pending.take();
        self.ready.take();
        match &mut self.encoder {
            Encoder::Av(av) => av.cancel(),
            Encoder::Ffmpeg(ff) => ff.cancel(),
            Encoder::Png => {}
        }
        if self.partial.is_dir() {
            let _ = std::fs::remove_dir_all(&self.partial);
        } else {
            let _ = std::fs::remove_file(&self.partial);
        }
    }
}
impl Drop for ExportJob {
    fn drop(&mut self) {
        if !self.done {
            self.cancel_resources();
        }
        let _ = std::fs::remove_dir_all(&self.owned_directory);
    }
}
struct ExportDirectory(Option<PathBuf>);
impl Drop for ExportDirectory {
    fn drop(&mut self) {
        if let Some(directory) = self.0.take() {
            let _ = std::fs::remove_dir_all(directory);
        }
    }
}
