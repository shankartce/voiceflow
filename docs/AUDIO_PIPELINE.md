# Audio pipeline (`vt-audio`)

Microphone → 16 kHz mono `f32` PCM, trimmed to speech, handed to the STT engine. The pipeline
is optimised for **never clipping the first word** and **never holding the mic open for no
reason**.

## 1. Flow

```
cpal input stream (WASAPI shared, device native rate e.g. 48 kHz, 1–2 ch, f32/i16)
   │  audio callback: downmix → push into rtrb SPSC ring buffer; compute block RMS → level atomic
   ▼
pipeline thread (vt-core), every ~20 ms while Recording:
   drain ring → rubato resampler (→ 16 kHz mono f32) → append to utterance buffer
   → Silero VAD on 512-sample (32 ms) windows → speech/silence timeline
   │
   on stop: flush resampler tail → trim → (optional) normalise → engine
```

## 2. Device handling

- Default: the Windows **default communications/input device**. Settings let the user pin a
  device by its stable ID. If the pinned device disappears, fall back to the default and show a
  notice.
- **Hot-plug** (headset connected mid-session): device list refreshed on
  `WM_DEVICECHANGE`/cpal errors. A recording in progress on a vanished device ends with an
  "Mic disconnected" notice, and the captured audio up to that point is still transcribed.
- **Microphone privacy:** if Windows Settings → Privacy → Microphone blocks desktop apps,
  opening the stream fails. Detect this and show a notice with a button to open
  `ms-settings:privacy-microphone`.
- Shared mode only (never exclusive), so other apps (Teams, Zoom) keep working.

## 3. Mic open strategy & the first word

Opening a WASAPI stream typically takes ~50–150 ms (measured in P1). Users start talking the
instant they press the key, so the first syllable is at risk.

| Mode | Behaviour | Trade-off |
|---|---|---|
| **Open on press** (default) | Stream opens on `HotkeyDown`; capture starts as soon as the first buffer arrives | Mic indicator only shows while dictating. Small risk of clipping the first ~100 ms |
| **Warm mic** (setting: off / 30 s / 5 min / always) | After a dictation the stream stays open for N seconds. A **pre-roll ring buffer (300 ms)** is prepended to the next dictation | Zero clipping for back-to-back dictations; Windows shows "mic in use" while warm |

Decision rule: if P1 measures stream open ≤ 80 ms on the laptop, default to open-on-press. If
it is slower, default warm mic to 30 s. Never default to "always": a permanently lit mic
indicator undermines the privacy promise.

The chime (if enabled) plays **after** the stream is open, so the sound means "go".

## 4. Resampling & format

- `rubato` FFT/sinc resampler from the device rate to **16 000 Hz mono f32**, in the range
  [-1, 1].
- Downmix stereo by averaging channels in the callback (cheap). Resampling happens on the
  pipeline thread, never in the callback.
- Buffers are preallocated for the max duration so there is no reallocation while recording.

## 5. Limits

- **Hold mode:** max 2 min (setting). On hitting it, stop and process as if released.
- **Hands-free mode:** max 10 min (setting), with a pill countdown in the last 30 s.
- **Minimum:** under 250 ms of captured audio is treated as an accidental press and discarded.

## 6. VAD (Silero v5)

- Thresholds: speech prob ≥ 0.5 starts speech and < 0.35 ends it (hysteresis). Min speech
  250 ms, min silence 300 ms. All tunable in settings under "Advanced".
- **Trim:** keep from (first speech − 200 ms) to (last speech + 300 ms). Padding avoids clipping
  soft onsets and trailing consonants.
- **No speech detected → do not call the engine.** Show "Didn't catch that" in the pill. This
  also prevents Whisper's silence hallucinations ("Thanks for watching!").
- **Auto-stop (hands-free, optional):** stop after N s of continuous silence (default 2.5 s,
  configurable, off by default).

## 7. Long recordings: segment while recording

In hands-free mode, once a VAD silence of ≥ 600 ms follows ≥ 5 s of speech, the speech
segment so far is **sent to the inference worker immediately** while recording continues.
Segments are capped at ~25 s (cut at the lowest-energy point if no pause occurs).

On stop, only the final segment remains to transcribe, so key-up latency stays near the 10 s
budget even for a 5-minute dictation. Segment texts are joined with a space, then cleanup runs
**once** over the full text, so rules like "scratch that" can see across segments.

Hold mode doesn't segment (utterances are short), which keeps it simple.

## 8. Gain

- No AGC by default; Windows and the device already apply it, and double AGC pumps noise.
- Optional "boost quiet mic" setting: peak-normalise the trimmed utterance to −3 dBFS before
  inference (helps Whisper more than Parakeet; measure in P1).

## 9. Level meter

The audio callback computes RMS per block into an `AtomicU32` (f32 bits). The `app` reads it at
≤ 30 Hz and emits `level` events to the overlay. There is no audio data on the UI channel.

## 10. Privacy

- Audio lives only in memory: the ring buffer and the utterance `Vec<f32>`. It is dropped after
  the engine returns, and zeroed on drop for good measure.
- **Debug capture** (off by default; Settings → Privacy → Advanced) writes trimmed WAVs to
  `%LOCALAPPDATA%\Murmur\debug-audio\`, auto-deleted after 24 h, for building the golden set
  or filing bugs.

## 11. Tests

- Unit: resampler output length/rate, downmix, trim padding maths, segmenter decisions, with
  synthetic sine and silence buffers.
- Fixture: VAD on short CC-licensed speech/silence WAVs in `crates/audio/tests/fixtures/`.
- Manual (Windows): device hot-plug, mic privacy blocked, Bluetooth headset (HFP switches the
  rate to 16 kHz and adds latency, which must still work).
