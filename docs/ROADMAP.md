# Roadmap

Each phase ends in something you can **run on the target laptop** and has explicit exit
criteria. Don't start a phase until the previous one's exit criteria are met (or consciously
waived in `docs/progress.md`). The order is chosen to retire the biggest risk first: *is local
CPU speech recognition fast and accurate enough on this machine?*

---

## P0: Planning docs ✅

CLAUDE.md, README, PRD, ARCHITECTURE, ROADMAP, MODELS, AUDIO_PIPELINE, TEXT_INSERTION, CLEANUP,
progress.

**Exit:** docs reviewed by the founder and open questions logged in `progress.md`.

---

## P1: Engine spike & benchmark (CLI only, no UI)

**Goal:** prove the speech engine choice with numbers from the real laptop.

- Cargo workspace skeleton: `vt-audio` (WAV loading + resample only), `vt-stt`, `vt-models`,
  `vt-bench`.
- `vt-models`: manifest (`models.toml`), downloader with resume and SHA-256 verification,
  `import <folder>`.
- `vt-stt`: `SpeechEngine` trait, `ParakeetEngine` (sherpa-onnx), `WhisperEngine` (whisper-rs).
  Moonshine is optional if cheap via sherpa-onnx.
- `vt-bench`:
  - `fetch <model-id>`.
  - `transcribe --engine <id> <wav>`.
  - `run <golden-dir>`: prints a table of engine, WER, RTF p50/p95, load time and peak RSS, and
    writes `bench/results/<date>-<host>.md`.
- Golden set v1:
  - 30–50 clips recorded by the founder: Slack-style, email-style, names/jargon, fast speech,
    a noisy café, laptop mic vs headset.
  - 20 LibriSpeech test-clean clips (CC-BY-4.0) for a public sanity baseline.
  - Personal audio stays out of git (`.gitignore`); only manifests and reference text are
    committed.
- CI: Linux fmt/clippy/test plus the network-fence check.

**Exit criteria**
- A results table committed for the founder's laptop.
- A **default engine chosen by rule**: the lowest WER among engines with p95 ≤ 1.0 s on 10 s
  clips **and** peak RSS ≤ 1 GB. Recorded as a decision in `progress.md`.
- Measured mic-open latency (cpal/WASAPI) on the laptop, which decides whether "warm mic"
  matters (see `AUDIO_PIPELINE.md`).

---

## P2: Core loop MVP (tray app, ugly but real)

**Goal:** daily-drivable hold-to-talk dictation.

- `vt-audio` live capture (cpal/WASAPI, ring buffer, resample, VAD trim).
- `vt-platform::windows`: `WH_KEYBOARD_LL` hold-to-talk (`Ctrl+Win`), Win-key masking, modifier
  release wait, clipboard guard + `Ctrl+V` paste, elevated-window detection → clipboard
  fallback.
- `vt-core`: state machine, inference worker with the model loaded at startup, `Esc` cancel,
  short-press discard.
- `app/`: Tauri v2 tray icon with states, Quit, and a hard-coded settings struct. No overlay
  yet; the tray icon shows state.
- Timing log per dictation (stage timings, no text).

**Exit criteria**
- The founder uses it for 3 working days.
- Insertion works in Notepad, Chrome, Slack, VS Code and Word.
- Clipboard is always restored (verified with a clipboard history check).
- No missed or stuck hotkeys.
- p50 latency within budget on the laptop.

---

## P3: Polish & first-run (it feels like Wispr Flow)

- **Overlay pill**: non-activating, waveform, processing, done and error states, optional
  chimes.
- **Rules cleanup v1** (`CLEANUP.md` stages 1–5): fillers, "scratch that", "new line/paragraph",
  casing/spacing, light number formatting. Table tests.
- **Hands-free toggle** (`Ctrl+Win+Space`), optional VAD auto-stop, max duration, segment-while-
  recording for long takes.
