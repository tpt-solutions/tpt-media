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
## Cinder — Lossless Mode

| Algorithm       | Reference                              | Source | License | Patent review | Status   |
| --------------- | -------------------------------------- | ------ | ------- | ------------- | -------- |
| MDCT (N=128)    | Princen & Bradley, 1986                | GREEN  | -       | 2026-09-07 PT | reviewed |
| Sine window     | Oppenheim & Schafer, ch. 7             | GREEN  | -       | 2026-09-07 PT | reviewed |
| Uniform scalar quantizer | Widrow & Kollár, ch. 4         | GREEN  | -       | 2026-09-07 PT | reviewed |
| Static Huffman  | Sayood, "Introduction to Data Compression", ch. 5 | GREEN | - | 2026-09-07 PT | reviewed |
```

## Reed — v0.1

Everything below is implemented from textbook/public-domain
descriptions only (`codecs/reed/src/lib.rs`); no code was derived from
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
| Imcinder-train excitation (voiced frames) | classic LPC vocoder model of the US DoD FS-1015 (LPC-10) family — publicly released federal standard, underlying patents expired | GREEN | - | 2026-09-28 PT | reviewed |
| dB-step gain quantization with an exact-mute index | trivial scalar quantization; the mute-index (exact-silence) convention is this project's own | GREEN | - | 2026-09-28 PT | reviewed |
| PLC by parameter repetition with per-frame gain decay | standard practice (cf. ITU-T G.711 Appendix I concealment philosophy, public recommendation) | GREEN | - | 2026-09-28 PT | reviewed |
| CRC-32 packet trailer | IEEE 802.3 reflected polynomial (public standard), matching the Aura/Cinder family convention | GREEN | - | 2026-09-28 PT | reviewed |

Deliberately *not* used (so the record above stays complete): adaptive
codebooks, long-term predictors, closed-loop analysis-by-synthesis
search, and postfiltering — all appear in the CELP-family literature
and are beyond v0.1 scope; revisiting any of them extends this table
before landing.

## Thrum — v0.1

Everything below is implemented from textbook/public-domain
descriptions only (`codecs/thrum/src/lib.rs`); no code was derived
from any reference implementation. As everywhere in `tpt-media`, no
absolute patent-free claim is made — patent status is jurisdictional
(todo.md §1).

| Algorithm | Reference | Source | License | Patent review | Status |
| --------- | --------- | ------ | ------- | ------------- | ------ |
| 2D DCT-II / DCT-III (separable row+column composition) | Ahmed, Natarajan & Rao (1984), "Discrete cosine transform" | GREEN | - | 2026-09-28 PT | reviewed |
| Intra prediction: DC / vertical / horizontal | H.261 (ITU-T, 1990) and H.263 (1996) intra modes, public recommendations | GREEN | - | 2026-09-28 PT | reviewed |
| Intra prediction: diagonal (top-edge shear) | this project's own simplification; the same "shear the available edge" idea as the H.26x diagonal modes | GREEN | - | 2026-09-28 PT | reviewed |
| Full-pixel motion estimation by SAD | classic full search; early-exit-by-running-best is this project's own optimisation (exact, since the SAD is monotonic in the partial sum) | GREEN | - | 2026-09-28 PT | reviewed |
| Full-pixel motion compensation, edge clamping | H.261/H.263 integer-pel MC, public recommendations | GREEN | - | 2026-09-28 PT | reviewed |
| Macroblock skip | H.263 PQUANT, public recommendation | GREEN | - | 2026-09-28 PT | reviewed |
| Uniform dead-zone scalar quantiser | Widrow & Kollár, "Digital Coding of Waveforms" (1984), ch. 4; dead-zone variants per standard practice in transform codecs | GREEN | - | 2026-09-28 PT | reviewed |
| Geometric `qp` step table | the H.26x convention of a `2^(qp/n)` step, tabulated here rather than computed | GREEN | - | 2026-09-28 PT | reviewed |
| Zigzag coefficient scan | the conventional ordering used across transform image/video codecs (JPEG T.81, public standard) | GREEN | - | 2026-09-28 PT | reviewed |
| Rice coding of run/level pairs, per-block adaptive `k` | Rice (1979), "Some Practical Universal Noiseless Coding Techniques"; implementation is `tpt-media-bitstream::rice` | GREEN | - | 2026-09-28 PT | reviewed |
| Intra refresh (sliding window over macroblocks) | standard periodic-intra practice (cf. H.263 Annex I periodic intra); the sliding-window formulation is this project's own | GREEN | - | 2026-09-28 PT | reviewed |
| 4×4-grid deblocking filter | H.263 loop filter, public recommendation; the two-strength `alpha`/`beta`/`tc` structure follows that description | GREEN | - | 2026-09-28 PT | reviewed |
| 4:2:0 chroma, chroma motion vector = luma vector / 2 | the 4:2:0 sampling relationship itself; common practice in 4:2:0 encoders | GREEN | - | 2026-09-28 PT | reviewed |
| CRC-32 packet trailer | IEEE 802.3 reflected polynomial (public standard), matching the Aura/Cinder/Reed family convention | GREEN | - | 2026-09-28 PT | reviewed |

Deliberately *not* used, so the record above stays complete: sub-pel
motion estimation, multiple reference frames, bi-prediction, weighted
prediction, rate-distortion optimisation, adaptive deblocking strength,
and per-macroblock quantiser adaptation. All appear in the H.26x / MPEG
literature and are beyond v0.1 scope; revisiting any of them extends
this table before landing.

## Contribution Rules

See [`CONTRIBUTING.md`](../CONTRIBUTING.md) §2 for the clean-room rules
every contributor must follow.

## When in Doubt

If you are unsure whether a planned algorithm is GREEN, open an issue
with the `provenance/question` label **before** writing any code. We
prefer to spend time on classification rather than later rewrite
work.