# PRD: Murmur (working name)

Local, private, Wispr-Flow-style voice typing for Windows.

## 1. Problem

Typing is the bottleneck for writing messages, emails, docs and prompts. Wispr Flow shows that
"hold a key, talk, get clean text anywhere" is a big upgrade, but it sends your voice to the
cloud, needs an account and a subscription, and doesn't work offline. Windows' built-in voice
typing (`Win+H`) is also cloud-backed, does no cleanup, and is clumsy in many apps.

**We want Wispr Flow's feel with none of the cloud:** everything runs on an ordinary 8 GB, CPU-only
laptop.

## 2. Users

| Persona | Needs |
|---|---|
| **The founder (primary)**: dictates all day into Slack, email, docs, and AI chat boxes | Speed, accuracy on names/jargon, zero friction, works offline |
| **Privacy-conscious professional**: lawyer, doctor, journalist, enterprise employee | Guarantee that audio never leaves the machine; no account |
| **OSS tinkerer** | Hackable engines/rules, reproducible benchmarks, good docs |
| **RSI / accessibility user** | Hands-free mode, reliable insertion, low fatigue |

## 3. Principles

1. **Instant beats clever.** The rules path must feel instant. AI polish is a bonus and is
   never allowed to make the experience slow.
2. **Never lose the user's words.** Any failure ends with the text on the clipboard.
3. **Invisible until needed.** A tray icon and a small pill overlay; no window pops up while
   dictating.
4. **Local by construction**, not by setting. There is no network code path to turn on.
5. **Faithful, not creative.** Cleanup fixes form, never meaning.

## 4. Core user stories (v1)

| # | As a user I want to… | Acceptance |
|---|---|---|
| U1 | Hold `Ctrl+Win`, speak, release, and see text in the focused app | Works in the app matrix (`TEXT_INSERTION.md`); p50 ≤ 700 ms after release for 10 s of speech |
| U2 | Dictate long text hands-free | `Ctrl+Win+Space` starts; same chord or the pill stops; optional auto-stop after N s silence; up to 10 min |
| U3 | Cancel a bad take | `Esc` while recording discards audio; nothing inserted |
| U4 | Get punctuation and no fillers | "um so I think we should uh ship it" → "So I think we should ship it." |
| U5 | Use spoken commands | "new paragraph", "new line", "scratch that" behave as documented in `CLEANUP.md` |
| U6 | Teach names/jargon | Dictionary entry `"get banaya" → "GetBanaya"` applied every time; also used as engine hotwords where supported |
| U7 | Expand snippets | Saying "insert my address" inserts the saved address |
| U8 | Fix self-corrections (AI polish on) | "let's meet at 5, no, 6pm" → "Let's meet at 6pm." |
| U9 | Edit text by voice (command mode) | Select text, hold `Ctrl+Win+Alt`, say "make it formal", and the selection is replaced |
| U10 | Find something I said | History window: search, copy, re-insert; delete one/all; can be disabled |
| U11 | See my stats | Words dictated, average WPM, time saved vs typing at 40 WPM |
| U12 | Set up in under 3 minutes | First-run wizard: mic test → model download (or import) → hotkey test in a practice box |
| U13 | Never lose text on failure | If insertion fails (elevated window etc.), text is on the clipboard and the pill says so |

## 5. UX

### 5.1 Surfaces

- **Tray icon**: states idle / listening / processing / error. Menu: Start dictation, History,
  Settings, Pause hotkeys, Quit.
- **Pill overlay**: small rounded bar, bottom-centre above the taskbar, always on top, **never
  takes focus**.
  - Idle: hidden, or a faint dot if the "show idle pill" setting is on (clicking it starts
    toggle mode).
  - Listening: live waveform (mic level) plus a timer in toggle mode.
  - Processing: shimmer.
  - Done: brief ✓ and fade.
  - Error / fallback: short message ("Copied, press Ctrl+V", "Didn't catch that",
    "Mic unavailable").
- **Settings window** (Svelte) with these tabs:
  - General: hotkeys, start with Windows, sounds, idle pill.
  - Microphone: device, test meter.
  - Models: engine, download/import, benchmark.
  - Cleanup: rules toggles, AI polish on/off, quality vs speed.
  - Dictionary & Snippets.
  - Apps: per-app insertion method and paste delay.
  - Privacy: history on/off, wipe, debug capture.
  - About: licenses, attributions.
- **History window**: search box, list, copy/re-insert/delete, stats header.

### 5.2 Default hotkeys (all rebindable)

These follow Wispr Flow's Windows layout so muscle memory carries over.

| Action | Default |
|---|---|
| Push-to-talk (hold) | `Ctrl+Win` |
| Hands-free toggle | `Ctrl+Win+Space` |
| Command mode (hold) | `Ctrl+Win+Alt` |
| Cancel | `Esc` while recording |
| Paste last transcript | `Alt+Shift+Z` |

### 5.3 Sounds

Optional soft start/stop chimes. They are short and play asynchronously, so they never delay
mic open.

## 6. Non-goals for v1

- Live streaming text into the target app (we insert once, on release).
- Languages other than English (Hindi/Hinglish is a "Later" item).
- macOS / Linux builds. The architecture keeps them possible.
- Cloud sync, accounts, mobile apps.
- App-aware tone/style per app (Later). Per-app *insertion method* is in v1.
- GPU acceleration (welcome later; never required).
- Speaker diarisation, meeting transcription, file transcription UI.

## 7. Success metrics

| Metric | Target | Measured by |
|---|---|---|
| Release-to-insert latency (10 s speech, rules path) | p50 ≤ 700 ms, p95 ≤ 1.2 s | `vt-bench` + in-app timing log |
| With AI polish | p95 ≤ 2 s; budget overrun falls back, never waits | same |
| Accuracy | WER on the golden set ≤ engine's published baseline + 2 pts | `vt-bench run` |
| Resident memory | ≤ 1 GB (STT), ≤ 2 GB (STT + LLM) | `vt-bench` peak RSS + Task Manager |
| Insertion success | 100 % of the app matrix (excluding documented elevated cases) | manual matrix each release |
| Network after setup | **zero** connections | Windows Firewall outbound block + Resource Monitor / Wireshark |
| Idle CPU | ~0 % (no polling; mic closed when idle unless "warm mic" is on) | Task Manager |
| Cold start to ready | ≤ 5 s including model load | in-app timing log |

## 8. Risks

| Risk | Mitigation |
|---|---|
| Parakeet too heavy for 8 GB / slow CPUs | Pluggable engines; first-run speed check falls back to Whisper base.en / Moonshine |
| CPU LLM too slow for the budget | 0.5B default, only invoked when the rules detect it's needed, static prompt prefix cached, hard timeout |
| Hook flagged by antivirus | Code signing, documented behaviour, no injection into other processes |
| Insertion quirks in specific apps | Per-app overrides (paste vs type, paste delay), compatibility matrix in docs |
| Win key opening the Start menu | Masking-key technique (see `TEXT_INSERTION.md`) |
| LLM "answers" a dictated question | Strict cleanup prompt + guardrails + fallback (see `CLEANUP.md`) |
