//! sherpa-onnx (ONNX Runtime) engines: NVIDIA Parakeet TDT and Moonshine.

use crate::{EngineConfig, Result, SpeechEngine, SttError, SttOptions, SAMPLE_RATE};
use sherpa_onnx::{
    OfflineMoonshineModelConfig, OfflineRecognizer, OfflineRecognizerConfig, OfflineTransducerModelConfig,
};
use std::path::Path;
use vt_models::ModelPaths;

pub struct SherpaEngine {
    id: String,
    recognizer: OfflineRecognizer,
}

fn s(p: &Path) -> Option<String> {
    Some(p.to_string_lossy().into_owned())
}

fn base_config(paths: &ModelPaths, cfg: &EngineConfig) -> Result<OfflineRecognizerConfig> {
    let mut c = OfflineRecognizerConfig::default();
    c.model_config.tokens = s(paths.role("tokens")?);
    c.model_config.num_threads = i32::try_from(cfg.threads.max(1)).unwrap_or(4);
    c.model_config.provider = Some("cpu".into());
    c.model_config.debug = false;
    c.decoding_method = Some("greedy_search".into());
    Ok(c)
}

impl SherpaEngine {
    pub fn nemo_transducer(paths: &ModelPaths, cfg: &EngineConfig) -> Result<Self> {
        let mut c = base_config(paths, cfg)?;
        c.model_config.transducer = OfflineTransducerModelConfig {
            encoder: s(paths.role("encoder")?),
            decoder: s(paths.role("decoder")?),
            joiner: s(paths.role("joiner")?),
        };
        c.model_config.model_type = Some("nemo_transducer".into());
        Self::create(paths, &c)
    }

    pub fn moonshine(paths: &ModelPaths, cfg: &EngineConfig) -> Result<Self> {
        let mut c = base_config(paths, cfg)?;
        c.model_config.moonshine = OfflineMoonshineModelConfig {
            preprocessor: s(paths.role("preprocessor")?),
            encoder: s(paths.role("encoder")?),
            uncached_decoder: s(paths.role("uncached_decoder")?),
            cached_decoder: s(paths.role("cached_decoder")?),
            merged_decoder: None,
        };
        Self::create(paths, &c)
    }

    fn create(paths: &ModelPaths, c: &OfflineRecognizerConfig) -> Result<Self> {
        let recognizer = OfflineRecognizer::create(c).ok_or_else(|| SttError::Load {
            model: paths.id.clone(),
            msg: "sherpa-onnx could not create the recognizer (see stderr for details)".into(),
        })?;
        Ok(Self { id: paths.id.clone(), recognizer })
    }
}

impl SpeechEngine for SherpaEngine {
    fn id(&self) -> &str {
        &self.id
    }

    fn transcribe_raw(&mut self, pcm: &[f32], _opts: &SttOptions) -> Result<String> {
        let stream = self.recognizer.create_stream();
        stream.accept_waveform(SAMPLE_RATE as i32, pcm);
        self.recognizer.decode(&stream);
        stream.get_result().map(|r| r.text).ok_or_else(|| SttError::Inference("sherpa-onnx returned no result".into()))
    }
}
