//! Speech-to-text engines behind one trait (docs/ARCHITECTURE.md §5).
//!
//! Engines are loaded once and kept warm; `transcribe` is called per dictation.
//! Which implementation loads a model is decided by the manifest's `engine` field.

use std::time::Instant;
use vt_models::ModelPaths;

#[cfg(feature = "sherpa")]
mod sherpa;
#[cfg(feature = "whisper")]
mod whisper;

/// Sample rate every engine takes (mono f32 in `[-1, 1]`).
pub const SAMPLE_RATE: u32 = 16_000;

#[derive(Debug, thiserror::Error)]
pub enum SttError {
    #[error("engine `{0}` is not compiled into this build")]
    EngineUnavailable(String),
    #[error("unknown engine kind `{0}`")]
    UnknownEngine(String),
    #[error("failed to load `{model}`: {msg}")]
    Load { model: String, msg: String },
    #[error("inference failed: {0}")]
    Inference(String),
    #[error(transparent)]
    Model(#[from] vt_models::ModelError),
}

pub type Result<T, E = SttError> = std::result::Result<T, E>;

#[derive(Debug, Clone, Default)]
pub struct SttOptions {
    /// Words/phrases to bias towards (used by engines that support it; P4).
    pub hotwords: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Transcript {
    pub text: String,
    pub audio_ms: u32,
    pub infer_ms: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct EngineConfig {
    /// Inference threads. Physical core count is a good default.
    pub threads: usize,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self { threads: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4) }
    }
}

pub trait SpeechEngine: Send {
    /// Manifest id of the loaded model.
    fn id(&self) -> &str;

    /// Transcribe 16 kHz mono audio. Implementations return raw engine text;
    /// timing is filled in by [`SpeechEngine::transcribe`].
    fn transcribe_raw(&mut self, pcm_16k_mono: &[f32], opts: &SttOptions) -> Result<String>;

    fn supports_hotwords(&self) -> bool {
        false
    }

    /// Transcribe and measure. Empty audio returns an empty transcript without
    /// touching the engine.
    fn transcribe(&mut self, pcm_16k_mono: &[f32], opts: &SttOptions) -> Result<Transcript> {
        let audio_ms = (pcm_16k_mono.len() as u64 * 1000 / u64::from(SAMPLE_RATE)) as u32;
        if pcm_16k_mono.is_empty() {
            return Ok(Transcript { text: String::new(), audio_ms, infer_ms: 0 });
        }
        let start = Instant::now();
        let text = self.transcribe_raw(pcm_16k_mono, opts)?;
        Ok(Transcript { text: text.trim().to_string(), audio_ms, infer_ms: start.elapsed().as_millis() as u32 })
    }
}

/// Engine kinds this build can load (manifest `engine` values).
pub fn available_engines() -> Vec<&'static str> {
    let mut v = Vec::new();
    if cfg!(feature = "sherpa") {
        v.extend(["sherpa-nemo-transducer", "sherpa-moonshine"]);
    }
    if cfg!(feature = "whisper") {
        v.push("whisper-cpp");
    }
    v
}

/// Load the engine for a verified model.
#[cfg_attr(not(any(feature = "sherpa", feature = "whisper")), allow(unused_variables))]
pub fn load(paths: &ModelPaths, cfg: &EngineConfig) -> Result<Box<dyn SpeechEngine>> {
    match paths.engine.as_str() {
        #[cfg(feature = "sherpa")]
        "sherpa-nemo-transducer" => Ok(Box::new(sherpa::SherpaEngine::nemo_transducer(paths, cfg)?)),
        #[cfg(feature = "sherpa")]
        "sherpa-moonshine" => Ok(Box::new(sherpa::SherpaEngine::moonshine(paths, cfg)?)),
        #[cfg(feature = "whisper")]
        "whisper-cpp" => Ok(Box::new(whisper::WhisperEngine::load(paths, cfg)?)),
        // Reached only when the matching feature is compiled out.
        #[allow(unreachable_patterns)]
        k @ ("sherpa-nemo-transducer" | "sherpa-moonshine" | "whisper-cpp") => {
            Err(SttError::EngineUnavailable(k.to_string()))
        }
        other => Err(SttError::UnknownEngine(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo;
    impl SpeechEngine for Echo {
        fn id(&self) -> &str {
            "echo"
        }
        fn transcribe_raw(&mut self, pcm: &[f32], _: &SttOptions) -> Result<String> {
            Ok(format!("  {} samples \n", pcm.len()))
        }
    }

    #[test]
    fn transcribe_trims_and_times() {
        let mut e = Echo;
        let t = e.transcribe(&vec![0.0; 16_000], &SttOptions::default()).unwrap();
        assert_eq!(t.text, "16000 samples");
        assert_eq!(t.audio_ms, 1000);
    }

    #[test]
    fn empty_audio_skips_engine() {
        let mut e = Echo;
        let t = e.transcribe(&[], &SttOptions::default()).unwrap();
        assert_eq!(t.text, "");
        assert_eq!(t.audio_ms, 0);
    }

    #[test]
    fn every_manifest_engine_is_known() {
        let known = ["sherpa-nemo-transducer", "sherpa-moonshine", "whisper-cpp"];
        for m in &vt_models::manifest().unwrap().models {
            if m.kind == vt_models::ModelKind::Stt {
                assert!(known.contains(&m.engine.as_str()), "{} uses unknown engine {}", m.id, m.engine);
            }
        }
    }
}
