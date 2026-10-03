# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

**Murmur** (working name — see "Renaming" below) is an open-source, **fully local** alternative
to Wispr Flow for Windows: hold a hotkey, speak, release, and clean, punctuated text appears in
whatever app has focus. Speech recognition, cleanup and the optional AI polish all run on the
user's CPU. After a one-time model download the app **never touches the network**.

**Read `docs/progress.md` first** (what is built, what is next), then `docs/ARCHITECTURE.md`.
The product spec is `docs/PRD.md`; the phase plan is `docs/ROADMAP.md`.

> Status: **Phase P1 (engine spike).** The workspace exists with `vt-models`, `vt-audio`,
> `vt-stt`, `vt-platform` (memory stats only) and the `vt-bench` CLI. There is no app yet.
> Commands marked *(planned)* don't exist yet. Update this file in the same commit that makes
> one of them real.

## Target environment (design for this, not for a dev workstation)

- **Windows 10 22H2+ / Windows 11, x86_64.**
- **CPU-only laptop, 8 GB RAM**, shared with a browser, Slack and an editor.
- Budgets (from `docs/ARCHITECTURE.md`; that file is the authority):
  - Release-to-text **p50 ≤ 700 ms, p95 ≤ 1.2 s** for a 10 s utterance on the rules path.
  - **≤ 2 s** when the LLM polish runs.
  - Resident RAM **≤ 1 GB** with STT only and **≤ 2 GB** with the LLM loaded.
- English only for v1.

## Commands

```bash
cargo build                                     # whole workspace
cargo test --workspace                          # unit tests (all crates run on Linux)
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all                                 # rustfmt.toml: max_width 120
scripts/network-fence.sh                        # only vt-models may depend on an HTTP client
cargo run -p vt-bench -- models                 # model status
cargo run -p vt-bench -- fetch --all            # download + verify all speech models
cargo run -p vt-bench -- transcribe -e parakeet clip.wav
cargo run -p vt-bench -- golden fetch-libri     # 20 LibriSpeech clips
cargo run -p vt-bench -- record                 # record the personal golden set (needs a mic)
cargo run -p vt-bench --release -- run          # WER + RTF + peak RAM report, per engine
cargo run -p vt-bench -- manifest lock --check  # dev/CI: re-hash and print pinned values
npm --prefix ui install && npm --prefix ui run dev   # (planned) settings UI alone
cargo tauri dev                                      # (planned) full app (Windows only)
cargo tauri build                                    # (planned) NSIS/MSI installer
```

Engine aliases for `-e`/`--engines`: `parakeet`, `moonshine`, `whisper-base`, `whisper-small`.
The data directory defaults to `%LOCALAPPDATA%\Murmur` (Linux: `~/.local/share/Murmur`).
Override it with `--data-dir` or `MURMUR_DATA_DIR`.

CI: `.github/workflows/ci.yml` (Linux lint/test/fence + Windows test, which uploads
`vt-bench-windows-x64` as an artifact) and `bench.yml` (Windows runner: lock check, then all 4
engines on LibriSpeech, failing if any engine's WER > 15%).

Windows dev prerequisites: Rust stable (MSVC toolchain), Visual Studio Build Tools (C++),
CMake, LLVM/clang (bindgen for whisper.cpp / llama.cpp), Node 20+, WebView2 (preinstalled on
Win 11).

**Claude Code sessions usually run on Linux.** Keep every crate except the Windows half of
`vt-platform` and the Tauri `app/` buildable and testable on Linux. Use `#[cfg(windows)]` for
the real implementation and a `noop`/`mock` implementation otherwise. Compile-check Windows code
from Linux with `cargo xwin check --target x86_64-pc-windows-msvc` when `cargo-xwin` is
available. Otherwise rely on the `windows-latest` CI job. **Never claim a Windows-only behaviour
works without it having run on Windows.**

**Cloud sessions can't reach Hugging Face, openslr.org or GitHub release assets** (egress
policy), so models can't be downloaded and sherpa-onnx's static libs can't be fetched there.
Link sherpa-onnx dynamically instead. The npm package `sherpa-onnx-linux-x64@<same version>`
ships the upstream `libsherpa-onnx-c-api.so` + `libonnxruntime.so`:
```bash
mkdir -p /tmp/sherpa && curl -sSL https://registry.npmjs.org/sherpa-onnx-linux-x64/-/sherpa-onnx-linux-x64-1.13.8.tgz | tar xz -C /tmp/sherpa
export SHERPA_ONNX_LIB_DIR=/tmp/sherpa/package LD_LIBRARY_PATH=/tmp/sherpa/package
cargo test --workspace --features vt-bench/sherpa-shared   # also needs: apt-get install libasound2-dev
```
Real inference (models) runs in the `bench.yml` workflow and on the founder's laptop.

