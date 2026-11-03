//! Portable compressed-audio fallback, producing an actual PCM16 WAV for the
//! native playback device. This decoder never opens an audio output or records.
use crate::{Error, Result};
use std::{io::Cursor, path::Path};
use symphonia::core::{
    codecs::audio::AudioDecoderOptions,
    formats::{FormatOptions, TrackType, probe::Hint},
    io::MediaSourceStream,
    meta::MetadataOptions,
};
pub fn to_wav(bytes: &[u8], name: Option<&str>) -> Result<Vec<u8>> {
    if bytes.is_empty() || bytes.len() > 128 * 1024 * 1024 {
        return Err(Error("Compressed audio is empty or exceeds128MiB".into()));
    }
    let mut hint = Hint::new();
    if let Some(extension) = name
        .and_then(|n| Path::new(n).extension())
        .and_then(|e| e.to_str())
    {
        hint.with_extension(extension);
    }
    let stream = MediaSourceStream::new(Box::new(Cursor::new(bytes.to_vec())), Default::default());
    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            stream,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|e| Error(format!("Audio container: {e}")))?;
    let track = format
        .default_track(TrackType::Audio)
        .ok_or_else(|| Error("Container has no default audio track".into()))?;
    let track_id = track.id;
    let parameters = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or_else(|| Error("Audio codec parameters are missing".into()))?;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(parameters, &AudioDecoderOptions::default())
        .map_err(|e| Error(format!("Audio codec: {e}")))?;
    let mut pcm = Vec::<u8>::new();
    let mut spec = None;
    let mut packets = 0usize;
    while let Some(packet) = format
        .next_packet()
        .map_err(|e| Error(format!("Audio packet: {e}")))?
    {
        packets += 1;
        if packets > 2_000_000 {
            return Err(Error("Audio exceeds2000000 packets".into()));
        }
        if packet.track_id != track_id {
            continue;
        }
        let decoded = decoder
            .decode(&packet)
            .map_err(|e| Error(format!("Audio decode: {e}")))?;
        let current = (decoded.spec().rate(), decoded.spec().channels().count());
        if current.0 < 8000 || current.0 > 384000 || current.1 == 0 || current.1 > 32 {
            return Err(Error(
                "Decoded audio rate/channel count is unsupported".into(),
            ));
        }
        if spec.is_some_and(|old| old != current) {
            return Err(Error(
                "Audio changes channel/rate format within one playback asset".into(),
            ));
        }
        spec = Some(current);
        let samples = decoded.samples_interleaved();
        if samples > 128 * 1024 * 1024
            || pcm
                .len()
                .checked_add(samples * 2)
                .is_none_or(|n| n > 256 * 1024 * 1024)
        {
            return Err(Error("Decoded PCM exceeds256MiB".into()));
        }
        let mut output = vec![0i16; samples];
        decoded.copy_to_slice_interleaved(&mut output);
        for sample in output {
            pcm.extend_from_slice(&sample.to_le_bytes());
        }
    }
    let (rate, channels) = spec.ok_or_else(|| Error("Audio contains no decoded samples".into()))?;
    pcm16_wav(rate, channels as u16, &pcm)
}
pub fn pcm16_wav(rate: u32, channels: u16, pcm: &[u8]) -> Result<Vec<u8>> {
    if rate == 0 || channels == 0 || !pcm.len().is_multiple_of(channels as usize * 2) {
        return Err(Error(
            "PCM16 WAV has an invalid rate/channel/frame layout".into(),
        ));
    }
    let byte_rate = rate
        .checked_mul(channels as u32 * 2)
        .ok_or_else(|| Error("PCM16 WAV byte-rate overflow".into()))?;
    let n = u32::try_from(pcm.len())
        .map_err(|_| Error("PCM16 WAV exceeds32-bit RIFF length".into()))?;
    let length = n
        .checked_add(36)
        .ok_or_else(|| Error("PCM16 WAV RIFF size overflow".into()))?;
    let mut out = Vec::with_capacity(pcm.len() + 44);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&length.to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&(channels * 2).to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&n.to_le_bytes());
    out.extend_from_slice(pcm);
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_fallback_is_a_decoded_riff_with_preserved_pcm_frames() {
        let pcm = [-32768i16, -8192, 0, 8192, 32767]
            .into_iter()
            .flat_map(i16::to_le_bytes)
            .collect::<Vec<_>>();
        let original = pcm16_wav(48000, 1, &pcm).unwrap();
        let decoded = to_wav(&original, Some("owned.wav")).unwrap();
        assert_eq!(&decoded[..4], b"RIFF");
        assert_eq!(&decoded[44..], pcm);
        assert_eq!(
            u32::from_le_bytes(decoded[24..28].try_into().unwrap()),
            48000
        );
        assert!(to_wav(b"not audio", None).is_err());
    }
}
