//! Explicit owned synthetic RGBA export. Never opens source wallpapers/audio.
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use wpe_platform::export::{ExportSettings, FfmpegEncoder};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let ffmpeg = PathBuf::from(args.next().ok_or("FFmpeg absolute path required")?);
    let output = PathBuf::from(args.next().ok_or("New output file required")?);
    let settings = ExportSettings {
        width: 32,
        height: 32,
        fps: 10,
        duration: 1.,
        format: "ffmpeg_mp4_h264".into(),
        output: Some(output.clone()),
        ..Default::default()
    };
    let mut writer = FfmpegEncoder::new(&ffmpeg, &output, &settings)?;
    let start = Instant::now();
    for index in 0..settings.frames() {
        let mut frame = vec![0; 32 * 32 * 4];
        for pixel in frame.chunks_exact_mut(4) {
            pixel.copy_from_slice(&[(index * 20) as u8, 90, 180, 255]);
        }
        while let Some(returned) = writer.submit(frame)? {
            frame = returned;
            if start.elapsed().as_secs() > 30 {
                return Err("Encoder input timeout".into());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    writer.finish();
    loop {
        let status = writer.status();
        if let Some(error) = status.error {
            return Err(error.into());
        }
        if status.finished {
            println!(
                "{}",
                serde_json::json!({"output":output,"input_frames":settings.frames(),"written_frames":status.written,"profile":"owned32x32RGBA,10fps,1s,noaudio","encoder":ffmpeg,"native_mac_execution":false})
            );
            break;
        }
        if start.elapsed().as_secs() > 40 {
            return Err("Encoder close timeout".into());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    Ok(())
}
