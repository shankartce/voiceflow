> **CI runner baseline, a sanity check only.** GitHub `windows-latest` (2 physical cores), bench.yml run
> [37140044816](https://github.com/shankartce/voiceflow/actions/runs/37140044816) on `612c5a7`. The default
> engine is decided on the founder's laptop with the personal set (ROADMAP P1).

# vt-bench results: github-windows-latest (2026-10-03)

| Machine | |
|---|---|
| CPU | AMD EPYC 7763 64-Core Processor |
| Cores | 2 physical / 4 logical |
| RAM | 16.0 GB |
| OS | Windows Server 2025 Datacenter |
| Inference threads | 4 |
| Timed runs per clip | 3 (after 1 warm-up) |
| vt-bench | 0.1.0 (7638cc4f1f) |

## Set `libri`: 20 clips, 3.2 min of audio

| Engine | WER | P&C WER | RTF p50 | RTF p95 | Est. wait after 10 s (p95) | Load | Peak RAM |
|---|---:|---:|---:|---:|---:|---:|---:|
| `parakeet-tdt-0.6b-v2-int8` | 1.0% | n/a¹ | 0.119 | 0.134 | 1341 ms | 2.1 s | 937 MB |
| `moonshine-base-en-int8` | 2.5% | n/a¹ | 0.070 | 0.091 | 907 ms | 1.4 s | 447 MB |
| `whisper-small.en-q5_1` | 2.5% | n/a¹ | 1.019 | 1.767 | 17675 ms | 0.2 s | 516 MB |
| `whisper-base.en-q5_1` | 3.3% | n/a¹ | 0.279 | 0.490 | 4901 ms | 0.1 s | 277 MB |

## Recommendation (ROADMAP P1 rule)

**Default engine: `moonshine-base-en-int8`**. It has the lowest WER on `libri` among engines with an estimated p95 wait ≤ 1.0 s after 10 s of speech and peak RAM ≤ 1 GB.

_Decided on public LibriSpeech audio only. Record the personal set (`vt-bench record`) for the real decision._

**Notes**
- WER ignores case, punctuation and number formatting. P&C WER counts them (what you'd have to fix by hand).
- ¹ LibriSpeech references have no punctuation or casing, so P&C WER is only meaningful on the personal set.
- RTF = inference time ÷ audio length (lower is faster). The estimated wait for 10 s is the p95 RTF × 10 s.
- Peak RAM is the whole benchmark process (model + audio + runtime), measured in a fresh process per engine.
