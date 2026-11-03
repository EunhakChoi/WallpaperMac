//! Source-contract spectrum analysis and scene-frame audio processing.
//! This preserves the recovered input/weight/reduction equations. Scalar reciprocal,
//! libm and independent FFT arithmetic require original/Mac numeric comparison.
use crate::{Error, Result, config::AudioSettings};
use std::collections::VecDeque;

#[derive(Clone, Debug)]
pub struct PcmBlock {
    pub sample_rate: u32,
    pub channels: usize,
    pub samples: Vec<f32>,
    pub input_route: bool,
}
#[derive(Clone, Debug)]
pub struct RawSpectrum {
    pub left: [f32; 64],
    pub right: [f32; 64],
    pub input_route: bool,
}
impl Default for RawSpectrum {
    fn default() -> Self {
        Self {
            left: [0.0; 64],
            right: [0.0; 64],
            input_route: false,
        }
    }
}
#[derive(Clone, Debug)]
pub struct AudioBands {
    pub bands16: [[f32; 16]; 3],
    pub bands32: [[f32; 32]; 3],
    pub bands64: [[f32; 64]; 3],
}
impl Default for AudioBands {
    fn default() -> Self {
        Self {
            bands16: [[0.0; 16]; 3],
            bands32: [[0.0; 32]; 3],
            bands64: [[0.0; 64]; 3],
        }
    }
}
impl AudioBands {
    pub fn from_stereo(left: [f32; 64], right: [f32; 64]) -> Self {
        let mut out = Self::default();
        out.bands64[0] = left;
        out.bands64[1] = right;
        for i in 0..64 {
            out.bands64[2][i] = (left[i] + right[i]) * 0.5;
        }
        for ch in 0..3 {
            for i in 0..32 {
                out.bands32[ch][i] = out.bands64[ch][i * 2].max(out.bands64[ch][i * 2 + 1]);
            }
            for i in 0..16 {
                out.bands16[ch][i] = out.bands32[ch][i * 2].max(out.bands32[ch][i * 2 + 1]);
            }
        }
        out
    }
    pub fn channel(&self, resolution: usize, channel: usize) -> Option<&[f32]> {
        if channel > 2 {
            return None;
        }
        match resolution {
            16 => Some(&self.bands16[channel]),
            32 => Some(&self.bands32[channel]),
            64 => Some(&self.bands64[channel]),
            _ => None,
        }
    }
    pub fn web_stereo(&self) -> Vec<f32> {
        self.bands64[0]
            .iter()
            .chain(&self.bands64[1])
            .copied()
            .collect()
    }
}
#[derive(Clone, Copy, Debug, Default)]
struct Complex {
    re: f32,
    im: f32,
}
impl Complex {
    fn mul(self, b: Self) -> Self {
        Self {
            re: self.re * b.re - self.im * b.im,
            im: self.re * b.im + self.im * b.re,
        }
    }
    fn cis(angle: f64) -> Self {
        Self {
            re: angle.cos() as f32,
            im: angle.sin() as f32,
        }
    }
}
fn radix2(values: &mut [Complex], inverse: bool) {
    let n = values.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            values.swap(i, j);
        }
    }
    let mut size = 2;
    while size <= n {
        let step =
            Complex::cis((if inverse { 2.0 } else { -2.0 }) * std::f64::consts::PI / size as f64);
        for start in (0..n).step_by(size) {
            let mut w = Complex { re: 1.0, im: 0.0 };
            for k in 0..size / 2 {
                let a = values[start + k];
                let b = values[start + k + size / 2].mul(w);
                values[start + k] = Complex {
                    re: a.re + b.re,
                    im: a.im + b.im,
                };
                values[start + k + size / 2] = Complex {
                    re: a.re - b.re,
                    im: a.im - b.im,
                };
                w = w.mul(step);
            }
        }
        size *= 2;
    }
    if inverse {
        for v in values {
            v.re /= n as f32;
            v.im /= n as f32;
        }
    }
}
struct TransformPlan {
    n: usize,
    m: usize,
    chirp: Vec<Complex>,
    kernel: Vec<Complex>,
}
impl TransformPlan {
    fn new(n: usize) -> Self {
        if n.is_power_of_two() {
            return Self {
                n,
                m: n,
                chirp: vec![],
                kernel: vec![],
            };
        }
        let m = (n * 2 - 1).next_power_of_two();
        let mut kernel = vec![Complex::default(); m];
        let mut chirp = Vec::with_capacity(n);
        for i in 0..n {
            let phase =
                std::f64::consts::PI * ((i as u64 * i as u64) % (2 * n) as u64) as f64 / n as f64;
            let positive = Complex::cis(phase);
            chirp.push(Complex {
                re: positive.re,
                im: -positive.im,
            });
            kernel[i] = positive;
            if i > 0 {
                kernel[m - i] = positive;
            }
        }
        radix2(&mut kernel, false);
        Self {
            n,
            m,
            chirp,
            kernel,
        }
    }
    fn transform(&self, input: &[Complex]) -> Vec<Complex> {
        if self.n.is_power_of_two() {
            let mut result = input.to_vec();
            radix2(&mut result, false);
            return result;
        }
        let mut a = vec![Complex::default(); self.m];
        for i in 0..self.n {
            a[i] = input[i].mul(self.chirp[i]);
        }
        radix2(&mut a, false);
        for (a, b) in a.iter_mut().zip(&self.kernel) {
            *a = a.mul(*b);
        }
        radix2(&mut a, true);
        a.truncate(self.n);
        for (a, b) in a.iter_mut().zip(&self.chirp) {
            *a = a.mul(*b);
        }
        a
    }
}

