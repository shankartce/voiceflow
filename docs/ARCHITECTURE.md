# Architecture

> The authority for structure and budgets. If code and this file disagree, fix one of them in
> the same PR.

## 1. Goals that shape everything

1. **Low perceived latency on a CPU-only laptop.** Models stay resident and warm, there is
   exactly one inference thread, and nothing heavy runs on the hook or UI threads.
2. **Small memory footprint on 8 GB RAM.** One STT model resident, the LLM lazy and evictable,
   no Chromium beyond WebView2.
3. **Local by construction.** Network code exists in one crate behind one feature.
4. **Portable core.** Only `vt-platform` (and Tauri glue) knows it's on Windows.

## 2. Repository layout

```
Cargo.toml                 # [workspace], shared deps, lints, release profile (lto = "thin", opt-level = 3)
crates/
  core/      vt-core       # orchestrator + state machine (no Tauri, no Win32)
  audio/     vt-audio      # capture, resample, VAD, levels
  stt/       vt-stt        # SpeechEngine trait + engines
  cleanup/   vt-cleanup    # rules, dictionary, snippets, LLM polish, command mode
  platform/  vt-platform   # traits + windows/ impl + noop/ impl
  storage/   vt-storage    # settings.toml + SQLite (history, dictionary, snippets, stats)
  models/    vt-models     # manifest, downloader (feature "download"), verify, import
  bench/     vt-bench      # dev CLI: fetch / transcribe / run (WER, RTF, RSS)
app/         murmur        # Tauri v2 binary: tray, windows, IPC, wiring
  tauri.conf.json
  src/main.rs  src/ipc.rs  src/tray.rs  src/overlay.rs
ui/                        # Svelte + Vite + TS (settings, history, overlay views)
assets/                    # icons, chimes, a CC-licensed 10 s speech sample for the speed check
bench/golden/              # manifest.toml + reference transcripts (personal audio is .gitignored)
docs/
.github/workflows/         # ci.yml (Linux + Windows), release.yml (P6)
```

### Crate dependency graph (arrows = "depends on")

```
app ──▶ vt-core ──▶ vt-audio
  │        ├──────▶ vt-stt ──────▶ (sherpa-onnx, whisper-rs)
  │        ├──────▶ vt-cleanup ──▶ (llama-cpp-2)
  │        ├──────▶ vt-platform ─▶ (windows crate, cfg(windows))
  │        └──────▶ vt-storage ──▶ (rusqlite bundled, toml, directories)
  └──▶ vt-models (download feature ON — the only place it is enabled)
vt-bench ──▶ vt-audio, vt-stt, vt-cleanup, vt-models
```

`vt-core` does **not** depend on `vt-models`. It receives model paths, already verified, from
`app`.

## 3. Runtime threads

| Thread | Owner | Does | Must never |
|---|---|---|---|
| **Hook thread** | `vt-platform` | `WH_KEYBOARD_LL` + message loop; maps keys → `HotkeyEvent`; posts to channel | block, lock, allocate heavily, log I/O |
| **Audio callback** | cpal (WASAPI) | copies frames into a lock-free ring buffer (`rtrb`); computes RMS for the meter | allocate, lock, resample |
| **Pipeline thread** | `vt-core` | runs the state machine; drains audio; resample + VAD; dispatches jobs | do inference itself |
| **Inference worker** | `vt-core` | owns the loaded `SpeechEngine` and (optionally) the LLM; runs one job at a time | be more than one thread (CPU is shared; ORT/ggml already parallelise internally) |
| **Tauri main/UI** | `app` | tray, windows, IPC, settings | wait on inference |

Channels are `crossbeam-channel`. The pipeline thread is the only writer of dictation state.
The UI receives `StateChanged` / `Level` / `Notice` events through Tauri events.

**Inference threads:** ONNX Runtime and ggml are each given `num_physical_cores` threads,
configurable. Never run STT and LLM at the same time; they are sequential on the worker.

