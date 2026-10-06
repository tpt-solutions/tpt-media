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

Per-crate records live in this document (sections below) rather than in
per-crate `PROVENANCE.md` files — there are deliberately none. The shape
such a record takes:

```markdown
## Cinder — Lossless Mode

| Algorithm       | Reference                              | Source | License | Patent review | Status   |
| --------------- | -------------------------------------- | ------ | ------- | ------------- | -------- |
| MDCT (N=128)    | Princen & Bradley, 1986                | GREEN  | -       | 2026-09-07 PT | reviewed |
| Sine window     | Oppenheim & Schafer, ch. 7             | GREEN  | -       | 2026-09-07 PT | reviewed |
| Uniform scalar quantizer | Widrow & Kollár, ch. 4         | GREEN  | -       | 2026-09-07 PT | reviewed |
| Static Huffman  | Sayood, "Introduction to Data Compression", ch. 5 | GREEN | - | 2026-09-07 PT | reviewed |
```

The real per-crate records follow.

## Cinder — v0.1

Implemented from textbook/public-domain descriptions only
(`codecs/cinder/src/lib.rs`); no code was derived from any reference
implementation. As everywhere in `tpt-media`, no absolute patent-free
claim is made — patent status is jurisdictional (todo.md §1).

| Algorithm | Reference | Source | License | Patent review | Status |
| --------- | --------- | ------ | ------- | ------------- | ------ |
| MDCT analysis/synthesis (N=128/256) | Princen & Bradley (1986), "Analysis/Synthesis Filter Bank Design Based on Time Domain Aliasing Cancellation"; implementation is `tpt-dsp-core`'s `FastMdctPlan` (todo.md §5.1) | GREEN | - | 2026-09-28 PT | reviewed |
| Sine analysis/synthesis window | Princen & Bradley (1986), same paper — the `h[i]²+h[i+n]²=1` condition | GREEN | - | 2026-09-28 PT | reviewed |
| Uniform scalar quantizer with per-band integer step | textbook scalar quantization (Widrow & Kollár, "Quantization Noise", 2008, ch. 4) | GREEN | - | 2026-09-28 PT | reviewed |
| Fixed-width level coding (no entropy coder) | the trivial baseline, deliberately: v0.1 codes each level in `bits_per_coef` bits — Rice/range/Huffman live in `tpt-media-bitstream` for a future mode | GREEN | - | 2026-09-28 PT | reviewed |
| Optional threshold-in-quiet band weighting | Terhardt (1979), "Calculating Virtual Pitch" (absolute-threshold component); implementation is `tpt-dsp-core::psychoacoustic::threshold_in_quiet` (todo.md §10) | GREEN | - | 2026-09-28 PT | reviewed |
| No packet integrity trailer | v0.1 predates the family CRC-32 convention Aura/Reed/Thrum adopted; retrofitting one is a wire-format break, so the limitation is documented rather than silently patched — per-cluster/packet integrity is the container's job for Cinder streams today | GREEN | - | 2026-09-28 PT | reviewed (documented limitation) |

Deliberately *not* used, so the record above stays complete: entropy
coding of levels, mid/side or intensity stereo, a bit reservoir, window
switching, and noise shaping — all standard in MDCT audio codecs and
all beyond the low-latency v0.1 scope; revisiting any of them is a
version-bump event for the wire format.

## Aura — v0.1

Everything below is implemented from textbook/public-domain
descriptions only (`codecs/aura/src/lib.rs`); no code was derived from
any reference implementation. As everywhere in `tpt-media`, no absolute
patent-free claim is made — patent status is jurisdictional (todo.md
§1).

