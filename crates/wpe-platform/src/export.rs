//! Bounded offline frame settings, temporal accumulation and an explicit
//! user-configured FFmpeg process. No encoder executable is bundled.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender, TrySendError},
    },
    time::{Duration, Instant},
};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportSettings {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate: u32,
    pub duration: f64,
    pub blend: u32,
    pub format: String,
    pub output: Option<PathBuf>,
    pub live_inputs: bool,
}
impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            fps: 30,
            bitrate: 8_000_000,
            duration: 5.,
            blend: 1,
            format: "mp4_h264".into(),
            output: None,
            live_inputs: false,
        }
    }
}
impl ExportSettings {
    pub fn validate(&self) -> Result<()> {
        if self.width == 0
            || self.height == 0
            || self.width > 8192
            || self.height > 8192
            || self.width as u64 * self.height as u64 > 16_777_216
        {
            return Err(Error("Export dimensions exceed8192 or16M pixels".into()));
        }
        if self.fps == 0
            || self.fps > 120
            || self.blend == 0
            || self.blend > 64
            || !self.duration.is_finite()
            || self.duration <= 0.
            || self.duration > 600.
            || self.frames() as u64 * self.blend as u64 > 1_000_000
        {
            return Err(Error(
                "Export requires FPS1..120, samples1..64, duration0..600s and at most1M samples"
                    .into(),
            ));
        }
        if self.bitrate < 1000 || self.bitrate > 1_000_000_000 {
            return Err(Error("Export bitrate must be1000..1000000000".into()));
        }
        if !matches!(
            self.format.as_str(),
            "mp4_h264"
                | "png"
                | "avi"
                | "wmv_3"
                | "ffmpeg_mp4_h264"
                | "ffmpeg_mp4_h265"
                | "ffmpeg_mp4_vp8"
                | "ffmpeg_mp4_vp9"
                | "ffmpeg_gif"
                | "ffmpeg_avi_raw"
                | "ffmpeg_avi_yuv422"
                | "ffmpeg_avi_yuv420"
        ) {
            return Err(Error(format!("Unknown export format {}", self.format)));
        }
        if self.format != "png" && (!self.width.is_multiple_of(2) || !self.height.is_multiple_of(2))
        {
            return Err(Error("Video export dimensions must be even".into()));
        }
        Ok(())
    }
    pub fn frames(&self) -> u32 {
        (self.duration * self.fps as f64).ceil() as u32
    }
    pub fn extension(&self) -> &'static str {
        match self.format.as_str() {
            "png" => "frames",
            "ffmpeg_mp4_vp8" | "ffmpeg_mp4_vp9" => "webm",
            "ffmpeg_gif" => "gif",
            "avi" | "ffmpeg_avi_raw" | "ffmpeg_avi_yuv422" | "ffmpeg_avi_yuv420" => "avi",
            "wmv_3" => "wmv",
            _ => "mp4",
        }
    }
}
pub struct FrameAccumulator {
    sums: Vec<u32>,
    count: u32,
    target: u32,
    width: u32,
    height: u32,
}
impl FrameAccumulator {
    pub fn new(settings: &ExportSettings) -> Result<Self> {
        settings.validate()?;
        Ok(Self {
            sums: vec![0; settings.width as usize * settings.height as usize * 4],
            count: 0,
            target: settings.blend,
            width: settings.width,
            height: settings.height,
        })
    }
    pub fn push(
        &mut self,
        image: &wpe_formats::RgbaImage,
    ) -> Result<Option<wpe_formats::RgbaImage>> {
        if (image.width, image.height) != (self.width, self.height)
            || image.rgba.len() != self.sums.len()
        {
            return Err(Error(
                "Export readback dimensions differ from configured output".into(),
            ));
        }
        for (sum, value) in self.sums.iter_mut().zip(&image.rgba) {
            *sum += *value as u32;
        }
        self.count += 1;
        if self.count < self.target {
            return Ok(None);
        }
        let rgba = self
            .sums
            .iter()
            .map(|v| ((v + self.count / 2) / self.count) as u8)
            .collect();
        self.sums.fill(0);
        self.count = 0;
        Ok(Some(wpe_formats::RgbaImage {
            width: self.width,
            height: self.height,
            rgba,
        }))
    }
}
#[derive(Clone, Debug, Default, Serialize)]
pub struct EncoderStatus {
    pub written: u32,
    pub finished: bool,
    pub error: Option<String>,
}
pub struct FfmpegEncoder {
    tx: Option<SyncSender<Vec<u8>>>,
    status: Arc<Mutex<EncoderStatus>>,
    cancel: Arc<AtomicBool>,
    pid: u32,
    live: Arc<Mutex<bool>>,
}
impl FfmpegEncoder {
    pub fn new(executable: &Path, path: &Path, settings: &ExportSettings) -> Result<Self> {
        settings.validate()?;
        if !executable.is_absolute() || !executable.is_file() {
            return Err(Error("Configure an absolute FFmpeg executable path".into()));
        }
        if path.exists() {
            return Err(Error("FFmpeg output path already exists".into()));
        }
        let (codec, container, pixfmt) = match settings.format.as_str() {
            "ffmpeg_mp4_h264" => ("libx264", "mp4", "yuv420p"),
            "ffmpeg_mp4_h265" => ("libx265", "mp4", "yuv420p"),
            "ffmpeg_mp4_vp8" => ("libvpx", "webm", "yuv420p"),
            "ffmpeg_mp4_vp9" => ("libvpx-vp9", "webm", "yuv420p"),
            "ffmpeg_gif" => ("gif", "gif", "rgb8"),
            "wmv_3" => ("wmv3", "asf", "yuv420p"),
            "ffmpeg_avi_yuv422" => ("rawvideo", "avi", "yuv422p"),
            "ffmpeg_avi_yuv420" => ("rawvideo", "avi", "yuv420p"),
            "avi" | "ffmpeg_avi_raw" => ("rawvideo", "avi", "bgr24"),
            _ => {
                return Err(Error(
                    "This format does not use the external FFmpeg provider".into(),
                ));
            }
        };
        let mut command = Command::new(executable);
        command
            .args([
                "-nostdin",
                "-hide_banner",
                "-loglevel",
                "error",
                "-n",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgba",
                "-video_size",
                &format!("{}x{}", settings.width, settings.height),
                "-framerate",
                &settings.fps.to_string(),
                "-i",
                "pipe:0",
                "-an",
                "-c:v",
                codec,
                "-pix_fmt",
                pixfmt,
                "-b:v",
                &settings.bitrate.to_string(),
                "-f",
                container,
            ])
            .arg(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let live = Arc::new(Mutex::new(true));
        let mut child = EncoderChild {
            child: command.spawn()?,
            live: live.clone(),
        };
        let pid = child.id();
        let mut input = child
            .stdin
            .take()
            .ok_or_else(|| Error("FFmpeg input pipe unavailable".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| Error("FFmpeg error pipe unavailable".into()))?;
        let errors = std::thread::spawn(move || {
            let mut bytes = vec![];
            let _ = stderr.take(65536).read_to_end(&mut bytes);
            bytes
        });
        let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(2);
        let status = Arc::new(Mutex::new(EncoderStatus::default()));
        let cancel = Arc::new(AtomicBool::new(false));
        let (shared, stop) = (status.clone(), cancel.clone());
        let expected = settings.width as usize * settings.height as usize * 4;
        std::thread::Builder::new()
            .name("wallpaper-ffmpeg-export".into())
            .spawn(move || {
                let result = (|| -> Result<()> {
                    while !stop.load(Ordering::Acquire) {
                        match rx.recv_timeout(Duration::from_millis(100)) {
                            Ok(bytes) => {
                                if bytes.len() != expected {
                                    return Err(Error(
                                        "Invalid RGBA frame size sent to encoder".into(),
                                    ));
                                }
                                input.write_all(&bytes)?;
                                if let Ok(mut s) = shared.lock() {
                                    s.written += 1;
                                }
                            }
                            Err(mpsc::RecvTimeoutError::Disconnected) => break,
                            Err(mpsc::RecvTimeoutError::Timeout) => {
                                if let Some(exit) = child.try_wait()? {
                                    return Err(Error(format!(
                                        "FFmpeg exited before all frames ({exit})"
                                    )));
                                }
                            }
                        }
                    }
                    drop(input);
                    if stop.load(Ordering::Acquire) {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(Error("Export cancelled".into()));
                    }
                    let end = Instant::now() + Duration::from_secs(30);
                    loop {
                        if let Some(exit) = child.try_wait()? {
                            if exit.success() {
                                return Ok(());
                            }
                            return Err(Error(format!("FFmpeg encoder failed ({exit})")));
                        }
                        if Instant::now() >= end {
                            let _ = child.kill();
                            let _ = child.wait();
                            return Err(Error("FFmpeg finalization exceeded30s".into()));
                        }
                        std::thread::sleep(Duration::from_millis(20));
                    }
                })();
                if result.is_err() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                let message =
                    String::from_utf8_lossy(&errors.join().unwrap_or_default()).into_owned();
                if let Ok(mut s) = shared.lock() {
                    s.finished = true;
                    s.error = result.err().map(|e| format!("{e}: {message}"));
                }
            })
            .map_err(Error::from)?;
        Ok(Self {
            tx: Some(tx),
            status,
            cancel,
            pid,
            live,
        })
    }
    pub fn submit(&self, rgba: Vec<u8>) -> Result<Option<Vec<u8>>> {
        if let Some(error) = self.status().error {
            return Err(Error(error));
        }
        let tx = self
            .tx
            .as_ref()
            .ok_or_else(|| Error("Encoder already finishing".into()))?;
        match tx.try_send(rgba) {
            Ok(()) => Ok(None),
            Err(TrySendError::Full(bytes)) => Ok(Some(bytes)),
            Err(TrySendError::Disconnected(_)) => Err(Error("Encoder input disconnected".into())),
        }
    }
    pub fn finish(&mut self) {
        self.tx.take();
    }
    pub fn status(&self) -> EncoderStatus {
        self.status
            .lock()
            .map(|s| s.clone())
            .unwrap_or(EncoderStatus {
                finished: true,
                error: Some("Encoder status unavailable".into()),
                ..Default::default()
            })
    }
    pub fn cancel(&mut self) {
        self.cancel.store(true, Ordering::Release);
        self.tx.take();
        #[cfg(unix)]
        if let Ok(live) = self.live.lock()
            && *live
        {
            unsafe {
                libc::kill(-(self.pid as i32), libc::SIGKILL);
            }
        }

        #[cfg(not(unix))]
        {
            let _ = self.pid;
        }
    }
}
// Reaping and cancellation share the liveness lock, so a delayed cancellation
// cannot signal a recycled PID. The guard also cleans up thread-spawn failures.
struct EncoderChild {
    child: std::process::Child,
    live: Arc<Mutex<bool>>,
}
impl std::ops::Deref for EncoderChild {
    type Target = std::process::Child;
    fn deref(&self) -> &Self::Target {
        &self.child
    }
}
impl std::ops::DerefMut for EncoderChild {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.child
    }
}
impl EncoderChild {
    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        let mut live = self.live.lock().unwrap_or_else(|e| e.into_inner());
        let result = self.child.try_wait();
        if !matches!(result, Ok(None)) {
            *live = false;
        }
        result
    }
    fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        let mut live = self.live.lock().unwrap_or_else(|e| e.into_inner());
        let result = self.child.wait();
        *live = false;
        result
    }
}
impl Drop for EncoderChild {
    fn drop(&mut self) {
        if matches!(self.try_wait(), Ok(None)) {
            #[cfg(unix)]
            unsafe {
                libc::kill(-(self.child.id() as i32), libc::SIGKILL);
            }
            let _ = self.child.kill();
            let _ = self.wait();
        }
    }
}
impl Drop for FfmpegEncoder {
    fn drop(&mut self) {
        if !self.status().finished {
            self.cancel();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn temporal_accumulation_and_format_limits_are_explicit() {
        let s = ExportSettings {
            width: 2,
            height: 2,
            blend: 2,
            duration: 1.25,
            fps: 4,
            ..Default::default()
        };
        let mut a = FrameAccumulator::new(&s).unwrap();
        assert_eq!(s.frames(), 5);
        let image = wpe_formats::RgbaImage {
            width: 2,
            height: 2,
            rgba: vec![10; 16],
        };
        assert!(a.push(&image).unwrap().is_none());
        let other = wpe_formats::RgbaImage {
            rgba: vec![30; 16],
            ..image
        };
        assert_eq!(a.push(&other).unwrap().unwrap().rgba, vec![20; 16]);
        assert!(
            ExportSettings {
                width: 1,
                ..s.clone()
            }
            .validate()
            .is_err()
        );
        assert!(
            ExportSettings {
                format: "guess".into(),
                ..s
            }
            .validate()
            .is_err()
        );
    }
}
