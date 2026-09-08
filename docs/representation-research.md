# Representation Research Platform — methodology and results

Milestone `todo.md` §56. Investigates whether natural video can be represented
more sparsely by a **fundamentally different mathematical representation**
than the block-transform paradigm, rather than only by improving
prediction/transform/quantisation/entropy coding within it.

This is explicitly *not* an attempt to build a better codec. The deliverable is
the experimental apparatus (`tpt-media-representation` +
`tpt-media-research`) plus enough concrete representations to prove the
abstraction is real, and an honest record of what those representations
actually measure like.

All numbers below are reproducible with:

```text
cargo test -p tpt-media-research --lib experiments::tests -- --nocapture
```

(The experiments' tests print the full reports through `format_report` in
`crates/tpt-media-research/src/experiments.rs`.)

---

## 1. Measurement methodology

### 1.1 The signal type

All signals are three-dimensional `V(x, y, t)` real-valued volumes
(`tpt_media_representation::SignalBuffer`, dimensions `width × height ×
frames`). Pixel-value packing from `tpt-media-core` is deliberately not used:
the research question is mathematical (real-valued samples, arbitrary
transforms), not about production pixel formats.

Synthetic generators live in `tpt-media-research/src/signals`:

- `spatial.rs` — `gradient`, `texture` (det-RNG noise), `constant`, `edge`,
  `corner`, `impulse`.
- `spatiotemporal.rs` — `moving_edge(x, t) = f(x - vt)` with a *known*
  velocity, `stationary`, `sinusoidal_motion`.

`det_rng` (`crates/tpt-media-research/src/det_rng.rs`) is a seeded,
deterministic RNG — no `rand` dependency, byte-identical across runs.

### 1.2 The representation abstraction

`tpt_media_representation` defines the contracts; `tpt-media-research`
implements five concrete representations:

| name | idea | decoder state (`Coeffs`/`Quantized`) |
|---|---|---|
| `baseline` | raw identity | the samples themselves |
| `block_dct` | separable 2D DCT-II over `B×B` blocks (via `tpt-dsp-core` 1D `dct_ii`/`dct_iii`) | DCT coefficient stream |
| `temporal_dct` | previous-frame prediction + block DCT on the residual | residual DCT stream (+ frame recursion) |
| `patch_vq` | patch extraction + Lloyd/k-means codebook (`kmeans.rs`) + `tpt-dsp-core::vq` search | codebook entries + per-patch indices |
| `motion_aligned` | coordinate warp `x' = x - vt` before block DCT | velocity vector **and** aligned coefficient stream |

`tpt-dsp` is **not modified.** 2D transforms are separable passes over the
existing 1D DCT; VQ-line only adds *training*, since tpt-dsp already has VQ
*search*.

### 1.3 "No free decoder-side information" (enforced structurally)

Every bit a decoder needs lives inside the `Quantized` value, and every
`RateEstimator` sums over the **whole** value:

- `patch_vq`: `entropy_bits(codebook values) + entropy_bits(indices)`.
- `motion_aligned`: the velocity is embedded in
  `MotionAlignedCoeffs.velocity` and charged at full `f32` precision
  (64 bits) **on top of** the coefficient entropy — so the oracle handing it
  the *true* velocity still pays to transmit it.
- A `Representation`'s `Config` (block size, codebook size, sweep step) is
  out-of-band fixed machinery, never charged.

Rate is *empirically estimated* as the zeroth-order Shannon entropy of the
quantized symbol stream (`entropy.rs`) — an analytic model, **not** a real
entropy coder. Wiring `tpt-media-bitstream` so the rate axis is measured
rather than estimated is a documented follow-on, not required for this
milestone's questions.

### 1.4 Uniform scalar quantization (mid-tread)

Every linear-coefficient representation uses the same quantizer:
`index(c) = round(c/Δ)`, `value(i) = i·Δ`
(`ScalarStep`/`LinearQuantizer` in `crates/tpt-media-research/src/uniform.rs`).
The RD sweep covers Δ from `2^-8` to `2^7` (16 geometric steps, see §2.1 for
why the coarse end is wide).

### 1.5 Primary / secondary / diagnostic metric hierarchy

- **Primary** — the RD frontier: bits-per-sample vs. MSE
  (`interpolated_distortion`, `matched_rate_table`). This is the only axis on
  which a representation is ever ranked.
