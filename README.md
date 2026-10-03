# Murmur *(working name)*

**Private voice typing for Windows. Hold a key, speak, and clean text appears wherever your
cursor is. Runs entirely on your own CPU, with no cloud, no account and no API keys.**

An open-source, local-first alternative to [Wispr Flow](https://wisprflow.ai).

> **Status: Phase 1, engine benchmark.** There is no dictation app yet. The repo has the
> design docs and `vt-bench`, a CLI that measures local speech engines on your machine
> ([`bench/README.md`](bench/README.md)). See [`docs/ROADMAP.md`](docs/ROADMAP.md) for the
> plan and [`docs/progress.md`](docs/progress.md) for what is done.

## What it will do

- **Dictate into any app**: Word, Chrome, Slack, Teams, VS Code, Notepad, anything with a text
  cursor.
- **Hold-to-talk** (`Ctrl+Win`), **hands-free toggle** (`Ctrl+Win+Space`), or click the tray
  icon or floating pill. `Esc` cancels. All hotkeys are configurable.
- **Clean output**: punctuation, capitalisation and filler words ("um", "uh") removed, plus
  spoken commands like "new paragraph" and "scratch that".
- **Optional local AI polish**: a small on-device language model handles self-corrections
  ("at 5, no, 6pm" → "at 6pm") and list formatting. It always has a time budget and never
  rewrites your meaning.
- **Command mode**: select text, hold `Ctrl+Win+Alt` and say "make this more formal" or "turn
  this into bullet points".
- **Personal dictionary & snippets**: teach it names and jargon. Say "my address" to insert
  your full address.
- **Local history & stats**: search past dictations, re-paste them, see words per minute.
  Stored only on your disk, and can be disabled.

## Privacy promise

1. Speech recognition and AI cleanup run **on your machine**.
2. The **only** network activity is a one-time, checksum-verified model download on first run
   (or import the model files from a folder for fully offline installs).
3. **No telemetry, analytics, crash reporting or update pings.**
4. Audio is held in memory and discarded after transcription. It is never saved unless you turn
   on debug capture.
5. Your clipboard is restored after every insertion.

These are engineering rules enforced in code and CI. See [`CLAUDE.md`](CLAUDE.md#non-negotiables).

## Requirements (target)

- Windows 10 22H2+ or Windows 11 (x64)
- Any modern 4-core CPU. **No GPU needed.**
- 8 GB RAM (the app aims to use ≤ 1 GB, or ≤ 2 GB with AI polish on)
- ~1–2 GB disk for models

## How it works

```
mic ─▶ voice-activity trim ─▶ speech-to-text (NVIDIA Parakeet or Whisper, on CPU)
    ─▶ cleanup rules ─▶ optional local LLM polish ─▶ pasted into the focused app
```

Built with Rust + Tauri v2. Speech engines: [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx)
(Parakeet TDT) and [whisper.cpp](https://github.com/ggerganov/whisper.cpp). LLM:
[llama.cpp](https://github.com/ggerganov/llama.cpp). Details are in
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

## Documentation

| Doc | What's in it |
|---|---|
| [`docs/progress.md`](docs/progress.md) | Live status checklist |
| [`docs/PRD.md`](docs/PRD.md) | Product spec: users, stories, UX, success metrics |
| [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) | Crates, threads, state machine, budgets |
| [`docs/ROADMAP.md`](docs/ROADMAP.md) | Phases P0–P6 with exit criteria |
| [`docs/MODELS.md`](docs/MODELS.md) | Speech + LLM model choices, sizes, licenses |
| [`docs/AUDIO_PIPELINE.md`](docs/AUDIO_PIPELINE.md) | Microphone → engine |
| [`docs/TEXT_INSERTION.md`](docs/TEXT_INSERTION.md) | Hotkeys and getting text into any Windows app |
| [`docs/CLEANUP.md`](docs/CLEANUP.md) | Rules, AI polish, command mode |

## Building from source

```bash
cargo test --workspace
cargo run -p vt-bench --release -- --help
```
Windows needs Visual Studio Build Tools (C++), CMake and LLVM. See [`CLAUDE.md`](CLAUDE.md#commands).
To just run the benchmark, use the prebuilt `vt-bench.exe` ([`bench/README.md`](bench/README.md)).

## License

Code is dual-licensed under **MIT OR Apache-2.0**, at your option. Model weights are downloaded
from their publishers and carry their own licenses (see [`docs/MODELS.md`](docs/MODELS.md)).
