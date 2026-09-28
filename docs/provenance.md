# Provenance and IP Policy

`tpt-media` is implemented from public-domain mathematics only. Every
non-trivial algorithm carries a provenance record identifying its
source, classification, and last patent review.

## Per-Algorithm Record

For each algorithm, we record:

| Field           | Description                                    |
| --------------- | ---------------------------------------------- |
| `algorithm`     | Human name of the algorithm                     |
| `reference`     | Source paper / textbook / expired patent #     |
| `source`        | GREEN / YELLOW / RED classification             |
| `derivation`    | Where in this repo the implementation lives    |
| `implementation`| Link to the source file                        |
| `license`       | License of the reference work                  |
| `patent review` | Date + reviewer initials                       |
| `status`        | reviewed / under-review / blocked              |

## Source Classifications

```text
GREEN    Public-domain mathematics, textbooks, academic papers,
         expired patents, public standards where legally appropriate.

YELLOW   Open-source implementations (with license compatibility),
         modern patents (with explicit avoidance), ambiguous licensing,
         vendor documentation.

RED      Proprietary implementation source, confidential information,
         restricted specifications, unclear ownership.
```

`tpt-media` accepts only **GREEN** sources. YELLOW sources are allowed
only with an explicit waiver from the maintainers and a documented
derivation path. RED sources are forbidden.

## Per-Crate Records

Each codec crate and each non-trivial module has a `PROVENANCE.md`
that records the algorithms it implements. Example:

```markdown
## Pulse — Lossless Mode

| Algorithm       | Reference                              | Source | License | Patent review | Status   |
| --------------- | -------------------------------------- | ------ | ------- | ------------- | -------- |
| MDCT (N=128)    | Princen & Bradley, 1986                | GREEN  | -       | 2026-09-07 PT | reviewed |
| Sine window     | Oppenheim & Schafer, ch. 7             | GREEN  | -       | 2026-09-07 PT | reviewed |
| Uniform scalar quantizer | Widrow & Kollár, ch. 4         | GREEN  | -       | 2026-09-07 PT | reviewed |
| Static Huffman  | Sayood, "Introduction to Data Compression", ch. 5 | GREEN | - | 2026-09-07 PT | reviewed |
```

## Vox — v0.1

Everything below is implemented from textbook/public-domain
descriptions only (`codecs/vox/src/lib.rs`); no code was derived from
any reference implementation. As everywhere in `tpt-media`, no absolute
patent-free claim is made — patent status is jurisdictional (todo.md
§1).

| Algorithm | Reference | Source | License | Patent review | Status |
| --------- | --------- | ------ | ------- | ------------- | ------ |
| Autocorrelation LPC analysis + Levinson-Durbin | Markel & Gray, "Linear Prediction of Speech" (1976), ch. 3–4; Rabiner & Schafer, "Digital Processing of Speech Signals" (1978), ch. 8 | GREEN | - | 2026-09-28 PT | reviewed |
| LSP/LSF representation and LPC↔LSP conversion | Itakura (1975), "Line Spectrum Representation of Speech"; Soong & Juang (1984), "Line Spectrum Pair (LSP) and Speech Data Compression" | GREEN | - | 2026-09-28 PT | reviewed |
| Uniform LSP quantization with ordering safeguard | textbook scalar quantization (Widrow & Kollár) applied to LSPs; ordering necessity per Soong & Juang | GREEN | - | 2026-09-28 PT | reviewed |
| Normalized autocorrelation pitch detection + voicing threshold | Rabiner (1977), "On the Use of Autocorrelation Analysis for Pitch Detection"; implemented in `tpt-dsp-core` (todo.md §7) | GREEN | - | 2026-09-28 PT | reviewed |
| Shape-gain VQ over a fixed stochastic codebook (CELP fixed-codebook idea) | Schroeder & Atal (1985), "Code-excited linear prediction (CELP): High-quality speech at very low bit rates"; the underlying US patent 4,672,628 (filed 1983) has expired | GREEN | - | 2026-09-28 PT | reviewed |
| Deterministic codebook generation (SplitMix64 stream) | Steele, Lea & Flood (OOPSLA 2014), "Fast Splittable Pseudorandom Number Generators"; the splitmix64 code fragment is public domain | GREEN | - | 2026-09-28 PT | reviewed |
| Impulse-train excitation (voiced frames) | classic LPC vocoder model of the US DoD FS-1015 (LPC-10) family — publicly released federal standard, underlying patents expired | GREEN | - | 2026-09-28 PT | reviewed |
| dB-step gain quantization with an exact-mute index | trivial scalar quantization; the mute-index (exact-silence) convention is this project's own | GREEN | - | 2026-09-28 PT | reviewed |
| PLC by parameter repetition with per-frame gain decay | standard practice (cf. ITU-T G.711 Appendix I concealment philosophy, public recommendation) | GREEN | - | 2026-09-28 PT | reviewed |
| CRC-32 packet trailer | IEEE 802.3 reflected polynomial (public standard), matching the Aura/Pulse family convention | GREEN | - | 2026-09-28 PT | reviewed |

Deliberately *not* used (so the record above stays complete): adaptive
codebooks, long-term predictors, closed-loop analysis-by-synthesis
search, and postfiltering — all appear in the CELP-family literature
and are beyond v0.1 scope; revisiting any of them extends this table
before landing.

## Contribution Rules

See [`CONTRIBUTING.md`](../CONTRIBUTING.md) §2 for the clean-room rules
every contributor must follow.

## When in Doubt

If you are unsure whether a planned algorithm is GREEN, open an issue
with the `provenance/question` label **before** writing any code. We
prefer to spend time on classification rather than later rewrite
work.