## Architecture (the big picture)

```
 hotkey down ─▶ vt-audio: open mic, capture 16 kHz mono  ─▶ overlay pill shows waveform
 hotkey up   ─▶ VAD trim ─▶ vt-stt (Parakeet | Whisper)  ─▶ raw text
             ─▶ vt-cleanup: rules (always) ─▶ LLM polish (optional, time-boxed, guarded)
             ─▶ vt-platform: insert into focused app (clipboard paste ▸ SendInput fallback)
             ─▶ vt-storage: history + stats (text only, local SQLite)
```

| Path | Package | Responsibility |
|---|---|---|
| `crates/core` | `vt-core` | Pipeline orchestrator + dictation state machine. UI-agnostic, no Tauri types |
| `crates/audio` | `vt-audio` | cpal/WASAPI capture, resample, VAD trim, level meter |
| `crates/stt` | `vt-stt` | `SpeechEngine` trait; Parakeet (sherpa-onnx) and Whisper (whisper.cpp) engines |
| `crates/cleanup` | `vt-cleanup` | Rule pipeline, dictionary/snippets, LLM polish, command mode |
| `crates/platform` | `vt-platform` | Hotkey hook, text insertion, clipboard guard, foreground app, selection. Traits + `windows/` impl |
| `crates/storage` | `vt-storage` | Settings (TOML) + history/dictionary/snippets/stats (SQLite) |
| `crates/models` | `vt-models` | Model manifest, **the only network code** (one-time download), SHA-256 verify, offline import |
| `crates/bench` | `vt-bench` | Dev CLI: fetch models, transcribe, WER/latency/RSS benchmark |
| `app/` | `murmur` | Tauri v2 shell: tray, overlay pill, settings window, wiring |
| `ui/` | — | Svelte + Vite + TypeScript frontend for the settings window and overlay |

Packages carry the neutral `vt-` ("voice typing") prefix. A bare `core` would shadow Rust's
built-in `core` crate, and the product name is not final.

Deep dives:
- `docs/MODELS.md`: engines, sizes, licenses, manifest.
- `docs/AUDIO_PIPELINE.md`: capture → VAD → engine.
- `docs/TEXT_INSERTION.md`: hotkeys, paste, SendInput, the app matrix.
- `docs/CLEANUP.md`: rules, LLM prompt, guardrails, command mode.

## Non-negotiables

1. **No network at runtime outside `vt-models`.** Only `vt-models` may depend on an HTTP client,
   and only behind its `download` feature. Nothing else opens a socket: no telemetry, no
   analytics, no crash upload, no update ping. A CI check (`cargo tree -e normal`) fails if any
   other crate pulls in an HTTP/TLS client. Build-time downloads by `-sys` crates are fine. The
   rule is about the shipped binary.
2. **Audio never touches disk** unless the user enables "debug capture" in settings. It lives in
   memory and is dropped after transcription. History stores text only, and it can be disabled
   and wiped.
3. **The keyboard hook callback never blocks.** It classifies the key, posts an event to a
   channel and returns. Windows silently removes a low-level hook that exceeds
   `LowLevelHooksTimeout`, and the app then looks dead. No locks, allocation-heavy work or
   logging I/O inside it.
4. **The user's clipboard is always restored**, on success, failure and panic (RAII guard). Our
   temporary entry is marked to be excluded from clipboard history and cloud clipboard.
5. **The LLM can never cost the user their text.** It runs under a hard time budget. On timeout,
   error or a guardrail rejection, the rules output is inserted. It cleans and never answers:
   dictating "what's the capital of France" inserts that sentence, not "Paris".