| Algorithm | Reference | Source | License | Patent review | Status |
| --------- | --------- | ------ | ------- | ------------- | ------ |
| Autocorrelation LPC analysis + Levinson-Durbin (order ≤ 32) | Markel & Gray, "Linear Prediction of Speech" (1976), ch. 3–4; implemented in `tpt-dsp-core` (todo.md §6) | GREEN | - | 2026-09-28 PT | reviewed |
| Q15 coefficient quantization + shared integer prediction (lossless bit-exactness) | the standard lossless-audio arrangement — predict with the *quantized* coefficients over exactly-reconstructed history so prediction error only shrinks the residual, never changes decode (Shorten/FLAC-family practice; this implementation is independent) | GREEN | - | 2026-09-28 PT | reviewed |
| Zigzag-mapped Exp-Golomb residual coding | Golomb (1966), "Run-length encodings"; the exponential-Golomb variant and zigzag sign mapping per ITU-T H.264 (public standard) | GREEN | - | 2026-09-28 PT | reviewed |
| MDCT analysis/synthesis (2048/4096) | Princen & Bradley (1986), as for Cinder; implementation is `tpt-dsp-core`'s `mdct`/`imdct` | GREEN | - | 2026-09-28 PT | reviewed |
| Bark-scale critical-band mapping | Traunmüller (1990), "Analytical Expressions for the Tonotopic Sensitivity Scale"; implementation is `tpt-dsp-core::bark` (todo.md §10.1) | GREEN | - | 2026-09-28 PT | reviewed |
| Tonal/noise classification | simplified from ISO/IEC 11172-3 (MPEG-1) psychoacoustic model 1's tonality determination — a local-peak-with-prominence test, documented as such in `tpt-dsp-core` (todo.md §10.2) | GREEN | - | 2026-09-28 PT | reviewed |
| Simultaneous masking (spreading function) | Schroeder (1979), "Optimizing Digital Speech Coders by Hearing Masks"; implementation is `tpt-dsp-core::psychoacoustic` (todo.md §10.2) | GREEN | - | 2026-09-28 PT | reviewed |
| Threshold in quiet | Terhardt (1979), as for Cinder | GREEN | - | 2026-09-28 PT | reviewed |
| Masking-matched quantizer step selection | the standard psychoacoustic quantization philosophy of ISO/IEC 11172-3 — choose the step whose uniform-quantizer noise (`step²/12`) matches the band's combined masking threshold | GREEN | - | 2026-09-28 PT | reviewed |
| First-order spectral noise shaping | delta-sigma error feedback (`NTF = 1 − z⁻¹`), Inose & Yasuda (1962); implementation is `tpt-dsp-core::noise_shaping` (todo.md §11) | GREEN | - | 2026-09-28 PT | reviewed |
| Transmitted per-band step table (f32 per Bark band) | this project's own convention: side information rides in the packet so the decoder needs no shared state — chosen over an implicit step model for exactly reproducible decode | GREEN | - | 2026-09-28 PT | reviewed |
| CRC-32 packet trailer | IEEE 802.3 reflected polynomial (public standard), the Aura/Reed/Thrum family convention | GREEN | - | 2026-09-28 PT | reviewed |

Deliberately *not* used, so the record above stays complete: stereo or
mid/side coding, temporal masking, window switching / short blocks, a
bit reservoir, entropy coding of the quantized spectrum (fixed 16-bit
indices today), and scalefactor-band splitting below the Bark resolution
— all appear in the MPEG-4 AAC literature and are beyond v0.1 scope;
revisiting any of them extends this table and bumps the wire version
before landing.

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

## Mote — v0.1

Implemented from textbook/academic descriptions only
(`codecs/mote/src/`); no code was derived from any external reference
implementation, and no external VQ or video codec source was read. The one
in-repo precedent is `tpt-media-research`'s `kmeans.rs` (same project and
licence): Mote's trainer follows the same textbook Lloyd shape but is a
separate implementation (integer centroids, different init and re-seating
details), and the codec does not depend on the research crate. As everywhere in `tpt-media`, no absolute patent-free claim is
made — patent status is jurisdictional (todo.md §1). The source classification is
GREEN for every row, but **no patent review has been recorded yet**: every row
is `pending` / `under-review` until a maintainer signs it off, unlike the
Thrum table above. Dictionary
(vector-quantisation) image coding and conditional replenishment are decades
old; the specific combination below is this project's own.

