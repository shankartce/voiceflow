# Progress

**Read this first every session.** It is the single source of truth for *what exists*. Update it
in the same commit as the work. Phase definitions and exit criteria live in
[`ROADMAP.md`](ROADMAP.md); this file tracks the checkboxes.

_Last updated: 2026-10-02_

## Current status

**Phase P0 (planning docs): done.** No code exists yet. **Next: P1 (engine spike & benchmark).**

## Decisions log

| Date | Decision | Why |
|---|---|---|
| 2026-10-02 | Windows first, platform code behind traits | Founder's machine; macOS/Linux later without a rewrite |
| 2026-10-02 | Rust + Tauri v2, Svelte settings UI | Small binary, low RAM on an 8 GB laptop, native audio/hooks, OSS-friendly |
| 2026-10-02 | CPU-only, 8 GB RAM target; English only for v1 | Founder's hardware; English unlocks the fastest CPU models |
| 2026-10-02 | Parakeet TDT 0.6B int8 default *candidate*, Whisper as alternative, pluggable | Best speed/accuracy on CPU; **confirmed or overturned by P1 numbers** |
| 2026-10-02 | Tiered cleanup: rules always, local LLM (Qwen2.5-0.5B) optional with gating + budget + guardrails | Wispr-style polish without making every dictation slow |
| 2026-10-02 | Insert on release; hybrid paste + SendInput fallback, per-app overrides | Most reliable across Windows apps; no live typing mess |
| 2026-10-02 | One-time verified model download; no other network code; zero telemetry | "No API calls" requirement |
| 2026-10-02 | Open source, MIT OR Apache-2.0 | Founder's choice; compatible with all chosen runtimes and models |
| 2026-10-02 | Working name "Murmur"; crates neutral (`vt-*`) | Final name undecided; makes the rename cheap |

## P0: Planning docs ✅
- [x] CLAUDE.md
- [x] README.md, LICENSE-MIT, LICENSE-APACHE
- [x] docs/PRD.md, ARCHITECTURE.md, ROADMAP.md
- [x] docs/MODELS.md, AUDIO_PIPELINE.md, TEXT_INSERTION.md, CLEANUP.md
- [x] docs/progress.md
- [ ] Founder review of the docs; answers to the open questions below

## P1: Engine spike & benchmark
- [ ] Cargo workspace skeleton + `[workspace.lints]` + CI (Linux fmt/clippy/test, network-fence check)
- [ ] `vt-models`: manifest, resumable download, SHA-256 verify, import-from-folder
- [ ] Fill in real URLs + SHA-256 for Parakeet int8, whisper base.en/small.en q5_1, Silero VAD
- [ ] `vt-stt`: `SpeechEngine` trait, `ParakeetEngine` (sherpa-onnx), `WhisperEngine` (whisper-rs)
- [ ] (optional) Moonshine via sherpa-onnx
- [ ] `vt-audio`: WAV load + resample (live capture comes in P2)
- [ ] `vt-bench`: `fetch`, `transcribe`, `run` (WER, RTF p50/p95, load time, peak RSS) → `bench/results/`
- [ ] Golden set: 30–50 founder clips + 20 LibriSpeech clips; manifests + references committed
- [ ] Measure WASAPI mic-open latency on the laptop
- [ ] **Decision:** default engine chosen by the rule in ROADMAP P1 and logged above

## P2: Core loop MVP
- [ ] Live capture (cpal/WASAPI → ring buffer → resample → VAD trim)
- [ ] `WH_KEYBOARD_LL` hold-to-talk, chord matcher, Win-key masking, injected-input tag, watchdog
- [ ] Clipboard guard + Ctrl+V paste + modifier-release wait + elevated-target fallback
- [ ] `vt-core` state machine + inference worker (model warm), Esc cancel, short-press discard
- [ ] Tauri tray app with state icons
- [ ] Per-dictation timing log
- [ ] 3-day founder dogfood ✔ / app spot-check (Notepad, Chrome, Slack, VS Code, Word)

## P3: Polish & first-run
- [ ] Overlay pill (non-activating) with waveform/processing/done/error, chimes
- [ ] Rules stages 1–5 + table tests + idempotence test
- [ ] Hands-free toggle, auto-stop, max duration, segment-while-recording
- [ ] SendInput Unicode fallback, per-app overrides, shipped defaults
- [ ] Settings window (Svelte), IPC types
- [ ] First-run wizard + speed-check auto-pick
- [ ] Paste-last hotkey, start with Windows
- [ ] Full app matrix pass; firewall zero-traffic test

## P4: Personalisation & history
- [ ] SQLite schema + migrations
- [ ] Dictionary (stage 6 + engine hotwords) · Snippets (stage 7)
- [ ] History window with FTS5 search, wipe, off switch · Stats

## P5: Local LLM polish & command mode
- [ ] llama.cpp polisher, lazy load / idle unload, prefix KV cache
- [ ] Smart gating, guardrails, 1.2 s budget + fallback
- [ ] Command mode (selection read, 5 s budget)
- [ ] `vt-bench cleanup` + adversarial set at 0 failures

## P6: Open-source release
- [ ] NSIS/MSI bundle, release workflow, code signing
- [ ] THIRD_PARTY_NOTICES.md, CONTRIBUTING.md, issue templates
- [ ] Final name + rename
- [ ] Clean-VM install test

## Open questions (for the founder)
1. **Final product name** (placeholder "Murmur"). Check that the name and domain are free before
   P6.
2. **Golden set:** fine to record ~40 short clips of your own voice for benchmarking? They stay
   local and are never committed.
3. **Laptop specs:** exact CPU model and Windows version, so P1 results can be read against the
   target.
4. **Code-signing route for P6:** SignPath Foundation (free for OSS, needs approval) vs Azure
   Trusted Signing (~$10/month).
5. **Default hotkey:** keep `Ctrl+Win` (Wispr Flow's), or prefer a single key such as Right Alt
   or CapsLock?