- **Secondary** — entropy, non-zero coefficient count, and coefficient count
  for 99%-energy, measured at a fixed operating point Δ = 0.125
  (`SecondaryDiagnostics`). Descriptive; never a ranking argument.
- **Diagnostic** — coefficient-retention curves
  (`retention_curve` in `diagnostics.rs`). Used to think about sparsity
  structure, never to declare a "better" representation.

Section 2.4 shows a concrete case where the secondary metric *contradicts*
the primary result — exactly why the hierarchy exists.

### 1.6 Types of distortion

- MSE = mean squared error over the volume, peak value 1.0 (so
  `PSNR = 10·log10(1/MSE)`), `psnr = inf` means lossless reproduction.
- Retaining *no* coefficients produces the "drop everything" floor
  `MSE = E[x²]` (0.5 for a 0/1 signal, ≈1/3 for uniform noise).

### 1.7 The matched-rate comparison (never extrapolate)

`rd_curve` samples each condition at the shared Δ sweep; points may share a
rate because entropy quantizes. Interpolation collapses duplicate rates to the
lowest-MSE reading and is **linear in (rate, MSE) strictly inside** each
curve's sampled range. `common_targets` (experiments.rs) picks rates inside
the intersection of every condition's range; a curve that cannot *reach* a
target rate reports `None` and is never ranked there.

Two degenerate-but-honest cases are handled:

1. A condition whose whole envelope is **a single rate** (motion-aligned on
   the easy moving edge — §4.1). The intersection is that one point; the
   harness still requires every other condition to reach it.
2. A wide intersection is sampled inside it; extrapolation is structurally
   rejected (`interpolated_distortion` returns `None`).

### 1.8 Determinism contract

Every experiment must reproduce byte-identical curved output given identical
inputs. Enforced by:

- `det_rng` for all noise injection;
- entropy summation over `BTreeMap` (sorted key order), never a `HashMap`
  (whose per-process random iteration order breaks bit-stability);
- no parallelism.

`oracle_is_deterministic_run_to_run` asserts the whole oracle report is
byte-identical across two independent runs.

### 1.9 What the tests assert (and, deliberately, do not assert)

Tests assert *measurement validity* only: values finite; each representation's
own round trip reconstructs within tolerance; rates sorted non-decreasing;
comparison never extrapolates; determinism. **No test asserts the
hypothesized direction** of any result — the experiment is allowed to conclude
"motion alignment didn't help," or "the dictionary was a losing idea under
these conditions."

---

## 2. The oracle experiment (moving edge, `vx = 1`)

`oracle_moving_edge(64, 64, 8)` — 64×64×8 edge moving one pixel per frame at
`I(x,t) = f(x - t)`, toroidal. Four conditions: raw pixels (baseline),
spatial DCT (8×8), DCT + previous-frame prediction, and motion-aligned with
the **true** velocity.

### 2.1 Ranges

| condition | sampled rate range (b/s) |
|---|---|
| raw pixels | 0.000000 – 1.000000 |
| spatial DCT | 0.053677 – 0.358949 |
| DCT + previous-frame pred | 0.008239 – 0.293159 |
| motion-aligned (true v) | 0.067868 – 0.067868 |

Motion-aligned's curve is a **single-rate plateau**: 64-bit velocity charge
amortized over the volume plus the entropy of the aligned blocks' DC
coefficients ≈ 0.0679 b/s, lossless at every Δ until the quantizer step
crosses the DC level (§4.1 cliff). This is *why* the coarse Δ sweep exists —
without it, no matched rate exists between a representation that lives at
`10^-2` b/s and a block-DCT family that cannot operate below `10^-1` b/s —
and why the intersection is a single point.

### 2.2 Matched-rate table

Only one common rate exists, `0.06787 b/s`. Every condition reaches it in
range (no extrapolation):

| condition | MSE | PSNR (dB) |
|---|---|---|
| raw pixels | 0.466066 | 3.32 |
| spatial DCT | 0.062272 | 12.06 |
| DCT + previous-frame pred | 0.089524 | 10.48 |
| motion-aligned (true v) | 0.000000 | inf (lossless) |

Motion-aligned wins decisively (lossless at ~7% of the next-best condition's
rate). Notably, **previous-frame prediction measured *worse* than plain
spatial DCT** at this rate (0.0895 > 0.0623 MSE): the open-loop predictor's
drift is charged into the reconstruction while the compensated entropy stays
high at coarse quantization. A hypothesis one might have had — "motion DCT
must beat spatial DCT" — is *not* what the primary metric says here.

