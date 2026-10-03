//! Audio in → 16 kHz mono `f32` in `[-1, 1]`, the format every STT engine takes.
//!
//! P1 scope: decode WAV/FLAC files, downmix, resample, write 16 kHz WAV.
//! Live capture (cpal/WASAPI ring buffer, VAD) arrives in P2.

use audioadapter_buffers::direct::InterleavedSlice;
use rubato::audioadapter_buffers;
use rubato::{Fft, FixedSync, Resampler};
use std::path::{Path, PathBuf};

/// Sample rate every engine expects.
pub const TARGET_RATE: u32 = 16_000;

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("{path}: {msg}")]
    Decode { path: PathBuf, msg: String },
    #[error("{path}: unsupported audio file (use .wav or .flac)")]
    Unsupported { path: PathBuf },
    #[error("resampling failed: {0}")]
    Resample(String),
    #[error("{path}: {msg}")]
    Write { path: PathBuf, msg: String },
}

pub type Result<T, E = AudioError> = std::result::Result<T, E>;

/// Mono 16 kHz audio.
#[derive(Debug, Clone)]
pub struct Pcm16k {
    pub samples: Vec<f32>,
}

impl Pcm16k {
    pub fn duration_ms(&self) -> u64 {
        self.samples.len() as u64 * 1000 / u64::from(TARGET_RATE)
    }
}

/// Decode a `.wav` or `.flac` file and convert it to 16 kHz mono.
pub fn load_file(path: &Path) -> Result<Pcm16k> {
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
    let (interleaved, channels, rate) = match ext.as_str() {
        "wav" => decode_wav(path)?,
        "flac" => decode_flac(path)?,
        _ => return Err(AudioError::Unsupported { path: path.to_path_buf() }),
    };
    let samples = to_mono_16k(&interleaved, channels, rate)?;
    Ok(Pcm16k { samples })
}

/// Downmix interleaved `channels`-channel audio at `rate` Hz to 16 kHz mono.
pub fn to_mono_16k(interleaved: &[f32], channels: u16, rate: u32) -> Result<Vec<f32>> {
    let mono = downmix(interleaved, channels);
    resample(&mono, rate, TARGET_RATE)
}

/// Average all channels into one.
pub fn downmix(interleaved: &[f32], channels: u16) -> Vec<f32> {
    let ch = usize::from(channels.max(1));
    if ch == 1 {
        return interleaved.to_vec();
    }
    interleaved.chunks_exact(ch).map(|frame| frame.iter().sum::<f32>() / ch as f32).collect()
}

/// Resample mono audio. Uses rubato's FFT resampler (fixed ratio, high quality).
pub fn resample(mono: &[f32], from: u32, to: u32) -> Result<Vec<f32>> {
    if from == to || mono.is_empty() {
        return Ok(mono.to_vec());
    }
    let mut resampler = Fft::<f32>::new(from as usize, to as usize, 1024, 1, FixedSync::Input)
        .map_err(|e| AudioError::Resample(e.to_string()))?;
    let input = InterleavedSlice::new(mono, 1, mono.len()).map_err(|e| AudioError::Resample(e.to_string()))?;
    let out = resampler.process_all(&input, mono.len(), None).map_err(|e| AudioError::Resample(e.to_string()))?;
    Ok(out.take_data())
}

/// Write 16 kHz mono audio as 16-bit PCM WAV.
pub fn write_wav_16k(path: &Path, samples: &[f32]) -> Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: TARGET_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let werr = |e: hound::Error| AudioError::Write { path: path.to_path_buf(), msg: e.to_string() };
    let mut w = hound::WavWriter::create(path, spec).map_err(werr)?;
    for &s in samples {
        let v = (s.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16;
        w.write_sample(v).map_err(werr)?;
    }
    w.finalize().map_err(werr)
}

