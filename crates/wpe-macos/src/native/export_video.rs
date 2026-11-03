//! Time-indexed AVAssetReader decoder for offline embedded video textures.
//! Decodes actual samples; never advances a real-time AVPlayer or plays audio.
use super::*;
use objc2::{rc::Retained, runtime::AnyObject};
use objc2_av_foundation::*;
use objc2_core_video::*;
use objc2_foundation::{NSDictionary, NSNumber};
use std::{
    io::Write,
    sync::atomic::{AtomicU64, Ordering},
};
use wpe_formats::RgbaImage;
struct Decoder {
    asset: Retained<AVURLAsset>,
    track: Retained<AVAssetTrack>,
    reader: Retained<AVAssetReader>,
    output: Retained<AVAssetReaderTrackOutput>,
    path: PathBuf,
    duration: f64,
    fps: f32,
    current: Option<(f64, RgbaImage)>,
    next: Option<(f64, RgbaImage)>,
    time: f64,
    last_feedback: f64,
    eof: bool,
}
impl Decoder {
    #[allow(deprecated)]
    fn new(name: &str, bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() < 12 || bytes.len() > 256 * 1024 * 1024 {
            return Err(format!(
                "{name}: offline video payload is truncated or exceeds256MiB"
            ));
        }
        let extension = if matches!(
            &bytes[4..8],
            b"ftyp" | b"moov" | b"mdat" | b"wide" | b"free"
        ) {
            "mov"
        } else if bytes[..4] == [0x1a, 0x45, 0xdf, 0xa3] {
            "webm"
        } else if &bytes[..4] == b"RIFF" && &bytes[8..12] == b"AVI " {
            "avi"
        } else {
            return Err(format!("{name}: unrecognized embedded video container"));
        };
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "WallpaperMac-Offline-{}-{}.{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
            extension
        ));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| e.to_string())?;
        file.write_all(bytes).map_err(|e| {
            let _ = std::fs::remove_file(&path);
            e.to_string()
        })?;
        drop(file);
        let result = (|| unsafe {
            let url = NSURL::fileURLWithPath_isDirectory(
                &NSString::from_str(&path.to_string_lossy()),
                false,
            );
            let asset = AVURLAsset::URLAssetWithURL_options(&url, None);
            let kind = AVMediaTypeVideo.ok_or("Video media type unavailable")?;
            let track = asset
                .tracksWithMediaType(kind)
                .firstObject()
                .ok_or("Embedded container has no video track")?;
            let duration = asset.duration().seconds();
            if !duration.is_finite() || duration <= 0. {
                return Err("Embedded video duration is not finite and positive".into());
            }
            let fps = track.nominalFrameRate();
            let (reader, output) = Self::reader(&asset, &track)?;
            Ok(Self {
                asset,
                track,
                reader,
                output,
                path: path.clone(),
                duration,
                fps,
                current: None,
                next: None,
                time: 0.,
                last_feedback: 0.,
                eof: false,
            })
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(path);
        }
        result
    }
    unsafe fn reader(
        asset: &AVURLAsset,
        track: &AVAssetTrack,
    ) -> Result<(Retained<AVAssetReader>, Retained<AVAssetReaderTrackOutput>), String> {
        unsafe {
            let reader = AVAssetReader::assetReaderWithAsset_error(asset)
                .map_err(|e| e.localizedDescription().to_string())?;
            let key = NSString::from_str(&kCVPixelBufferPixelFormatTypeKey.to_string());
            let format = NSNumber::new_u32(kCVPixelFormatType_32BGRA);
            let settings = NSDictionary::<NSString, AnyObject>::from_slices(&[&*key], &[&*format]);
            let output = AVAssetReaderTrackOutput::assetReaderTrackOutputWithTrack_outputSettings(
                track,
                Some(&settings),
            );
            if !reader.canAddOutput(&output) {
                return Err("AVAssetReader cannot decode this video track to BGRA".into());
            }
            reader.addOutput(&output);
            if !reader.startReading() {
                return Err(reader
                    .error()
                    .map(|e| e.localizedDescription().to_string())
                    .unwrap_or("Offline decoder did not start".into()));
            }
            Ok((reader, output))
        }
    }
    fn read_next(&mut self) -> Result<Option<(f64, RgbaImage)>, String> {
        unsafe {
            for _ in 0..1024 {
                let Some(sample) = self.output.copyNextSampleBuffer() else {
                    if self.reader.status() == AVAssetReaderStatus::Failed {
                        return Err(self
                            .reader
                            .error()
                            .map(|e| e.localizedDescription().to_string())
                            .unwrap_or("Offline video decode failed".into()));
                    }
                    self.eof = true;
                    return Ok(None);
                };
                let Some(buffer) = sample.image_buffer() else {
                    continue;
                };
                let buffer: &CVPixelBuffer = &buffer;
                let time = sample.presentation_time_stamp().seconds();
                if !time.is_finite() {
                    return Err("Decoded sample has a nonfinite timestamp".into());
                }
                if CVPixelBufferGetPixelFormatType(buffer) != kCVPixelFormatType_32BGRA {
                    return Err("Offline decoder did not return BGRA pixels".into());
                }
                let flags = CVPixelBufferLockFlags::ReadOnly;
                if CVPixelBufferLockBaseAddress(buffer, flags) != 0 {
                    return Err("Offline video pixel buffer lock failed".into());
                }
                struct Unlock<'a>(&'a CVPixelBuffer);
                impl Drop for Unlock<'_> {
                    fn drop(&mut self) {
                        unsafe {
                            CVPixelBufferUnlockBaseAddress(
                                self.0,
                                CVPixelBufferLockFlags::ReadOnly,
                            );
                        }
                    }
                }
                let _unlock = Unlock(buffer);
                let (w, h, stride) = (
                    CVPixelBufferGetWidth(buffer),
                    CVPixelBufferGetHeight(buffer),
                    CVPixelBufferGetBytesPerRow(buffer),
                );
                let count = w
                    .checked_mul(h)
                    .and_then(|n| n.checked_mul(4))
                    .filter(|n| *n <= wpe_formats::MAX_ALLOCATION)
                    .ok_or("Offline video frame exceeds allocation policy")?;
                let base = CVPixelBufferGetBaseAddress(buffer).cast::<u8>();
                if w == 0 || h == 0 || base.is_null() || stride < w * 4 {
                    return Err("Offline video pixel layout is invalid".into());
                }
                let mut rgba = vec![0; count];
                for y in 0..h {
                    let row = std::slice::from_raw_parts(base.add(y * stride), w * 4);
                    for (x, p) in row.chunks_exact(4).enumerate() {
                        rgba[(y * w + x) * 4..(y * w + x + 1) * 4]
                            .copy_from_slice(&[p[2], p[1], p[0], p[3]]);
                    }
                }
                return Ok(Some((
                    time,
                    RgbaImage::new(w as u32, h as u32, rgba).map_err(|e| e.to_string())?,
                )));
            }
            Err("Offline decoder returned1024 marker-only samples without a video frame".into())
        }
    }
    fn frame_at(&mut self, time: f64) -> Result<Option<RgbaImage>, String> {
        if self
            .current
            .as_ref()
            .is_some_and(|(current, _)| time + 1e-6 < *current)
        {
            unsafe {
                self.reader.cancelReading();
                let (reader, output) = Self::reader(&self.asset, &self.track)?;
                self.reader = reader;
                self.output = output;
            }
            self.current = None;
            self.next = None;
            self.eof = false;
        }
        let started = Instant::now();
        let mut count = 0;
        loop {
            if self.next.is_none() && !self.eof {
                self.next = self.read_next()?;
            }
            if self.next.as_ref().is_some_and(|(t, _)| *t <= time + 1e-6) || self.current.is_none()
            {
                if let Some(next) = self.next.take() {
                    self.current = Some(next);
                } else {
                    break;
                }
            } else {
                break;
            }
            count += 1;
            if count > 1_000_000 || started.elapsed().as_secs() > 30 {
                return Err("Offline video seek exceeded1M frames or30s".into());
            }
        }
        Ok(self.current.as_ref().map(|(_, image)| image.clone()))
    }
}
impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe {
            self.reader.cancelReading();
        }
        let _ = std::fs::remove_file(&self.path);
    }
}
#[derive(Default)]
pub(super) struct OfflineVideos {
    videos: BTreeMap<String, Decoder>,
}
impl OfflineVideos {
    pub fn update(&mut self, player: &mut wpe_core::ScenePlayer, dt: f32) -> Result<bool, String> {
        for (name, bytes) in player.drain_video_requests() {
            if !self.videos.contains_key(&name) {
                self.videos
                    .insert(name.clone(), Decoder::new(&name, &bytes)?);
            }
        }
        let mut changed = false;
        let objects = player.scene()["objects"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        for object in objects {
            let meta = &object["__videoTexture"];
            let Some(name) = meta["resource"].as_str() else {
                continue;
            };
            let Some(video) = self.videos.get_mut(name) else {
                continue;
            };
            let requested = meta["__time"]
                .as_f64()
                .filter(|t| t.is_finite() && *t >= 0.)
                .unwrap_or(video.time);
            let playing = meta["__playing"].as_bool().unwrap_or(true);
            let rate = meta["rate"]
                .as_f64()
                .filter(|r| r.is_finite() && *r >= 0.)
                .unwrap_or(1.);
            let looped = meta["loop"].as_bool().unwrap_or(true);
            if (requested - video.last_feedback).abs() > 1e-6 {
                video.time = requested;
            } else if playing {
                video.time += dt as f64 * rate;
            }
            if !video.time.is_finite() {
                return Err("Offline video clock overflowed".into());
            }
            if looped {
                video.time = video.time.rem_euclid(video.duration);
            } else {
                video.time = video.time.clamp(0., (video.duration - 1e-7).max(0.));
            }
            if let Some(image) = video.frame_at(video.time)? {
                player
                    .provide_texture_frame(name, image)
                    .map_err(|e| e.to_string())?;
                changed = true;
            }
            video.last_feedback = video.time;
            player.apply_provider_state(&object["id"],&json!({"__videoTexture":{"resource":name,"__time":video.time,"__playing":playing,"loop":looped,"rate":rate,"duration":video.duration,"fps":video.fps,"frameCount":(video.duration*video.fps as f64).round() as u64,"frameCountIsNominalEstimate":true}})).map_err(|e|e.to_string())?;
        }
        Ok(changed)
    }
}