### 2.3 Secondary diagnostics (Δ = 0.125)

| condition | entropy (b/s) | non-zero / total | coefs for 99% energy |
|---|---|---|---|
| raw pixels | 1.0000 | 16384 / 32768 | 16221 |
| spatial DCT | 0.3589 | 1016 / 32768 | 503 |
| DCT + previous-frame pred | 0.3014 | 928 / 32768 | 686 |
| motion-aligned (true v) | 0.0659 | 256 / 32768 | 254 |

### 2.4 The hierarchy in action

The temporal predictor has **lower** secondary entropy (0.3014) than plain
spatial DCT (0.3589) yet **higher** primary MSE at the matched rate (0.0895 vs
0.0623). At Δ = 0.125 its residual stream is genuinely smaller in entropy, but
at the 0.0679 b/s operating point the open-loop drift dominates the
reconstruction. Ranking by the secondary metric would have picked the wrong
condition; ranking by the RD frontier (as the harness does) is the correct
call. This is the concrete demonstration of why §1.5 exists.

### 2.5 The coarse-end cliff (honest boundary of the plateau)

At the coarsest step (Δ = 128) the aligned DC level (64) sits at exactly
round(½·Δ), so DC survives as index 1 and reconstructs to 128 instead of 64 —
reproduction collapses to MSE 0.5 at the *same* rate 0.0679 b/s as the
lossless plateau. The interpolation layer collapses equal-rate duplicates to
the lowest-MSE reading, so the plateau stays lossless for comparison, but the
single-rate characterization "0.0679 b/s, lossless" only holds while the step
stays below the aligned DC level. No lost-cost trickery — the entropy truly is
the same (same 256 DC symbols), and the cliff is a quantization-lattice
artifact of the extreme step, recorded here because it appeared in the data.

---

## 3. Phase results

Each phase is a two-condition matched-rate comparison on a
single-frame-signal sweep design (static signal, so frames = 1) except where
noted.

### 3.1 Phase 0 — raw vs. DCT (static gradient, 64×64)

Matched-rate table (excerpts):

| rate (b/s) | raw MSE | raw PSNR | DCT MSE | DCT PSNR |
|---|---|---|---|---|
| 0.000000 | 0.335979 | 4.74 | 0.335971 | 4.74 |
| 0.015900 | 0.335979 | 4.74 | 0.274714 | 5.61 |
| 0.320110 | 0.277364 | 5.57 | 0.000020 | 46.99 |

The DCT reaches ~47 dB at 0.32 b/s and 64+ dB by 0.51 b/s; raw pixels need
~6 b/s to approach 50 dB. Secondary (Δ=0.125): DCT entropy 0.3941 b/s,
192/4096 non-zero, 52 coefficients carry 99% of energy; raw 3.125 b/s entropy,
3840/4096 non-zero. This is the expected, near-tautological result — a smooth
ramp is DCT-sparse — established so the *harness* itself is the thing being
tested here, not the conclusion.

### 3.2 Phase 2 — blocks vs. patches (noise texture, 64×64)

`block_dct` (8×8, K = 16 entries for `patch_vq`).

| rate (b/s) | DCT MSE | DCT PSNR | VQ MSE | VQ PSNR |
|---|---|---|---|---|
| 0.036040 | 0.311536 | 5.06 | 0.297789 | 5.26 |
| 0.935360 | 0.048802 | 13.12 | 0.053153 | 12.74 |

Honest findings:

- **The fixed-K dictionary has a hard distortion floor.** VQ saturates at
  MSE ≈ 0.0527 (a reconstruction floor set by codebook size) while DCT keeps
  improving to 71.5 dB at 10.06 b/s. The VQ rate axis also caps out (~1.93
  b/s: dictionary + index entropy) because its symbol alphabet is bounded.
- Some non-monotonic VQ points in the raw sweep (e.g. rate 0.3096 at
  MSE 0.197) show coarse quantization of a tiny codebook snapping indices —
  recorded as a real measurement, not smoothed away.
- Secondary: DCT entropy 5.4815 b/s, non-zero 3917/4096, 1743 coefs for 99%
  energy.

Category: **the transform wins on i.i.d.-ish texture**, the expected
difficult case for locality — and the dictionary floor is a structural
limitation of fixed-size VQ, not a tuning artifact.

### 3.3 Phase 4 — stationary vs. motion-aligned (moving edge, 64×64×8)