| Algorithm | Reference | Source | License | Patent review | Status |
| --------- | --------- | ------ | ------- | ------------- | ------ |
| Vector quantisation (nearest-codeword encode, table-lookup decode) | Gray, "Vector quantization", IEEE ASSP Magazine (1984); Linde, Buzo & Gray, "An algorithm for vector quantizer design", IEEE Trans. Commun. (1980) | GREEN | - | pending | under-review |
| Nearest-vector search with partial-distance early exit | partial distance search (Bei & Gray, "An improvement of the minimum distortion encoding algorithm for vector quantization", 1985); implementation is `tpt_dsp_core::nearest_vector_accelerated`, reused rather than reimplemented | GREEN | - | pending | under-review |
| Codebook training: Lloyd iteration with integer-rounded centroids | Lloyd, "Least squares quantization in PCM" (1957/1982); LBG (1980). Integer rounding of centroids each iteration is this project's own choice, made for exact cross-platform determinism | GREEN | - | pending | under-review |
| Empty-cluster re-seating on the farthest training vector | standard k-means/LBG practice (cluster splitting); deterministic ordering is this project's own | GREEN | - | pending | under-review |
| Seeded distinct-sample initialisation (partial Fisher-Yates) | Fisher & Yates (1938), Durstenfeld (1964) | GREEN | - | pending | under-review |
| SplitMix64 generator (training initialisation only) | Steele, Lea & Flood, "Fast splittable pseudorandom number generators", OOPSLA (2014) | GREEN | - | pending | under-review |
| Image partitioned into fixed 8x8 patches, per-patch VQ | block-based image VQ, Nasrabadi & King, "Image coding using vector quantization: a review", IEEE Trans. Commun. (1988) | GREEN | - | pending | under-review |
| Conditional replenishment (code only changed blocks, copy the rest from the previous frame) | Mounts, "A video encoding system with conditional picture-element replenishment", Bell Syst. Tech. J. (1969), a long-expired technique | GREEN | - | pending | under-review |
| Skip decision "previous copy no worse than best codeword" | this project's own rate-distortion shortcut (coding a patch no better than the copy spends bits for nothing) | GREEN | - | pending | under-review |
| Golomb-Rice coding of skip runs, per-frame `k` | Golomb, "Run-length encodings" (1966); Rice (1979) | GREEN | - | pending | under-review |
| PSNR / mean absolute error quality metric | textbook definitions | GREEN | - | pending | under-review |
| CRC-32 packet trailer | IEEE 802.3 reflected polynomial (public standard), matching the family convention | GREEN | - | pending | under-review |

Deliberately *not* used, so the record above stays complete: any
transform (DCT/wavelet), motion estimation or compensation, entropy coding
of the index stream, product/tree/lattice quantisation, learned or neural
codebooks, and any post-filter. All appear in the literature and are beyond
v0.1 scope; revisiting any of them extends this table before landing.

## Vault — v0.1

Implemented from the literature and public recommendations listed below
(`codecs/vault/src/`). No source code of any reference codec was read or
referred to while writing it, and no code or structure was copied from
one. As everywhere in `tpt-media`, no absolute patent-free claim is made —
patent status is jurisdictional (todo.md §1).

The **patent review** column is deliberately *not* filled in for this
crate: the implementer recorded the sources, but a maintainer patent
review has not happened, so every row is `under-review` rather than
`reviewed`. Block-partitioned video coding is a heavily patented area; the
choices that reduce exposure (CONTRIBUTING §3) are a 2-tap/4-tap bilinear
sub-pel filter, static Rice coding with no context adaptation, and the
absence of CABAC, long interpolation filters and angular intra modes.