## 4. Dictation state machine (`vt-core`)

```
            HotkeyDown(Hold) / ToggleStart / PillClick
   ┌──────┐ ─────────────────────────────────────────▶ ┌───────────┐
   │ Idle │                                             │ Recording │──Esc / Cancel──▶ Idle (discard)
   └──────┘ ◀──────────────┐                            └───────────┘
       ▲                    │                 HotkeyUp(Hold) / ToggleStop / VAD auto-stop / MaxDuration
       │                    │                                  ▼
       │            InsertDone / Failed(→clipboard)    ┌──────────────┐
       │                    │                          │ Transcribing │──empty text──▶ Idle ("Didn't catch that")
       │             ┌───────────┐   CleanupDone       └──────────────┘
       └─────────────│ Inserting │◀──────────────┐            │ SttDone(raw)
                     └───────────┘               │            ▼
                                            ┌──────────┐
                                            │ Cleaning │  (rules → optional LLM under budget)
                                            └──────────┘
```

Rules:
- **One dictation at a time.** A hotkey press while not `Idle` is ignored and the pill shows
  "busy". Pipelining is a later optimisation.
- **Very short presses** (< 250 ms of audio, configurable) are treated as accidental: discarded,
  nothing inserted.
- **Command mode** follows the same machine with a `Mode::Command { selection }` payload. In
  `Cleaning` it runs the LLM edit instead of the polish (see `CLEANUP.md`).