- **SendInput Unicode fallback** plus per-app overrides (method, paste delay) keyed on exe name.
- **Settings window** (Svelte): General, Microphone, Models, Cleanup, Apps, Privacy, About.
- **First-run wizard**: mic test → model download/import → speed check (auto-pick engine) →
  practice box.
- `Paste last transcript` hotkey. Start with Windows.

**Exit criteria**
- The full app matrix in `TEXT_INSERTION.md` passes.
- A new user goes from installer to first dictation in under 3 minutes.
- Idle CPU ≈ 0 %.
- Zero outbound connections after setup (firewall test).

---

## P4: Personalisation & history

- `vt-storage` SQLite: history, dictionary, snippets, stats_daily with migrations.
- **Dictionary**:
  - Spoken → written replacement (rules stage 6).
  - Entries flagged as hotwords are passed to engines that support them (sherpa-onnx transducer
    hotwords; Whisper `initial_prompt`).
- **Snippets**: trigger phrase → body (rules stage 7). Must match the whole utterance or an
  explicit "insert …" prefix to avoid accidental expansion.
- **History window**: search (SQLite FTS5), copy, re-insert, delete, wipe all, off switch.
- **Stats**: words, WPM, time saved.

**Exit criteria:** the dictionary fixes the founder's top 20 misheard names, and history search
returns results in < 50 ms for 10k rows.

---

## P5: Local LLM polish & command mode

- `llama-cpp-2` polisher in `vt-cleanup`:
  - Default model Qwen2.5-0.5B-Instruct Q4_K_M; "quality" option 1.5B.
  - Lazy load, idle unload.
  - Static system-prompt prefix cached in the KV cache.
- **Gating**: the LLM runs only when the rules flag it (correction cues, list cues, long
  utterances) or the user chose "always".
- **Guardrails** (length ratio, word overlap, refusal/answer detection) and the hard budget
  (1.2 s) with fallback to the rules output.
- **Command mode** (`Ctrl+Win+Alt`): guarded `Ctrl+C` selection read → instruction + selection →
  LLM (5 s budget, spinner) → replace selection. No selection means a "Select text first"
  notice.
- `vt-bench` gains a `cleanup` suite: golden raw → expected text, scored for exact match and
  edit distance, with latency.

**Exit criteria**
- Polish improves the founder's subjective rating on the cleanup suite.
- p95 total ≤ 2 s.
- Zero "answered the question" failures on the adversarial set (`CLEANUP.md` §5).
- RAM ≤ 2 GB with the LLM loaded.

---

## P6: Open-source release

- Tauri bundler: NSIS installer (per-user, no admin) plus MSI. No models bundled; first-run
  download/import.
- GitHub Actions `release.yml`: tag → build → sign → GitHub Release with checksums.
- Code signing for OSS (e.g. SignPath Foundation or Azure Trusted Signing), to reduce SmartScreen
  and Defender friction.
- `THIRD_PARTY_NOTICES.md` (crates via `cargo-about`, model attributions), `CONTRIBUTING.md`,
  issue templates, a "how to add an engine" guide.
- Final product name decided; the rename checklist in `CLAUDE.md` applied.

**Exit:** a stranger installs from the Releases page on a clean Windows 11 VM and dictates
successfully with no developer tools installed.

---

## Later (unordered backlog)

- App-aware style (casual in Slack, formal in Outlook, no trailing period in a terminal or code
  editor).
- Hindi / Hinglish (Whisper multilingual or AI4Bharat IndicConformer). The engine trait already
  allows it.
- macOS (`vt-platform::macos`: CGEventTap, AX API, Fn key) and Linux X11/Wayland.
- GPU / NPU acceleration (DirectML / Vulkan) when present.
- Pipelined dictations (start the next recording while the previous one transcribes).
- Streaming partial preview in the pill.
- Auto-update via signed GitHub Releases. **Opt-in only**, consistent with the privacy promise.
