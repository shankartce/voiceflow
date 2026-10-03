# Models

All inference runs on the CPU. Sizes and speeds below are **expected values to be replaced by
measurements in P1** (`vt-bench run`). Numbers marked † come from upstream releases and are
approximate.

## 1. Speech-to-text candidates (English)

| ID (manifest) | Model | Runtime | Download† | Expected RAM | Expected CPU RTF (4 cores) | License | Role |
|---|---|---|---|---|---|---|---|
| `parakeet-tdt-0.6b-v2-int8` | NVIDIA Parakeet TDT 0.6B v2, int8 ONNX | sherpa-onnx (ONNX Runtime) | ~640 MB | 700–900 MB | 0.03–0.08 | CC-BY-4.0 | **Default** (if P1 confirms) |
| `whisper-base.en-q5_1` | OpenAI Whisper base.en, GGML q5_1 | whisper.cpp | ~60 MB | ~200 MB | 0.05–0.1 | MIT | Low-RAM / slow-CPU fallback |
| `whisper-small.en-q5_1` | Whisper small.en, GGML q5_1 | whisper.cpp | ~190 MB | ~450 MB | 0.15–0.3 | MIT | Accuracy alternative if Parakeet fails on a machine |
| `moonshine-base-int8` | Useful Sensors Moonshine base | sherpa-onnx | ~100–150 MB | ~250 MB | very low | MIT | Optional; evaluated in P1 if cheap to add |

Why Parakeet is the default candidate:
- It tops the open English ASR leaderboards at a fraction of Whisper-large's cost.
- It outputs **punctuation and capitalisation** natively.
- The TDT decoder skips blank frames, so it is very fast on CPU.
- sherpa-onnx ships an int8 export and supports **hotwords** for transducer models, which the
  personal dictionary uses.

Why keep Whisper:
- whisper.cpp is the most battle-tested CPU runtime.
- It is tiny at base.en, so it fits slow or crowded machines.
- `initial_prompt` gives a crude form of dictionary biasing.
- **Whisper weakness:** it can hallucinate text on silence. Mitigated by VAD trimming and by
  refusing to transcribe audio with no detected speech.

Note: Parakeet TDT **v3** is the multilingual (European) successor. It is not useful for
English-only v1, but keep it in mind for the "Later" language work alongside Indic models.

## 2. Voice activity detection

| ID | Model | Size | License | Use |
|---|---|---|---|---|
| `silero-vad-v5` | Silero VAD v5 ONNX | ~2 MB | MIT | Trim silence, detect "no speech", segment long takes, optional auto-stop |

Runs through sherpa-onnx's VAD wrapper (or `ort` directly) at 16 kHz with 32 ms windows. It is
cheap enough to run on every chunk on the pipeline thread.

## 3. LLM candidates (cleanup polish + command mode)

| ID | Model | Quant | File† | RAM (2k ctx) | License | Role |
|---|---|---|---|---|---|---|
| `qwen2.5-0.5b-instruct-q4km` | Qwen2.5-0.5B-Instruct | Q4_K_M GGUF | ~400 MB | ~500 MB | Apache-2.0 | **Default polish** |
| `qwen2.5-1.5b-instruct-q4km` | Qwen2.5-1.5B-Instruct | Q4_K_M GGUF | ~1.0 GB | ~1.2 GB | Apache-2.0 | "Quality" mode + command mode |
| `qwen3-0.6b-q4km` / `qwen3-1.7b-q4km` | Qwen3 (thinking disabled) | Q4_K_M | ~0.4 / ~1.1 GB | similar | Apache-2.0 | P5 bake-off contenders |

Selection rules:
- **Permissive licenses only** for models we offer to download (Apache/MIT/CC-BY). Users may
  import any GGUF themselves.
- Expected CPU generation speed: roughly 30–60 tok/s for 0.5B and 10–25 tok/s for 1.5B. A
  40-word utterance is ~55 output tokens, so **1.5B usually cannot meet a 1.2 s budget.** That
  is why 0.5B is the default polish model, 1.5B is opt-in, and command mode gets a longer budget.
  P5 measures this.

## 4. Manifest (`crates/models/models.toml`)