pub struct SpectrumAnalyzer {
    settings: AudioSettings,
    rate: u32,
    plan: TransformPlan,
    prefix: usize,
    pending: VecDeque<[f32; 2]>,
    pub dropped_frames: u64,
}
impl SpectrumAnalyzer {
    pub fn new(settings: AudioSettings) -> Self {
        let rate = settings.sample_rate.max(8000);
        let n = Self::length(rate);
        Self {
            settings,
            rate,
            plan: TransformPlan::new(n),
            prefix: Self::prefix(n),
            pending: VecDeque::new(),
            dropped_frames: 0,
        }
    }
    pub fn length(rate: u32) -> usize {
        ((rate as f32 / 44100.0).max(1.0) * 64.0 * 30.0) as usize
    }
    pub fn prefix(n: usize) -> usize {
        (n as f32 - ((10.0f32 / 30.0) * n as f32)) as usize
    }
    pub fn set_settings(&mut self, settings: AudioSettings) {
        self.settings = settings;
    }
    pub fn push(&mut self, block: &PcmBlock) -> Result<Option<RawSpectrum>> {
        if !(8000..=384000).contains(&block.sample_rate)
            || block.channels == 0
            || block.channels > 32
            || !block.samples.len().is_multiple_of(block.channels)
            || block.samples.iter().any(|x| !x.is_finite())
        {
            return Err(Error(
                "Invalid PCM rate/channel/layout or non-finite sample".into(),
            ));
        }
        if self.rate != block.sample_rate {
            self.rate = block.sample_rate;
            let n = Self::length(self.rate);
            self.plan = TransformPlan::new(n);
            self.prefix = Self::prefix(n);
            self.pending.clear();
        }
        for sample in block.samples.chunks_exact(block.channels) {
            self.pending
                .push_back([sample[0], sample[if block.channels == 1 { 0 } else { 1 }]]);
        }
        while self.pending.len() > self.prefix * 8 {
            self.pending.pop_front();
            self.dropped_frames += 1;
        }
        let mut last = None;
        while self.pending.len() >= self.prefix {
            let frames = self.pending.drain(..self.prefix).collect::<Vec<_>>();
            let threshold = self.settings.threshold as f32 * 0.001;
            if threshold > f32::EPSILON
                && frames.iter().map(|f| f[0]).fold(0.0f32, f32::max) < threshold
            {
                last = Some(RawSpectrum {
                    input_route: block.input_route,
                    ..RawSpectrum::default()
                });
                continue;
            }
            let mut left = [0.0f32; 64];
            let mut right = [0.0f32; 64];
            for (channel, out) in [(0, &mut left), (1, &mut right)] {
                let mut input = vec![
                    Complex {
                        re: 127.0,
                        im: 1.0 / 127.0
                    };
                    self.plan.n
                ];
                for (v, sample) in input.iter_mut().zip(&frames) {
                    let v0 = sample[channel] * 127.0 + 127.0;
                    *v = Complex {
                        re: v0,
                        im: 1.0 / v0,
                    };
                }
                let transformed = self.plan.transform(&input);
                let a = f32::from_bits(0x3f004189);
                let mut previous = 0;
                for (i, v) in transformed.iter().enumerate().take(640).skip(1) {
                    let power = v.re * v.re + v.im * v.im;
                    let power = if power.is_finite() { power } else { 0.0 };
                    let x = (i - 1) as f32 / 639.0;
                    let q = ((x.powf(0.25) * 64.0) as usize) % 64;
                    let bin = (previous + 1).min(q);
                    previous = bin;
                    let weight = a - (1.0 - a) * (std::f32::consts::PI * x).cos();
                    out[bin] = out[bin].max((weight * power).sqrt());
                }
                let gain = self.settings.volume as f32
                    * 0.02
                    * 0.001
                    * (640.0 / (self.plan.n as f32 * 0.5));
                for v in out.iter_mut() {
                    *v *= gain;
                }
            }
            last = Some(RawSpectrum {
                left,
                right,
                input_route: block.input_route,
            });
        }
        Ok(last)
    }
}

