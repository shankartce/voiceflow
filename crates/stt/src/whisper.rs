//! whisper.cpp engine (via whisper-rs), for GGML Whisper models.

use crate::{EngineConfig, Result, SpeechEngine, SttError, SttOptions, SAMPLE_RATE};
use std::sync::Once;
use vt_models::ModelPaths;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState};

/// whisper.cpp skips audio shorter than ~1 s; pad short clips with silence.
const MIN_SAMPLES: usize = (SAMPLE_RATE as usize * 105) / 100;

pub struct WhisperEngine {
    id: String,
    threads: i32,
    // `state` borrows nothing from `ctx` at the type level, but the context must
    // outlive it; field order keeps it dropped first.
    state: WhisperState,
    _ctx: WhisperContext,
}

impl WhisperEngine {
    pub fn load(paths: &ModelPaths, cfg: &EngineConfig) -> Result<Self> {
        // Route whisper.cpp/ggml logs into the (absent) `log` facade instead of stderr.
        static QUIET: Once = Once::new();
        QUIET.call_once(whisper_rs::install_logging_hooks);

        let load_err = |e: whisper_rs::WhisperError| SttError::Load { model: paths.id.clone(), msg: e.to_string() };
        let ctx = WhisperContext::new_with_params(paths.role("model")?, WhisperContextParameters::default())
            .map_err(load_err)?;
        let state = ctx.create_state().map_err(load_err)?;
        Ok(Self { id: paths.id.clone(), threads: i32::try_from(cfg.threads.max(1)).unwrap_or(4), state, _ctx: ctx })
    }
}

impl SpeechEngine for WhisperEngine {
    fn id(&self) -> &str {
        &self.id
    }

    fn transcribe_raw(&mut self, pcm: &[f32], opts: &SttOptions) -> Result<String> {
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some("en"));
        params.set_n_threads(self.threads);
        params.set_no_timestamps(true);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        let prompt = opts.hotwords.join(", ");
        if !prompt.is_empty() {
            params.set_initial_prompt(&prompt);
        }

        let padded;
        let audio = if pcm.len() < MIN_SAMPLES {
            padded = {
                let mut v = pcm.to_vec();
                v.resize(MIN_SAMPLES, 0.0);
                v
            };
            &padded[..]
        } else {
            pcm
        };

        self.state.full(params, audio).map_err(|e| SttError::Inference(e.to_string()))?;
        let mut text = String::new();
        for seg in self.state.as_iter() {
            let piece = seg.to_str_lossy().map_err(|e| SttError::Inference(e.to_string()))?;
            text.push_str(&piece);
        }
        Ok(text)
    }
}