```toml
[[model]]
id = "parakeet-tdt-0.6b-v2-int8"
kind = "stt"                          # stt | vad | llm
engine = "sherpa-nemo-transducer"     # which vt-stt implementation loads it
license = "CC-BY-4.0"
attribution = "NVIDIA Parakeet TDT 0.6B v2 (…), int8 ONNX export by k2-fsa/sherpa-onnx"
approx_mb = 640
source = { hf_repo = "csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8", revision = "<commit sha>" }

  [[model.file]]
  name = "encoder.int8.onnx"
  role = "encoder"                    # how the engine uses the file
  sha256 = "<64 hex>"
  size = 652000000
  # … decoder, joiner, tokens. A file may set `url = "https://…"` to override the derived URL.
```

- Download URL = `https://huggingface.co/<hf_repo>/resolve/<revision>/<name>`. Pinning a
  **commit** revision makes it immutable.
- **Locking:** an entry with an empty `revision`, `sha256` or `size` is *unlocked*, and
  `fetch`/`import` refuse it. `vt-bench manifest lock` (dev/CI; run by `bench.yml`, which can
  reach Hugging Face) resolves `main` → commit, downloads, hashes, and prints a regenerated
  `models.toml` to commit. `--check` fails if the manifest differs from the downloaded bytes.
- The LibriSpeech benchmark archive is pinned the same way in `bench/golden/libri.toml`.
- URLs point at **upstream** hosts (Hugging Face). We do not re-host weights.
- `vt-models` downloads over HTTPS:
  1. Resumable (`Range`) download to `<file>.part`.
  2. SHA-256 verified while streaming.
  3. Atomic rename.
  4. Writes a `.verified` marker.
  On a mismatch the file is deleted and the user sees a clear error.
- **Offline import:** "Import from folder…" copies the files and verifies them against the
  manifest, so an air-gapped install never needs the network.
- The manifest is compiled into the binary. Updating it means an app release. No remote
  manifest fetch.

## 5. Benchmark method (`vt-bench run`)

- **Golden sets** (`<data dir>/golden/<set>/manifest.toml`, each clip: `id`, audio `file`,
  `reference`, `tags`):
  - `personal`: 40 prompts from `bench/golden/prompts.toml` (tags: slack, email, ai-chat,
    names, numbers, question, long, fast, short), recorded with `vt-bench record`. Stays on
    the user's machine and is never committed.
  - `libri`: 20 LibriSpeech test-clean clips picked by a deterministic rule
    (`bench/golden/libri.toml`), fetched by `vt-bench golden fetch-libri`.
- **WER** (`crates/bench/src/wer.rs`): word-level Levenshtein after normalisation (lowercase,
  strip punctuation, expand contractions, numbers/times/₹/% → words, a few spelling variants).
  Table-tested. **P&C WER** keeps case and `. , ? !` as tokens, which matters for dictation.
  It is reported for the personal set only (LibriSpeech references have no punctuation).
- **Speed:** model load time, then per clip `infer_ms` and RTF = infer ÷ audio, over 3 timed
  runs after 1 warm-up. "Est. wait after 10 s (p95)" = p95 RTF × 10 s.
- **Memory:** peak working set (Windows `GetProcessMemoryInfo`, Linux `VmHWM`) of a fresh
  `vt-bench run-one` process per engine.
- **Output:** `<data dir>/results/<date>-<host>.md` (aggregates only, safe to share) + `.json`
  (includes transcripts, kept private). The founder's report gets committed to `bench/results/`.
- **Decision rule** (ROADMAP P1): lowest WER on `personal` (or `libri` if absent) among
  engines with est. p95 wait ≤ 1.0 s and peak RAM ≤ 1 GB. The report states the pick.

## 6. First-run auto-pick (in the app)

The app can't compute WER on the user's machine (no references), so the auto-pick measures
**speed only**:
1. Run each *downloaded* engine on the bundled 10 s CC-licensed sample (`assets/sample-10s.wav`).
2. Pick the **most accurate engine in our ranking** (from P1 results: Parakeet > small.en >
   base.en) whose measured time is ≤ 1.0 s and whose RAM fits under total RAM × 0.15.
3. The user can override it in Settings → Models, which also offers "Re-run speed check".

The wizard downloads the recommended engine first. Other engines download on demand.