- Every transition is logged at `debug` with timings (never with transcript text).
- The **target window is captured at `HotkeyDown`** (foreground HWND + process name) and
  re-checked before insert. If focus moved, we still insert into the *current* foreground window
  (that's what users expect) but record the change for diagnostics.

## 5. Core traits (design sketch, finalised in code)

```rust
// vt-stt
pub trait SpeechEngine: Send {
    fn id(&self) -> &str;
    fn transcribe(&mut self, pcm_16k_mono: &[f32], opts: &SttOptions) -> Result<Transcript, SttError>;
    fn supports_hotwords(&self) -> bool { false }
}
pub struct SttOptions { pub hotwords: Vec<String> }
pub struct Transcript { pub text: String, pub duration_ms: u32, pub infer_ms: u32 }

// vt-cleanup
pub struct CleanupContext<'a> { pub dictionary: &'a Dictionary, pub snippets: &'a Snippets, pub settings: &'a CleanupSettings }
pub fn apply_rules(raw: &str, ctx: &CleanupContext) -> RulesOutput;          // pure, instant
pub trait Polisher: Send { fn polish(&mut self, text: &str, budget: Duration) -> PolishResult; } // LLM tier

// vt-platform
pub trait HotkeySource { fn start(&self, bindings: Bindings, tx: Sender<HotkeyEvent>) -> Result<HookHandle>; }
pub trait TextInserter  { fn insert(&self, text: &str, method: InsertMethod) -> Result<(), InsertError>; }
pub trait ClipboardGuard { fn snapshot(&self) -> Result<ClipSnapshot>; fn restore(&self, s: ClipSnapshot) -> Result<()>; }
pub trait ForegroundApp  { fn current(&self) -> Option<AppInfo>; }   // process exe name, title, is_elevated
pub trait SelectionReader { fn read_selection(&self) -> Result<Option<String>>; } // command mode
```

Each trait has a `windows` implementation and a `noop`/`mock` implementation (used on Linux and
in tests). `vt-core` is tested end to end against mocks: fake audio in, captured text out.

## 6. Data and storage

| Data | Location | Format |
|---|---|---|
| Settings | `%APPDATA%\Murmur\settings.toml` | TOML, serde defaults for forward-compat |
| History, dictionary, snippets, stats | `%LOCALAPPDATA%\Murmur\murmur.db` | SQLite (rusqlite, `bundled`), WAL mode, migrations via `user_version` |
| Models | `%LOCALAPPDATA%\Murmur\models\<id>\` | as published + `.verified` marker holding the SHA-256 |
| Logs | `%LOCALAPPDATA%\Murmur\logs\` | rolling, 5 × 5 MB, no transcript text |
| Debug audio (opt-in only) | `%LOCALAPPDATA%\Murmur\debug-audio\` | WAV, auto-deleted after 24 h |

Initial tables: `history(id, created_at, app_exe, raw_text, final_text, words, audio_ms,
latency_ms, polished)`, `dictionary(id, spoken, written, hotword)`, `snippets(id, trigger, body)`,
`stats_daily(day, words, audio_ms, dictations)`. `raw_text` is kept so cleanup changes can be
debugged locally and re-run. Wiping history deletes both columns.

## 7. Latency budget (10 s utterance, rules path)

| Stage | Target |
|---|---|
| Key-up detected → pipeline | < 5 ms |
| Flush audio tail + resample + VAD trim | < 30 ms |
| STT inference (Parakeet int8, 4 threads) | ≤ 500 ms (RTF ≤ 0.05) — validated in P1 |
| Rules cleanup | < 5 ms |
| Wait for modifier release + insert (paste) | < 60 ms |
| **Total p50** | **≤ 700 ms** (p95 ≤ 1.2 s) |

With AI polish: + ≤ 1.2 s hard budget → **≤ 2 s** worst case, then fallback.
**Hands-free mode** (long recordings): audio is segmented at VAD pauses and each segment is
transcribed **while the user is still talking**, so key-up latency only covers the final
segment. The text is still inserted once, at the end.

## 8. Memory budget (8 GB machine)

| Component | Target RSS |
|---|---|
| App shell (Rust + WebView2 for settings; overlay is a tiny webview) | ≤ 150 MB |
| Parakeet TDT 0.6B int8 (ORT) | ~700–900 MB — measured in P1 |
| *or* Whisper base.en q5 / Moonshine base | ~150–300 MB |
| Qwen2.5-0.5B-Instruct Q4_K_M (llama.cpp, 2k ctx) | ~500 MB |
| *or* Qwen2.5-1.5B-Instruct Q4_K_M ("quality" mode) | ~1.2 GB |

The settings and history windows are created on demand and **destroyed on close**, so WebView2
memory is not held. The LLM unloads after 10 min idle (setting). If P1 shows Parakeet over
budget, the first-run check defaults 8 GB machines to Whisper base.en.

## 9. IPC (app ↔ ui)

- Commands (UI → Rust): `get_settings`, `save_settings`, `list_devices`, `test_mic`,
  `list_models`, `download_model`, `import_model`, `run_speed_check`, `history_query`,
  `history_delete`, `dictionary_*`, `snippets_*`, `stats`.
- Events (Rust → UI): `state_changed`, `level` (≤ 30 Hz, overlay only), `notice`,
  `download_progress`.
- All payloads are typed serde structs in `app/src/ipc.rs`, mirrored as TS types in
  `ui/src/lib/ipc.ts` (generated with `specta`/`tauri-specta` if it fits, otherwise hand-kept
  with a test).

## 10. Extension points

- **New STT engine:** implement `SpeechEngine`, add a manifest entry in `vt-models`, register it
  in `vt-stt::registry`.
- **New OS:** add `vt-platform/src/<os>/` implementing the five traits. Nothing else changes
  (Tauri already supports macOS/Linux).
- **New cleanup rule:** add a stage to the ordered pipeline in `vt-cleanup::rules` plus table
  tests.

## 11. CI

- `ubuntu-latest`: fmt, clippy, `cargo test` for every crate except `app` (platform uses noop).
  Also runs the **network-fence check**: `cargo tree -e normal` must show `ureq`/`reqwest`/
  `hyper`/`rustls` only under `vt-models`.
- `windows-latest`: full `cargo build`, `cargo test --workspace`, `cargo tauri build` (unsigned)
  on PRs; signed release on tags (P6).