fn decode_wav(path: &Path) -> Result<(Vec<f32>, u16, u32)> {
    let derr = |e: hound::Error| AudioError::Decode { path: path.to_path_buf(), msg: e.to_string() };
    let mut r = hound::WavReader::open(path).map_err(derr)?;
    let spec = r.spec();
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => r.samples::<f32>().collect::<Result<_, _>>().map_err(derr)?,
        hound::SampleFormat::Int => {
            let scale = int_scale(spec.bits_per_sample);
            r.samples::<i32>().map(|s| s.map(|v| v as f32 / scale)).collect::<Result<_, _>>().map_err(derr)?
        }
    };
    Ok((samples, spec.channels, spec.sample_rate))
}

fn decode_flac(path: &Path) -> Result<(Vec<f32>, u16, u32)> {
    let derr = |e: claxon::Error| AudioError::Decode { path: path.to_path_buf(), msg: e.to_string() };
    let mut r = claxon::FlacReader::open(path).map_err(derr)?;
    let info = r.streaminfo();
    let scale = int_scale(info.bits_per_sample as u16);
    let samples: Vec<f32> = r.samples().map(|s| s.map(|v| v as f32 / scale)).collect::<Result<_, _>>().map_err(derr)?;
    Ok((samples, info.channels as u16, info.sample_rate))
}

fn int_scale(bits: u16) -> f32 {
    (1u64 << (bits.clamp(1, 32) - 1)) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, secs: f32, hz: f32) -> Vec<f32> {
        let n = (rate as f32 * secs) as usize;
        (0..n).map(|i| 0.5 * (2.0 * std::f32::consts::PI * hz * i as f32 / rate as f32).sin()).collect()
    }

    #[test]
    fn downmix_averages_channels() {
        assert_eq!(downmix(&[1.0, 0.0, 0.5, 0.5], 2), vec![0.5, 0.5]);
        assert_eq!(downmix(&[0.25, 0.75], 1), vec![0.25, 0.75]);
    }

    #[test]
    fn resample_48k_to_16k_keeps_duration_and_tone() {
        let input = sine(48_000, 1.0, 440.0);
        let out = resample(&input, 48_000, 16_000).unwrap();
        let expected = 16_000i64;
        assert!((out.len() as i64 - expected).abs() <= 2, "len {}", out.len());
        // Energy preserved (a 0.5-amplitude sine has RMS ≈ 0.354), ignoring edges.
        let mid = &out[1600..14_400];
        let rms = (mid.iter().map(|s| s * s).sum::<f32>() / mid.len() as f32).sqrt();
        assert!((rms - 0.3536).abs() < 0.02, "rms {rms}");
    }

    #[test]
    fn resample_44k1_and_identity() {
        let input = sine(44_100, 0.5, 300.0);
        let out = resample(&input, 44_100, 16_000).unwrap();
        assert!((out.len() as i64 - 8_000).abs() <= 2, "len {}", out.len());
        assert_eq!(resample(&input, 16_000, 16_000).unwrap().len(), input.len());
        assert!(resample(&[], 48_000, 16_000).unwrap().is_empty());
    }

    #[test]
    fn wav_round_trip_stereo_48k() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("t.wav");
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for s in sine(48_000, 0.5, 440.0) {
            let v = (s * 32767.0) as i16;
            w.write_sample(v).unwrap();
            w.write_sample(v).unwrap();
        }
        w.finalize().unwrap();
        let pcm = load_file(&path).unwrap();
        assert!((pcm.duration_ms() as i64 - 500).abs() <= 1);

        let out = tmp.path().join("o.wav");
        write_wav_16k(&out, &pcm.samples).unwrap();
        let again = load_file(&out).unwrap();
        assert_eq!(again.samples.len(), pcm.samples.len());
        assert!(again.samples.iter().all(|s| (-1.0..=1.0).contains(s)));
    }

    #[test]
    fn rejects_unknown_extension() {
        assert!(matches!(load_file(Path::new("clip.mp3")), Err(AudioError::Unsupported { .. })));
    }
}
