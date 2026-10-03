# vt-bench: run the Phase 1 benchmark on your laptop

`vt-bench` answers one question: **which local speech engine is fast and accurate enough on
*this* machine, for *your* voice?** It downloads four engines once, records you reading 40
short prompts, transcribes everything with each engine, and writes a one-page report with a
recommended default.

Nothing leaves your machine except the one-time downloads below. Your recordings and
transcripts stay in `%LOCALAPPDATA%\Murmur`.

## What you need

- Windows 10/11, x64. No Rust, Visual Studio or Python needed.
- ~2 GB free disk (1.2 GB models + 350 MB LibriSpeech + your clips).
- The microphone you'll actually dictate with, and a quiet-ish room.
- ~45 minutes in total, most of it unattended.

## 1. Get `vt-bench.exe`

1. Open the pull request on GitHub → **Checks** → **CI** → the latest run → **Artifacts** →
   download **`vt-bench-windows-x64`** (a zip; you must be signed in to GitHub).
2. Unzip it, e.g. to `C:\vt-bench`.
3. Open PowerShell in that folder: in Explorer, click the address bar, type `powershell`,
   and press Enter.
4. The exe is not code-signed yet (that's Phase 6), so unblock it once:
   ```powershell
   Unblock-File .\vt-bench.exe
   .\vt-bench.exe --version
   ```
   If SmartScreen still appears: **More info → Run anyway**.

## 2. Download the models (one time, ~1.2 GB)

```powershell
.\vt-bench.exe fetch --all
.\vt-bench.exe models          # all four should say "✓ ready"
```
Every file is checked against a SHA-256 pinned in the source. Interrupted downloads resume.

## 3. Download the public test clips (one time, ~350 MB)

```powershell
.\vt-bench.exe golden fetch-libri
```
20 LibriSpeech audiobook clips: a public baseline to compare against other machines.

## 4. Record your voice (~15 minutes)

```powershell
.\vt-bench.exe record
```
- For each prompt: **Enter** to start, read it naturally (don't say the punctuation),
  **Enter** to stop, then **Enter** to keep or `r` to redo.
- Speak as you would when dictating a message: normal pace, normal distance from the mic.
- `q` quits at any time. Run `record` again later and it continues where you left off.
- Wrong mic? `.\vt-bench.exe record --list-devices`, then `--device "part of its name"`.
- Redo a single prompt: `.\vt-bench.exe record --only p07`.

## 5. Measure the microphone's start-up delay (10 seconds)

```powershell
.\vt-bench.exe mic-latency
```
This tells us whether the app can open the mic when you press the hotkey without clipping
your first word. Copy the last two lines of the output.

## 6. Run the benchmark (~10–20 minutes, unattended)

Plug in the charger (battery saver slows the CPU), close heavy apps, then:
```powershell
.\vt-bench.exe run
```
It prints a table and saves two files in `%LOCALAPPDATA%\Murmur\results\`:

| File | Contains | Share it? |
|---|---|---|
| `<date>-<pc-name>.md` | scores, speeds, memory, recommendation | **Yes**: send this one |
| `<date>-<pc-name>.json` | the same plus every transcript of your recordings | No, keep it private |

Open the folder with `explorer $env:LOCALAPPDATA\Murmur\results`.

## 7. Send back

The `.md` report and the two `mic-latency` lines. That's what decides the default engine.

## Cleaning up

Everything lives in one folder:
```powershell
Remove-Item -Recurse -Force $env:LOCALAPPDATA\Murmur
```

---

## Reading the report

- **WER** (word error rate): % of words wrong, ignoring punctuation, case and number
  formatting. Lower is better. Under 5% feels very good for dictation.
- **P&C WER**: same, but punctuation and capitalisation count. It shows how much you'd fix
  by hand.
- **RTF** (real-time factor): processing time ÷ audio length. 0.05 means 10 s of speech
  takes 0.5 s.
- **Est. wait after 10 s (p95)**: how long you'd wait after releasing the hotkey, in the
  slower 5% of cases. The budget is ≤ 1.0 s.
- **Peak RAM**: the budget is ≤ 1 GB for the speech engine on an 8 GB laptop.
- **Recommendation**: the lowest-WER engine that fits both budgets (docs/ROADMAP.md, P1).

## Developer notes

Build from source (Windows needs VS Build Tools, CMake and LLVM; see CLAUDE.md):
```bash
cargo run -p vt-bench --release -- run
```
`vt-bench manifest lock` (dev/CI only) re-resolves model revisions and prints the hashes to
pin in `crates/models/models.toml` and `bench/golden/libri.toml`.

On machines that can't reach GitHub release assets, link sherpa-onnx dynamically: put
`libsherpa-onnx-c-api.so` + `libonnxruntime.so` (e.g. from the npm package
`sherpa-onnx-linux-x64`, same version) in a folder and build with
`SHERPA_ONNX_LIB_DIR=<folder> cargo build --features vt-bench/sherpa-shared`. Run with
`LD_LIBRARY_PATH=<folder>`.
