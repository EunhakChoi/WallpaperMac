use std::{path::PathBuf, process::ExitCode};
fn main() -> ExitCode {
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let path = std::env::args_os()
            .nth(1)
            .map(PathBuf::from)
            .ok_or("audio path required")?;
        let source = std::fs::read(&path)?;
        let wav = wpe_platform::decode::to_wav(&source, path.to_str())?;
        let channels = u16::from_le_bytes(wav[22..24].try_into()?);
        let rate = u32::from_le_bytes(wav[24..28].try_into()?);
        let samples = wav[44..]
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes(b.try_into().unwrap()))
            .collect::<Vec<_>>();
        println!(
            "{}",
            serde_json::json!({"source":path,"source_bytes":source.len(),"pcm16_wav_bytes":wav.len(),"sample_rate":rate,"channels":channels,"frames":samples.len()/channels as usize,"duration_seconds":samples.len()as f64/(rate as f64*channels as f64),"nonzero_samples":samples.iter().filter(|v|**v!=0).count(),"peak":samples.iter().map(|v|(*v as i32).abs()).max(),"audio_devices_opened":0,"audio_output_calls":0})
        );
        Ok(())
    })();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