| Algorithm | Reference | Source | License | Patent review | Status |
| --------- | --------- | ------ | ------- | ------------- | ------ |
| Quadtree partitioning of a block into four equal children | Samet (1984), "The quadtree and related hierarchical data structures"; Sullivan & Baker (1994), "Efficient quadtree coding of images and video", IEEE Trans. Image Processing | GREEN | - | not reviewed | under-review |
| Bottom-up leaf-versus-split comparison by a Lagrangian cost | Chou, Lookabaugh & Gray (1989), "Optimal pruning with applications to tree-structured source coding and modeling", IEEE Trans. Information Theory (the encoder compares node-by-node with early termination instead of pruning a fully grown tree) | GREEN | - | not reviewed | under-review |
| `J = D + lambda * R` Lagrangian objective | Everett (1963), "Generalized Lagrange multiplier method"; Sullivan & Wiegand (1998), "Rate-distortion optimization for video compression", IEEE Signal Processing Magazine. The constant `LAMBDA_SCALE` is empirical and untuned | GREEN | - | not reviewed | under-review |
| Multiple reference frames, per-block reference index | Wiegand, Zhang & Girod (1999), "Long-term memory motion-compensated prediction", IEEE Trans. Circuits Syst. Video Technol. The reference list here is only the last N pictures | GREEN | - | not reviewed | under-review |
| Full-pel shrinking-step 8-neighbour pattern search | logarithmic / three-step search: Jain & Jain (1981), "Displacement measurement and its application in interframe image coding"; Koga et al. (1981), "Motion-compensated interframe coding for video conferencing". Seeding from the zero and predicted vectors is this project's own | GREEN | - | not reviewed | under-review |
| Half-pel bilinear motion compensation (2-tap; 4-sample diagonal) | H.263 (ITU-T, 1996) half-pel prediction, public recommendation | GREEN | - | not reviewed | under-review |
| Motion-vector difference coding against a left/top neighbour predictor | the idea of predictive vector coding is in H.261/H.263 (public recommendations); the left-else-top rule is this project's own simplification of H.263's median predictor | GREEN | - | not reviewed | under-review |
| Intra DC / vertical / horizontal prediction | H.261 / H.263 intra prediction, public recommendations | GREEN | - | not reviewed | under-review |
| Intra "plane" prediction (`left + top - top_left`, clamped) | linear extrapolation of the plane through three neighbours — elementary mathematics; also the classic gradient predictor of lossless image coders | GREEN | - | not reviewed | under-review |
| 2D DCT-II / DCT-III at 4..32 (separable row+column), via `tpt-dsp-core` | Ahmed, Natarajan & Rao (1984), "Discrete cosine transform" | GREEN | - | 2026-10-05 PT | reviewed |
| Fast transform path (`tpt-dsp-core::FastDct2dPlan`, std builds): DCT-II/III via a length-`2n` inverse FFT + half-twiddle | the standard FFT-based DCT derivation; the zero-padding formulation is textbook | GREEN | - | 2026-10-05 PT | reviewed |
| B-frames: bi-predictive leaves (average of two motion-compensated references), lookahead groups, POC reordering | the classical B-frame model of H.261/H.263/H.26x (public recommendations); averaging combine, POC-based reordering | GREEN | - | 2026-10-05 PT | reviewed |
| Orthonormal scaling of DCT coefficients before quantisation | Rao & Yip (1990), *Discrete Cosine Transform: Algorithms, Advantages, Applications* (Parseval / orthonormal basis) | GREEN | - | not reviewed | under-review |
| Uniform dead-zone scalar quantiser, rounding offset below 0.5 | Widrow & Kollár, "Digital Coding of Waveforms" (1984), ch. 4; dead-zone / rounding-offset practice in transform codecs | GREEN | - | not reviewed | under-review |
| Geometric `qp` step table | the H.26x convention of a `2^(qp/n)` step, tabulated (shared with Thrum) | GREEN | - | not reviewed | under-review |
| Diagonal zig-zag scan generalised to `n x n` | JPEG T.81 (public standard) zig-zag, extended to other sizes | GREEN | - | not reviewed | under-review |
| Rice coding, unsigned run form and signed level form, per-block parameter | Rice (1979), "Some Practical Universal Noiseless Coding Techniques"; signed form is `tpt-media-bitstream::rice`, the unsigned form is a few lines local to `codecs/vault/src/bits.rs` | GREEN | - | not reviewed | under-review |
| Run / last-position coefficient coding | run-length coding of zig-zag scans as in JPEG T.81 and H.263 (public); the `last` field as terminator is this project's own | GREEN | - | not reviewed | under-review |
| Macroblock skip (predicted vector, no residual) | H.263 COD/skip, public recommendation | GREEN | - | not reviewed | under-review |
| Deblocking filter | H.263 Annex J "Deblocking Filter Mode" (ITU-T, 1998), public recommendation; the boundary-strength selection between units is this project's own | GREEN | - | not reviewed | under-review |
| 4:2:0 chroma, chroma vector = luma vector / 2 | the 4:2:0 sampling relationship itself | GREEN | - | not reviewed | under-review |
| CRC-32 packet trailer | IEEE 802.3 reflected polynomial (public standard), matching the family convention | GREEN | - | not reviewed | under-review |

Deliberately *not* used, so the record above stays complete: CABAC or any
context-adaptive entropy coding, 6/8-tap sub-pel filters, quarter-pel
motion, angular (directional) intra modes, B-frames
and bi-prediction, weighted prediction, long-term reference management,
rate control, coefficient-level RDO / trellis quantisation, and
per-block quantiser adaptation. Revisiting any of them extends this table
before landing.

## Contribution Rules

Development is issues only — external code contributions are not
accepted (see [`CONTRIBUTING.md`](../CONTRIBUTING.md)). §2 there holds
the clean-room rules every line of the project's code — whoever writes
it — must follow.

## When in Doubt

If you are unsure whether a planned algorithm is GREEN, open an issue
with the `provenance/question` label **before** writing any code. We
prefer to spend time on classification rather than later rewrite
work.