6. **Platform APIs stay in `vt-platform`.** `windows`/Win32 calls appear nowhere else. `vt-core`
   talks to traits, so macOS/Linux become new modules, not rewrites.
7. **Models load once and stay warm** on a single inference worker thread. Never load a model
   per dictation. The LLM may load lazily and unload after an idle period (setting).
8. **If insertion fails, the text is not lost.** Leave it on the clipboard and say so in the
   overlay ("Copied — press Ctrl+V"). Covers elevated windows (UIPI) and secure desktops.
9. **Model files are verified** (SHA-256 from the manifest) before first load, and downloads are
   written atomically (temp file + rename).

## Conventions

- Rust stable, edition 2021, workspace-level `[workspace.dependencies]` and lints.
- Errors: `thiserror` in library crates, `anyhow` only in `app/` and `vt-bench`.
  **No `unwrap()`/`expect()` outside tests** (clippy `unwrap_used` = deny in libs).
- Logging: `tracing`. Log to a rolling file in `%LOCALAPPDATA%\Murmur\logs`. **Never log
  transcript text or audio at `info` or above.** Transcripts are user data; `trace` only, and
  off by default.
- Cleanup rules are **table-driven tests** (`input → expected` rows). Every bug report becomes a
  row.
- Settings: one `Settings` struct (serde, `#[serde(default)]` on every field so old files keep
  loading), stored at `%APPDATA%\Murmur\settings.toml`. Models, DB and logs live under
  `%LOCALAPPDATA%\Murmur\`. Use the `directories` crate; never hard-code paths.
- Frontend ↔ backend only through Tauri commands/events defined in `app/src/ipc.rs`. The UI holds
  no business logic.
- Commit messages: imperative subject, body says why.

## Windows gotchas (read before touching `vt-platform`)

- **Hold-to-talk needs key-up events.** `RegisterHotKey` only reports presses, so we use a
  `WH_KEYBOARD_LL` hook on a dedicated thread with its own message loop.
- **The Win key opens the Start menu** when released alone. While our chord is active, swallow
  it and inject a masking key (an unassigned VK such as `0xE8`) before the Win key-up, as
  AutoHotkey does.
- **Wait for modifiers to be physically released** before injecting Ctrl+V or text. Otherwise a
  still-held Win/Alt turns the paste into a shortcut.
- **UIPI:** a non-elevated process cannot inject input into an elevated (admin) window, and the
  failure is silent. Detect an elevated foreground window and fall back to clipboard + notice.
- **The overlay must never take focus.** Create it non-activating (`WS_EX_NOACTIVATE |
  WS_EX_TOOLWINDOW`, click-through when idle). If it steals focus, the text goes into our own
  window.
- **Static C runtime (`/MT`).** sherpa-onnx's prebuilt libs are `/MT`, so `.cargo/config.toml`
  sets `+crt-static` for MSVC. Any new C/C++ dependency must build with `/MT` too (cmake-rs and
  cc honour the flag; a CMake project that sets `CMAKE_MSVC_RUNTIME_LIBRARY` itself may not).
  Otherwise linking fails with `LNK2038 RuntimeLibrary mismatch`.
- **Defender/SmartScreen** dislike unsigned binaries that install keyboard hooks. Release builds
  are code-signed (see ROADMAP P6). Expect a SmartScreen warning on unsigned dev builds.

## Renaming (product name is not decided)

"Murmur" appears in: the docs, `app/Cargo.toml` (package `murmur`), `app/tauri.conf.json`
(`productName`, `identifier`), installer config, and the `%APPDATA%\Murmur` / `%LOCALAPPDATA%\Murmur`
directory name (one constant: `vt_models::APP_DIR_NAME` for now, moving to `vt_storage` in P2).
Crate names are neutral and stay put.

## Licensing

Code: **MIT OR Apache-2.0** (`LICENSE-MIT`, `LICENSE-APACHE`). Model weights are **not** in the
repo. They are downloaded from upstream under their own licenses (`docs/MODELS.md`).
Attributions that require it (Parakeet: CC-BY-4.0) are shown in Settings → About and in
`THIRD_PARTY_NOTICES.md` (created in P6).