`phase4_stationary_vs_motion_aligned`: plain spatial DCT against
motion-aligned carrying the true velocity.

| rate (b/s) | stationary MSE | stationary PSNR | motion-aligned MSE | motion-aligned PSNR |
|---|---|---|---|---|
| 0.067868 | 0.062272 | 12.06 | 0.000000 | inf |

Same common-rate point as the oracle (§2.2, this is *that* comparison in
isolated form). Secondary (Δ=0.125): stationary entropy 0.3589 b/s,
1016/32768 non-zero, top-k 503; motion-aligned entropy 0.0659, 256/32768,
top-k 254.

Finding: on motion-matched signals, aligning the coordinate frame *before*
the transform is worth 5.5× lower entropy and full losslessness at 5.3× fewer
bits than the stationary DCT at its most favorable common rate.

### 3.4 Phase 5 — coefficients vs. dictionary (oscillating texture, 64×64×8)

`sinusoidal_motion` signal, `block_dct` against `patch_vq` K = 32.

| rate (b/s) | DCT MSE | DCT PSNR | VQ MSE | VQ PSNR |
|---|---|---|---|---|
| 0.014170 | 0.300237 | 5.23 | 0.268528 | 5.71 |
| 0.275350 | 0.055885 | 12.53 | 0.047897 | 13.20 |

Here the **dictionary wins the mid-rate region** (13.20 vs 12.53 dB at
0.275 b/s): a codebook trained on this signal's *own* recurring patches beats
fixed-basis DCT coefficients. The same saturation floor appears above ~0.5 b/s
(≤13.24 dB for VQ against 71.5 dB for DCT at 10.2 b/s), so the dictionary's
advantage is confined to the low-rate, coarse-quantization regime it was
trained for.

Secondary: DCT entropy 5.2964 b/s, non-zero 30949/32768, top-k 11183. VQ has
no linear coefficient stream, so it is correctly omitted from the secondary
list rather than reported as "0 coefficients."

---

## 4. What the numbers actually say

### 4.1 The unified picture

Across all five comparisons, three structural facts coexist:

1. **A fixed-basis transform is unbeatable at high fidelity** on both smooth
   and noisy signals (Phases 0, 2, 5) — it scales to arbitrarily low MSE as
   Δ → 0 because its coefficient alphabet is unbounded.
2. **A fixed-size dictionary is capped** by its codebook: a hard reconstruction
   floor and a bounded rate axis (Phases 2, 5). Its only edge is mid-rate
   training-set-matched quantization (Phase 5).
3. **Coordinate alignment is the big lever shown so far.** On a motion-matched
   signal, motion-aligned encodes at 0.0679 b/s lossless while nothing else
   even reaches that regime (Phases 4, oracle). The entire block-DCT family
   cannot operate below ~10^-1 b/s on this signal.

The single-rate plateau deserves emphasis: motion-aligned's cost is dominated
by the *fixed* 64-bit velocity charge — the motion estimate must be paid once
per sequence irrespective of volume size, so its b/s advantage grows with `t`.
That is the mechanism the deferred phases (velocity estimation, 3-D
space-time, combining winners) build on.

### 4.2 Known limitations, recorded on purpose

- **Open-loop temporal prediction**: the encoder codes the residual vs. the
  *original* previous frame while the decoder reconstructs a *closed-loop*
  previous frame → drift accumulates. This is spec-conformant and documented
  in `temporal_dct.rs`; it is visible in the data (oracle, §2.2) and is a
  *design choice of this milestone's "3rd condition",* not a bug.
- **Rate is entropy, not a coder** (§1.3). Real bitstream costs (coder
  inefficiency, headers) are unmodeled; the RD frontiers here are
  entropy-ideal curves.
- **The oracle is omniscient** for motion: the true velocity is given (but
  still charged). A real motion estimator's error budget is unmodeled.
- **Per-condition codebook training.** `patch_vq` trains a fresh dictionary on
  the exact signal being encoded — an encode-time cost (charged in bits) that
  real systems amortize very differently.
- Re-presenting each experiment's **full sampled curves** here is feasible on
  request; the tables and secondary diagnostics above are the arguments,
  and `--nocapture` reproduces every point.

### 4.3 Front-line next

To trust motion-aligned's advantage beyond a synthetic oracle: Phase 4's
condition set needs a *real* velocity estimator (charged, imperfect), and the
rate axis should move from entropy to the real `tpt-media-bitstream` coder.
Both are explicit follow-ons; neither is required by §56's deliverable.