pub struct AudioFrameProcessor {
    peaks: [f32; 16],
    smooth: [f32; 128],
    previous: [f32; 128],
}
impl Default for AudioFrameProcessor {
    fn default() -> Self {
        Self {
            peaks: [0.0; 16],
            smooth: [0.0; 128],
            previous: [0.0; 128],
        }
    }
}
impl AudioFrameProcessor {
    pub fn process(&mut self, raw: &RawSpectrum, dt: f32) -> AudioBands {
        if raw.input_route {
            return AudioBands::from_stereo(raw.left, raw.right);
        }
        let mut data = [0.0f32; 128];
        data[..64].copy_from_slice(&raw.left);
        data[64..].copy_from_slice(&raw.right);
        let global = data.iter().copied().fold(0.0f32, f32::max);
        if global < 0.0001 {
            return AudioBands::default();
        }
        if self.peaks[0] <= 0.0001 {
            self.peaks.fill(1.0);
        }
        let dt = if dt.is_finite() {
            dt.clamp(0.0, 1.0)
        } else {
            0.0
        };
        for (group, values) in data.chunks_mut(8).enumerate() {
            let target = values
                .iter()
                .copied()
                .fold(0.0f32, f32::max)
                .max(global * 0.333);
            let delta = target - self.peaks[group];
            if delta.abs() <= 0.0001 {
                self.peaks[group] = target;
            } else {
                self.peaks[group] += delta.abs().min(dt) * if delta > 0.0 { 1.0 } else { -0.5 };
            }
            let reciprocal = 1.0 / self.peaks[group].max(0.001);
            for (j, value) in values.iter_mut().enumerate() {
                let i = group * 8 + j;
                self.smooth[i] += (*value * reciprocal - self.smooth[i]) * (20.0 * dt).min(1.0);
                *value = self.previous[i]
                    + (self.smooth[i] - self.previous[i])
                        .clamp(-(40.0 * dt).min(1.0), (40.0 * dt).min(1.0));
                self.previous[i] = *value;
            }
        }
        AudioBands::from_stereo(
            data[..64].try_into().unwrap(),
            data[64..].try_into().unwrap(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn arbitrary_fft_matches_independent_direct_dft() {
        for n in [3, 7, 16, 31, 64, 93] {
            let input = (0..n)
                .map(|i| Complex {
                    re: (i as f32 * 0.37).sin(),
                    im: (i as f32 * 0.71).cos(),
                })
                .collect::<Vec<_>>();
            let result = TransformPlan::new(n).transform(&input);
            for (k, actual) in result.iter().enumerate() {
                let mut expected = Complex::default();
                for (i, v) in input.iter().enumerate() {
                    let t = v.mul(Complex::cis(
                        -2.0 * std::f64::consts::PI * (i * k) as f64 / n as f64,
                    ));
                    expected.re += t.re;
                    expected.im += t.im;
                }
                assert!((actual.re - expected.re).abs() < 0.002, "n={n} k={k}");
                assert!((actual.im - expected.im).abs() < 0.002);
            }
        }
    }
    #[test]
    fn average_is_computed_before_pairwise_max() {
        let mut l = [0.0; 64];
        let mut r = l;
        l[0] = 1.0;
        r[1] = 1.0;
        let a = AudioBands::from_stereo(l, r);
        assert_eq!(a.bands32[0][0], 1.0);
        assert_eq!(a.bands32[1][0], 1.0);
        assert_eq!(a.bands32[2][0], 0.5);
    }
    #[test]
    fn capture_route_bypasses_frame_normalization() {
        let raw = RawSpectrum {
            left: [3.0; 64],
            right: [1.0; 64],
            input_route: true,
        };
        assert_eq!(
            AudioFrameProcessor::default().process(&raw, 0.03).bands16[2],
            [2.0; 16]
        );
    }
    #[test]
    fn rates_prefix_and_stereo_signal_produce_finite_bands() {
        assert_eq!(SpectrumAnalyzer::length(44100), 1920);
        assert_eq!(SpectrumAnalyzer::length(48000), 2089);
        let mut a = SpectrumAnalyzer::new(AudioSettings::default());
        let frames = a.prefix;
        let pcm = (0..frames)
            .flat_map(|i| [(i as f32 * 0.1).sin() * 0.3, 0.0])
            .collect();
        let raw = a
            .push(&PcmBlock {
                sample_rate: 48000,
                channels: 2,
                samples: pcm,
                input_route: false,
            })
            .unwrap()
            .unwrap();
        assert!(raw.left.iter().chain(&raw.right).all(|x| x.is_finite()));
        assert!(raw.left.iter().sum::<f32>() > 0.0);
    }
}

#[derive(Clone, Copy, Debug)]
pub enum PcmEncoding {
    Float32,
    Float64,
    Signed {
        bits: u8,
        storage: u8,
        high_aligned: bool,
    },
    Unsigned8,
}
impl PcmEncoding {
    pub fn storage_bytes(self) -> usize {
        match self {
            Self::Float32 => 4,
            Self::Float64 => 8,
            Self::Signed { storage, .. } => storage as usize,
            Self::Unsigned8 => 1,
        }
    }
    /// Decode one host PCM sample without alignment assumptions.
    pub fn decode(self, bytes: &[u8], big_endian: bool) -> Result<f32> {
        let n = self.storage_bytes();
        if bytes.len() < n {
            return Err(Error("Truncated PCM sample".into()));
        }
        let value = match self {
            Self::Float32 => {
                let b: [u8; 4] = bytes[..4].try_into().unwrap();
                if big_endian {
                    f32::from_be_bytes(b)
                } else {
                    f32::from_le_bytes(b)
                }
            }
            Self::Float64 => {
                let b: [u8; 8] = bytes[..8].try_into().unwrap();
                (if big_endian {
                    f64::from_be_bytes(b)
                } else {
                    f64::from_le_bytes(b)
                }) as f32
            }
            Self::Unsigned8 => (bytes[0] as f32 - 128.) / 128.,
            Self::Signed {
                bits,
                storage,
                high_aligned,
            } => {
                if bits == 0 || bits > 32 || storage == 0 || storage > 4 || bits > storage * 8 {
                    return Err(Error("Invalid signed PCM bit/storage dimensions".into()));
                }
                let mut raw = 0u32;
                for i in 0..n {
                    let b = bytes[if big_endian { n - 1 - i } else { i }];
                    raw |= (b as u32) << (i * 8);
                }
                if high_aligned {
                    raw >>= storage * 8 - bits;
                }
                let shift = 64 - bits;
                let signed = ((raw as i64) << shift) >> shift;
                signed as f32 / (1u64 << (bits - 1)) as f32
            }
        };
        Ok(if value.is_finite() { value } else { 0. })
    }
}
