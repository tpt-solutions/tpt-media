# TPT Media — Master Implementation TODO

**Repository:** `tpt-media`
**Organization:** TPT Solutions
**Language:** Rust
**Status:** Early Implementation
**Document version:** 2.1
**Last updated:** 2026-09-07

---

## Known issues (as of 2026-09-07)

* ~~**Range coder is broken.**~~ **Correction (2026-09-07, later same day):**
  re-ran `cargo test -p tpt-media-bitstream --lib range` 5× — all 4 range
  tests (`encode_decode_uniform`, `encode_decode_skewed`, and 2 others)
  pass consistently, and the test inputs are fully deterministic (no
  randomness), so this wasn't flakiness either. The original "fails its
  own round-trip tests" finding from earlier today could not be
  reproduced. Flipped §17's `range coder` box back to done; treat the
  original finding as unconfirmed/stale rather than re-flagging it without
  evidence.
* ~~**`Timestamp::rescale` overflow detection is wrong.**~~ **Fixed
  2026-09-07 (later same day):** the conversion used `self.timebase.num`
  in both factors of the numerator, which both produced wrong results
  and masked real overflows. Math re-derived from first principles,
  corrected to `new = ticks · self.timebase.num · target.den /
  (self.timebase.den · target.num)`, and the existing
  `rescale_overflow_returns_none` test (which used `i64::MAX` µs → 90
  kHz and fit even with the bug) replaced with one whose inputs
  actually overflow `i64`, plus a new sanity check asserting 1 s → 90
  kHz = exactly 90 000 ticks. See `CHANGELOG.md` `[Unreleased] / Fixed`.
* ~~**Aura's residual coding is Exp-Golomb, not Rice.**~~ **Fixed
  2026-09-07 (later same day):** brought the doc comment into line
  with the code (Exp-Golomb-coded residuals with `write_exp_golomb` /
  `read_exp_golomb` in-crate, zigzag-mapped). Switching the wire
  format to Rice would be a separate, breaking change and is still
  tracked as the §974 unchecked item.
* ~~**`CHANGELOG.md`'s `[Unreleased]` entry overstates scope.**~~
  **Fixed 2026-09-07 (later same day):** removed the
  not-yet-on-disk claims about `apps/`, `ffi/`, `tpt-cli`, `tpt-web`,
  `tpt-media-sys`, and `tpt-video-{livelink,vault,whisper}`, replaced
  with an honest note that they're target structure pending
  implementation, and added a `### Fixed` section summarising the
  `Timestamp::rescale` and Aura doc-comment corrections.
* `cargo test --workspace` currently has 0 failing test binaries
  (the `rescale` bug above was the last one — `tpt-media-core`'s
  timebase module and `tpt-media-bitstream` both pass in full).
* **2026-09-07 tpt-dsp reconciliation**: verified §3–§14 against the
  sibling `tpt-dsp` repo's actual code (`tpt-dsp-core/src/*.rs`). MDCT
  (§5.1) and LPC (§6) are genuinely implemented and consumed by
  Pulse/Aura — flipped below. Pitch analysis (§7), VQ (§8), LSP/LSF
  (§9), psychoacoustics (§10), and noise shaping (§11) were **entirely
  absent** from `tpt-dsp-core` at the time — confirmed by grep, no
  matches. **Update, same day, later sessions**: all five have since
  been implemented (see their own sections below); this line is kept for
  history rather than silently deleted.
* **2026-09-07 — a real sign error in the first noise-shaping
  implementation (§11), caught by a test, not by review.** The first
  version fed back the past quantization error by *adding* it
  (`shaped = input + predicted_error`); this compiles fine and even
  passes a shallow smoke test, but for a constant input it converges to
  a fixed point after one step and gets stuck there forever (a
  degenerate limit cycle) — it doesn't do anything a plain memoryless
  quantizer doesn't. A DC-tracking test (checking the shaped output's
  long-run average recovers the true constant, the classic delta-sigma
  demonstration) failed with `shaped avg error = plain avg error`
  exactly, which forced re-deriving the standard `NTF=1-z⁻¹` formula from
  scratch — the correct sign *subtracts* the fed-back error. Worth
  remembering: a noise-shaping/error-feedback filter with the wrong
  feedback sign still type-checks, still runs, and can still look
  plausible in a quick check; only a test of the property the filter is
  actually *for* (here, DC-averaging accuracy) caught it.
* **2026-09-07 (later same day) — fixed a real precision bug in
  `tpt-dsp-core` that affects Pulse/Aura directly.** Implementing
  `FastMdctPlan`/`FastDctIvPlan` (FFT-backed, see §5.1/§5.2 below) and
  cross-checking them against the existing direct `mdct`/`imdct`/`dct_iv`
  turned up a real disagreement at N=1024+ (Pulse/Aura's actual block
  sizes) — not a bug in the new fast path, but in the **existing** direct
  reference: it evaluated `cos()` at the raw, unreduced `(π/N)·x·y` angle,
  which reaches thousands of radians at these block sizes and is poorly
  conditioned for `f32`/`f64` `cos()`. Fixed in `tpt-dsp` via an
  integer-domain angle-reduction helper (`complex::cos_pi_over_n`) applied
  to all five direct transforms (`dct_ii`/`iii`/`iv`, `mdct`/`imdct`);
  `tpt-media`'s Pulse/Aura tests still pass against the corrected
  reference. This means Pulse/Aura at 1024+ block sizes were silently
  producing a measurably wrong spectrum before this fix — worth a mental
  note if `tpt-dsp`'s version isn't yet bumped/pulled everywhere this
  workspace's `Cargo.lock` might have cached the old code.

---

# 0. Purpose

`tpt-media` is the native TPT media framework and codec ecosystem.

It provides:

* media data models
* timestamps and synchronization
* packet/frame abstractions
* codec interfaces
* media pipelines
* bitstream primitives
* container formats
* native TPT codecs
* media probing
* CLI tooling
* WebAssembly playback
* stable FFI

`tpt-media` does **not** attempt to become a general-purpose DSP library.

General signal-processing and mathematical primitives belong in `tpt-dsp`.

`tpt-kinetix` remains the standards-oriented media implementation project.

The intended ecosystem is:

```text
                         TPT APPLICATIONS
                               │
              ┌────────────────┼────────────────┐
              │                │                │
       tpt-government      tpt-kinetix      tpt-media
                                             │
                                      Native TPT Media
                                             │
                                             ▼
                                          tpt-dsp
```

More specifically:

```text
tpt-media
│
├── Media representation
├── Codecs
├── Containers
├── Bitstreams
├── Pipelines
├── Synchronization
└── I/O integration
        │
        ▼
    tpt-dsp
        │
        ├── transforms
        ├── filters
        ├── signal analysis
        ├── codec DSP
        └── SIMD
```

---

# 1. Core Design Principles

* [ ] Rust-first
* [ ] Safe Rust wherever practical
* [ ] `unsafe` isolated and documented
* [ ] `no_std` capability for appropriate components
* [ ] deterministic behavior where practical
* [ ] zero allocation in real-time hot paths
* [ ] caller-owned buffers where practical
* [ ] bounded resource usage
* [ ] explicit numerical contracts
* [ ] fuzz-resistant parsing
* [ ] corruption-tolerant decoding where practical
* [ ] forward-compatible formats
* [ ] clean architectural boundaries
* [ ] reusable crates
* [ ] WASM-compatible core components
* [ ] stable FFI boundary
* [ ] reproducible benchmarks
* [ ] provenance tracking
* [ ] patent-conscious design

Avoid:

* [ ] unnecessary dependencies
* [ ] hidden heap allocation
* [ ] global mutable state
* [ ] codec-specific DSP duplicated across codecs
* [ ] application-specific logic in core crates
* [ ] undocumented numerical assumptions
* [ ] claims of absolute patent-free status

Preferred terminology:

> **Patent-conscious / royalty-free design objective**

rather than:

> 100% patent-free

Patent status is jurisdictional and must be independently reviewed.

---

# 2. Repository Structure

Target structure:

```text
tpt-media/
│
├── Cargo.toml
├── README.md
├── LICENSE
├── LICENSE-APACHE
├── CHANGELOG.md
├── CONTRIBUTING.md
├── SECURITY.md
├── ARCHITECTURE.md
├── TODO.md
│
├── docs/
│   ├── architecture.md
│   ├── codec-design.md
│   ├── container-design.md
│   ├── pipeline-design.md
│   ├── performance.md
│   ├── security.md
│   └── provenance.md
│
├── crates/
│   ├── tpt-media-core/
│   ├── tpt-media-bitstream/
│   ├── tpt-media-codec/
│   ├── tpt-media-container/
│   ├── tpt-media-pipeline/
│   ├── tpt-media-io/
│   └── tpt-media-probe/
│
├── codecs/
│   ├── pulse/
│   ├── aura/
│   ├── vox/
│   ├── livelink/
│   ├── vault/
│   └── whisper/
│
├── containers/
│   └── tpt-container/
│
├── apps/
│   ├── tpt-cli/
│   └── tpt-web/
│
├── ffi/
│   └── tpt-media-sys/
│
├── tests/
│   ├── integration/
│   ├── corpus/
│   ├── corruption/
│   └── interoperability/
│
├── fuzz/
│   ├── bitstream/
│   ├── container/
│   ├── codecs/
│   └── probe/
│
└── benches/
```

Do **not** create:

```text
tpt-media-math
tpt-media-signal
tpt-media-simd
```

Those responsibilities belong in `tpt-dsp`.

---

# 3. TPT-DSP Integration

## 3.1 Establish dependency boundary

* [x] Define `tpt-dsp` as the reusable signal-processing foundation — `tpt-media/ARCHITECTURE.md` Non-Goals: "Not a general-purpose DSP library. Transforms, filters, signal analysis and SIMD live in `tpt-dsp`."
* [x] Define `tpt-media` as the media representation/codec/container layer — `ARCHITECTURE.md` §1 Goals + §2 Crate Topology
* [x] Document dependency direction — `ARCHITECTURE.md` §3: `tpt-media ─────────► tpt-dsp (never reversed)`
* [x] Prevent `tpt-dsp` from depending on `tpt-media` — verified: no `tpt-media` reference anywhere in `tpt-dsp`'s `Cargo.toml` files
* [x] Prevent codec-specific APIs from leaking into `tpt-dsp` — `tpt-dsp-core`'s public API (`fft`, `dct_*`, `mdct`/`imdct`, `lpc::*`, `Biquad`, `RingBuffer`, ...) is entirely generic; no Pulse/Aura/Vox-specific types

Desired:

```text
tpt-media ───────► tpt-dsp
```

Never:

```text
tpt-dsp ───────► tpt-media
```

---

# 4. TPT-DSP Foundation Upgrade

This section is part of the master `tpt-media` plan.

Implementation may occur in the `tpt-dsp` repository.

## 4.1 Existing capabilities to retain

* [x] complex arithmetic
* [x] FFT
* [x] inverse FFT
* [x] DCT-II
* [x] DCT-III
* [x] DCT-IV
* [x] windows
* [x] filters
* [x] convolution
* [x] resampling
* [x] Hilbert transform
* [x] ring buffers
* [x] SPSC queues
* [x] SIMD abstraction
* [x] spectral analysis
* [x] audio I/O
* [x] WAV support

These should be extended rather than duplicated.

---

# 5. TPT-DSP — Transform Expansion

## 5.1 MDCT

Required by Pulse and Aura.

* [x] Define MDCT mathematical convention — `tpt-dsp-core/src/mdct.rs` doc: Princen–Bradley convention, explicit `X[k]` formula
* [x] Define IMDCT convention — explicit inverse formula in the same doc comment
* [x] Define normalization — `2/N` normalization documented and implemented in `imdct`
* [x] Define windowing convention — `sine_window`, documented Princen–Bradley condition `h[i]²+h[i+n]²=1`
* [x] Define overlap convention — documented: hop of `N` samples, overlap-add of windowed inverse blocks
* [x] Define supported block sizes — `MdctPlan::SUPPORTED_SIZES = [128, 256, 512, 1024, 2048, 4096]`, matching this section's target sizes exactly
* [x] Implement reference MDCT — `mdct()`
* [x] Implement reference IMDCT — `imdct()`
* [x] Add round-trip tests — `overlap_add_round_trip_reconstructs_signal` (TDAC round trip, tolerance `1e-6`)
* [x] Add numerical error tests — same test asserts sample-level error tolerance
* [x] Add edge-case tests — `silence_round_trips_to_silence`, `extreme_magnitude_input_stays_finite`, `nan_input_confines_to_taps_that_touch_it` (added 2026-09-07)
* [x] Add reusable `MdctPlan`
* [x] Avoid allocation during processing — `mdct`/`imdct`/`MdctPlan::forward`/`inverse` all take caller-owned output buffers, no internal `Vec`
* [x] Add optimized implementation — `FastMdctPlan` (added 2026-09-07): a single length-`2n` complex FFT via `FftPlan` plus O(N) chirp pre/post-multiplies, instead of the O(N²) direct sum. Verified against the direct reference (`fast_matches_reference_forward`/`_inverse`, `fast_plan_round_trip_matches_direct_overlap_add`)
* [x] Add SIMD implementation — `FastMdctPlan` is backed by `FftPlan` (RustFFT), which dispatches to AVX/SSE/NEON at runtime; no separate hand-rolled `core::simd` MDCT kernel was written since that would just reimplement what RustFFT already provides
* [x] Benchmark scalar vs optimized — `tpt-dsp-core/benches/dsp_bench.rs::bench_mdct`, all 6 target sizes, forward+inverse, direct vs `FastMdctPlan`; results in `tpt-dsp/BENCHMARKS.md` (~2000-15000× faster, widening with N as expected for O(N²) vs O(N log N))
* [x] Document numerical guarantees — module-level doc thoroughly documents convention, normalization, and the TDAC requirement; `FastMdctPlan`'s doc documents its own chirp/FFT derivation

Initial target sizes:

```text
128
256
512
1024
2048
4096
```

---

## 5.2 DCT optimization

Current DCT-IV implementation is suitable as a correctness reference but must be evaluated for codec workloads.

* [x] Benchmark current DCT implementation — `tpt-dsp-core/benches/dsp_bench.rs::bench_dct` (DCT-II/III/IV across `DCT_SIZES`), results recorded in `BENCHMARKS.md` (e.g. `dct/iv_f32/1024`: 50.0 µs, 20.5 Melem/s)
* [x] Define performance targets — **target (2026-09-07): the MDCT/DCT-IV transform (forward on encode, inverse on decode) must consume ≤10% of the codec's own real-time frame budget, `frame_size / sample_rate`, at its default operating point** — leaving ≥90% headroom for windowing, quantization, entropy coding, and system jitter. This is derived from Pulse's own committed real-time contract (§29, §1 "zero allocation in real-time hot paths" / deterministic execution), not an arbitrary number; it only binds codecs that claim real-time (Pulse — §30's Aura is explicitly "high-fidelity/archival", i.e. the Offline profile in §14, so this target doesn't apply to it).
  Measured against `tpt-dsp-core/benches/dsp_bench.rs::bench_mdct` (2026-09-07, `--quick`, noisy per `tpt-dsp/BENCHMARKS.md`'s own caveat):
  | Pulse `frame_size` (N) | frame budget (48kHz) | direct `mdct()` (what Pulse actually calls) | vs. 10% budget | `FastMdctPlan` |
  | ---: | ---: | ---: | :--- | ---: |
  | 128 | 2.667 ms | 214 µs | 8.0% — **passes**, thinly | 0.34 µs (0.01%) |
  | 256 | 5.333 ms | 822 µs | 15.4% — **fails** | 0.69 µs (0.01%) |
  So Pulse's *current* code (it calls the direct `mdct`/`imdct`, not the new `FastMdctPlan` — confirmed via `use tpt_dsp_core::{imdct, mdct, ...}` in `codecs/pulse/src/lib.rs`) already misses this target at its larger supported frame size, and only barely clears it at the smaller one, before accounting for anything else in the encode/decode path. `FastMdctPlan` clears the target by ~3 orders of magnitude at both sizes. **Follow-up (not yet done): wire `FastMdctPlan` into Pulse's actual `encode`/`decode`** so the codec meets its own real-time claim in practice, not just in the benchmark suite.
* [x] Implement optimized DCT-IV — `FastDctIvPlan` (added 2026-09-07): same FFT/chirp technique as `FastMdctPlan`, specialized to DCT-IV's `θ(m,k)=(π/N)(m+½)(k+½)` (zero-padded to `2N` since both indices range only `0..N`, unlike MDCT)
* [x] Preserve reference implementation for verification — the direct `dct_iv` stays; `FastDctIvPlan` is checked against it, not a replacement
* [x] Add scalar/SIMD equivalence tests — `fast_dct_iv_matches_reference` (direct `dct_iv` vs `FastDctIvPlan` across 4 sizes) and `fast_dct_iv_is_self_inverse_up_to_scale`; `FastDctIvPlan` is `FftPlan`/RustFFT-backed (same SIMD-dispatch rationale as `FastMdctPlan`)
* [x] Verify numerical error bounds — `dct_ii_of_constant_is_scaled`, `dct_ii_then_iii_roundtrips`, `dct_iv_orthogonality_roundtrip` all assert explicit tolerances; also fixed a real precision bug this uncovered — see Known Issues

---

## 5.3 2D transforms

Required primarily by video codecs.

_Confirmed 2026-09-07: no 2D transform code exists anywhere in `tpt-dsp-core` (no `dct_2d`, `Dct2d`, or separable-transform helpers) — expected, since this is only needed once Livelink/Vault/Whisper exist. All items below remain correctly unchecked._

* [ ] Define 2D DCT API
* [ ] Implement separable 2D DCT
* [ ] Implement inverse 2D DCT
* [ ] Support 4×4
* [ ] Support 8×8
* [ ] Support 16×16
* [ ] Benchmark
* [ ] SIMD optimize common block sizes
* [ ] Verify against 1D decomposition

---

# 6. TPT-DSP — LPC

Required by Vox and Aura.

* [x] Implement autocorrelation — `lpc.rs::autocorrelation`, tested
* [x] Define LPC coefficient convention — doc comment: `predicted[n] = Σ a[k]·x[n-1-k]`
* [x] Implement Levinson-Durbin — `levinson_durbin`, tested against an idealized pure-tone autocorrelation to floating-point precision
* [x] Define stability behavior — documented: a reflection coefficient reaching unity stops the recursion early, keeping only the stable prefix
* [x] Handle singular/degenerate inputs — `r[0] <= 0` (silence) returns zero early; `levinson_durbin_handles_silence` test
* [x] Implement LPC analysis — `LpcAnalyzer::analyze` (autocorrelation + Levinson-Durbin, reused scratch buffers)
* [x] Implement prediction — `predict()`
* [x] Implement residual generation — `residual()`
* [x] Implement synthesis — `synthesize()`, `residual_then_synthesize_is_identity` proves exact inversion
* [x] Add numerical tests — `levinson_durbin_predicts_pure_tone`, `predict_matches_residual_formula`, others
* [x] Add stability tests — added 2026-09-07: `levinson_durbin_stops_early_at_first_reflection` (order-1, `k=2.0`) and `levinson_durbin_stops_early_and_keeps_stable_prefix` (order-2, first reflection stable, second reaches `k=1.5`, proving the stable prefix is kept and only the aborted coefficient stays zero)
* [x] Add deterministic tests — `analyzer_reuses_buffers` asserts identical output across repeated calls
* [x] Add benchmarks — added 2026-09-07: `bench_lpc` in `tpt-dsp-core/benches/dsp_bench.rs` (`autocorrelation`+`levinson_durbin` at Aura's actual orders 8/16/32 against its default 1024-sample block; `predict`/`residual`/`synthesize` at order 16); results in `tpt-dsp/BENCHMARKS.md` — `autocorrelation` (O(N·order)) dominates over `levinson_durbin` (O(order²)) as expected

Potential API:

```text
LpcAnalyzer
LpcCoefficients
autocorrelation()
levinson_durbin()
predict()
residual()
synthesize()
```

---

# 7. TPT-DSP — Pitch Analysis

Required primarily by Vox and potentially useful elsewhere.

* [x] Define pitch-analysis API — `tpt-dsp-core/src/pitch.rs`: `PitchEstimate<F>{period_samples: Option<usize>, confidence, voiced}` + `frequency_hz()` helper
* [x] Implement autocorrelation pitch estimator — `autocorrelation_pitch`, normalized `Σx[n]x[n+lag]/Σx[n]²`, argmax over the search range. Biases the argmax toward the *shortest* near-tied lag (an absolute margin) to avoid the classic "octave error": a signal periodic at N is, by construction, also periodic at 2N/3N/..., and a naive global argmax can pick a harmonic instead of the fundamental on floating-point noise alone — this was caught by the test below actually returning the wrong (doubled) period before the fix
* [x] Implement AMDF estimator — `amdf_pitch`, `mean(|x[n]-x[n+lag]|)`, argmin over the search range. Same octave-error concern and fix, but scaled to the AMDF range observed so far rather than a fixed margin, since AMDF's "good" values cluster near zero rather than near a bounded ceiling
* [x] Define pitch period range — `hz_range_to_period_samples(sample_rate, min_f0_hz, max_f0_hz)` + `SPEECH_MIN_F0_HZ`/`SPEECH_MAX_F0_HZ` (50-500Hz) constants
* [x] Define voiced/unvoiced detection — threshold on `confidence` against a caller-supplied `voiced_threshold` (`DEFAULT_VOICED_THRESHOLD = 0.3`, literature-typical); `period_samples` is `None` when unvoiced so callers can't accidentally use a meaningless lag
* [x] Add confidence metric — autocorrelation: normalized peak value; AMDF: `1 - min/max` of the AMDF values seen across the search range
* [x] Add tests using synthetic signals — `autocorrelation_pitch_recovers_known_period`/`amdf_pitch_recovers_known_period` (known-frequency sine), `silence_is_unvoiced_for_both_estimators`, `hash_noise_is_unvoiced_for_both_estimators` (SplitMix64 PRNG — a simpler per-index hash was tried first and rejected because it still had short-range structure `autocorrelation_pitch` could lock onto, a real false-positive risk worth documenting)
* [ ] Add speech corpus tests — no real speech corpus is available in this environment; only synthetic signals are tested. Left honestly unchecked rather than substituting a synthetic proxy for what this item actually asks
* [x] Benchmark — `tpt-dsp-core/benches/dsp_bench.rs::bench_pitch`, both estimators, at a Vox-realistic 16kHz/40ms frame over the full speech F0 range; ~53µs each (0.13% of the frame budget) — see `BENCHMARKS.md`

---

# 8. TPT-DSP — Vector Quantisation

Required by Vox and Whisper.

* [x] Define generic VQ API — `tpt-dsp-core/src/vq.rs`: flat-codebook convention (`entries*dim`), `DistanceMetric` enum, free functions + `VqCodebook` owning wrapper (`alloc`-gated)
* [x] Implement nearest-vector search — `nearest_vector`, linear scan, deterministic lowest-index tie-break
* [x] Implement distance metrics — `DistanceMetric::{SquaredEuclidean, Manhattan}`; `metrics_can_disagree_on_ranking` test proves they're genuinely distinct, not accidentally identical
* [x] Implement fixed-size codebooks — flat `&[F]` convention (`codebook.len() == entries*dim`) for the free functions; `VqCodebook` for owned storage
* [x] Implement encoder — `vq_encode`
* [x] Implement decoder — `vq_decode`
* [x] Support caller-owned buffers — all free functions take caller-provided output slices, no internal allocation
* [x] Add deterministic behavior — `ties_break_toward_lowest_index_and_are_deterministic` (exact-tie input, repeated calls agree)
* [x] Add optional accelerated search — `nearest_vector_accelerated`: early-exit once a codeword's partial distance sum can't beat the current best (both metrics accumulate non-negative terms, so this is always safe). Proven bit-identical to the linear scan (`accelerated_matches_linear_search`, 2 metrics × 20 random queries against a 40-entry codebook), not just "usually agrees" — ~3-3.6× faster at Vox's/Whisper's target codebook sizes (data-dependent, see `BENCHMARKS.md`)
* [x] Benchmark linear search — `tpt-dsp-core/benches/dsp_bench.rs::bench_vq`, linear vs accelerated, at (1024,10) and (4096,16) — Vox's/Whisper's own target codebook sizes (§31/§34); results in `BENCHMARKS.md`
* [ ] Investigate tree/product quantisation for larger codebooks — intentionally not implemented: no concrete consumer (Whisper doesn't exist yet) to validate requirements against, same reasoning as holding off on §5.3's 2D transforms

VQ itself belongs in DSP.

The codec-specific codebook belongs in the codec.

---

# 9. TPT-DSP — LSP / LSF

Useful for speech coding.

* [x] Define LSP representation — `tpt-dsp-core/src/lsp.rs`: `p` strictly-increasing frequencies in radians `(0,π)`, alternating between the P/Q root families
* [x] Define LSF representation — `lsp_rad_to_lsf_hz`/`lsf_hz_to_lsp_rad`: the same frequencies in Hz (`rad * sample_rate/2π`)
* [x] LPC → LSP conversion — `lpc_to_lsp`: builds the symmetric/antisymmetric `P(z)`/`Q(z)` polynomials and finds their interlacing roots via a direct evaluate-and-bisect grid search (not the classical Chebyshev-recursion formulation — chosen because it needs no extra scratch/coefficient transform and is directly testable against `lsp_to_lpc`)
* [x] LSP → LPC conversion — `lsp_to_lpc`: rebuilds `P(z)`/`Q(z)` from the interlacing roots (`A = (P+Q)/2`); round-trips exactly against `lpc_to_lsp` at LPC orders 2, 4, 10, 20, and 32 (Aura's max)
* [x] Stability validation — `lsp_is_stable`: strictly increasing, all in `(0,π)`, even length — exactly the condition under which `lsp_to_lpc` reconstructs a stable filter
* [x] Interpolation — `lsp_interpolate`; test proves the practical reason LSP exists (a convex combination of two stable LSP sets is itself stable, unlike interpolating raw LPC coefficients)
* [x] Quantisation support — `lsp_quantize_uniform`: uniform per-coefficient quantization with an ordering safeguard (nudges a colliding value past its predecessor) — naive independent quantization can otherwise collapse two close LSPs to the same level and silently produce an unstable filter
* [x] Tests — round trip (5 orders), stability-rejection, interpolation-preserves-stability, quantization-preserves-ordering-under-collision, LSF conversion

---

# 10. TPT-DSP — Psychoacoustics

Required primarily by Aura.

## 10.1 Frequency scales

* [x] Bark scale — `tpt-dsp-core/src/bark.rs::hz_to_bark`, Traunmüller (1990) formula with low/high-frequency corrections; matches known reference points (1kHz≈8.5 Bark, 5kHz≈19 Bark)
* [x] critical-band mapping — `fft_bin_bark_bands`: maps every FFT bin to its Bark band index for a given sample rate/FFT size
* [x] ERB utilities if useful — `erb_bandwidth`/`hz_to_erb_rate`/`erb_rate_to_hz` (Glasberg & Moore, 1990); the ERB-rate formula's `log10` is directly (exactly) invertible, unlike Bark's corrected formula which needs `bark_to_hz`'s bisection
* [x] frequency ↔ Bark conversion — `hz_to_bark`/`bark_to_hz` (the latter via bisection, since the corrected formula has no closed-form inverse)
* [x] band aggregation — `aggregate_bands`: sums per-bin spectrum values into their assigned Bark band

## 10.2 Psychoacoustic primitives

* [x] threshold-in-quiet model — `tpt-dsp-core/src/psychoacoustic.rs::threshold_in_quiet`, Terhardt (1979) approximation; test confirms the ear is most sensitive around 3kHz vs. 100Hz/15kHz
* [x] tonal/noise classification — `is_tonal_peak`/`classify_tonal_bins`: a local-peak-with-prominence test (simplified vs. MPEG psychoacoustic model 1's frequency-region-specific neighbor windows, documented as such)
* [x] spectral spreading — `spreading_function_db`, Schroeder (1979); peaks exactly at `dz=0` (verified via the formula's own calculus, not just assumed) and decays in both directions
* [x] simultaneous masking — `simultaneous_masking`: spreads every masker band's energy across the Bark axis with the standard tonal (`14.5+z` dB) or noise (`5.5` dB constant) offset
* [x] masking threshold — `combined_threshold_db`: max of the spread masking excitation and the threshold in quiet
* [x] perceptual weighting — `perceptual_weight_db`: signal-to-mask ratio (SMR) in dB
* [x] model validation — integration test: a single loud tonal masker at 1kHz raises the combined threshold >10dB above quiet near itself, and the threshold is higher near the masker than several critical bands away
* [x] benchmark — `tpt-dsp-core/benches/dsp_bench.rs::bench_psychoacoustic`: `simultaneous_masking` across all 25 audible Bark bands (6.72µs), `classify_tonal_bins` over a 1024-bin spectrum (594ns); results in `BENCHMARKS.md`

These should be generic models, not Aura-specific implementations.

---

# 11. TPT-DSP — Noise Shaping

* [x] Define noise-shaping filter API — `tpt-dsp-core/src/noise_shaping.rs`: `noise_shape_step(input, feedback, error_history, quantize)`, error subtracted from input (not added — see below), caller-owned history
* [x] Implement generic noise-shaping filter — `noise_shape_step` + `NoiseShaper` (`alloc`-gated owning wrapper); generic over any caller-supplied `quantize` closure, not tied to a specific quantization scheme
* [x] Support configurable orders — order is just `feedback.len()` (`error_history` sized to match); tested at order 1 and 2
* [x] Validate stability — `noise_shaper_is_stable`: round-to-nearest is self-limiting for *any* finite feedback gain (proven, not just tested, up to gain 1000), so the check has real content only for a `quantize` closure that isn't self-limiting — verified it correctly distinguishes a converging vs. diverging linear recursion for a deliberately non-self-limiting test quantizer
* [x] Add tests — DC-tracking (shaped average recovers a non-quantizable constant almost exactly; plain quantization doesn't), bounded-cumulative-error (vs. plain quantization's linear growth), hand-computed history shift/update, both stability-check scenarios, wrapper-matches-free-function
* [x] Add benchmark — `tpt-dsp-core/benches/dsp_bench.rs::bench_noise_shaping`, orders 1 and 16 (~4.9ns / ~16.5ns, scales linearly with order as expected); results in `BENCHMARKS.md`

---

# 12. TPT-DSP — Numerical Contracts

**Audited 2026-09-07** against the actual code in `tpt-dsp-core/src/*.rs`,
now that all 8 required algorithm families genuinely exist (they didn't
when this section was last touched — see the Known Issues note above).
This section's 9 items describe a *formal, uniform contract* across all
8 families; the honest finding is that no single item is uniformly true
across all 8, so each stays unchecked below with what's actually there:

* [ ] Define input domain — asserted (and panics documented) in every
  module this session touched (`mdct`/`dct`/`lpc`/`vq`/`psychoacoustic`/
  `bark`/`lsp`/`pitch`/`noise_shaping`, 6-8 `# Panics` sections each) but
  `fft.rs` (pre-existing, untouched this session) asserts valid sizes
  without a matching `# Panics` doc section — not a uniform contract yet
* [ ] Define output domain — rarely stated explicitly anywhere (mostly
  inferred from the input domain + the operation, e.g. "N inputs → N
  outputs"); no module states an output *range* guarantee (e.g. "always
  finite", "always in `[-1,1]`")
* [ ] Define normalization — well documented for MDCT/DCT/LPC/VQ (this
  session's convention docs spell out exact formulas); FFT's
  forward/inverse normalization convention is only demonstrated via
  tests (`inverse_roundtrip` dividing by `len`), not stated in prose
* [ ] Define precision — no module states a formal precision contract
  (e.g. "expect ~1e-6 relative error in f32"); all generic `F: Float`
  code is silent on this, precision only shows up ad hoc as test
  tolerances
* [ ] Define acceptable error — same gap as precision: tolerances exist
  per-test (`1e-4`, `1e-6`, `1e-9`, ...) but there's no single documented
  policy for what tolerance a given algorithm/precision should target
* [x] Define valid sizes — genuinely uniform: every module (including
  `fft.rs`) asserts its size constraints (power-of-two, even order,
  matching lengths, etc.) and panics rather than producing garbage
* [x] Define failure behavior — uniform: every algorithm here panics on
  invalid input (no silent wrong answers); `lpc_to_lsp` is the one
  exception with a real non-panic failure mode (`bool` return when the
  input isn't stable enough to find all roots), and that's documented
* [ ] Define determinism expectations — implicit everywhere (pure
  functions, no RNG, so trivially deterministic) but never *stated*;
  §39/§40 elsewhere in this document cover determinism/SIMD-equivalence
  at the workspace level, which is arguably the right place for this,
  making a per-algorithm restatement here low-value
* [x] Add reference tests — genuinely uniform and the strongest item:
  every one of the 8 families below has a real reference/round-trip/
  equivalence test with an explicit tolerance (FFT vs. naive DFT,
  MDCT/DCT-IV fast-vs-direct, LPC round-trip through LSP, VQ
  accelerated-vs-linear, psychoacoustic model validation, filters/
  resampling have their own pre-existing tests)

Required for:

* [ ] FFT — valid sizes/failure behavior/reference tests all present; no formal `# Panics` docs, no precision/normalization prose
* [ ] DCT — same profile as FFT, plus the precision fix from §5.2 (angle-reduction) is documented in code but not as a formal "precision" contract
* [ ] MDCT — the best-documented family in the crate (explicit convention, normalization, panics, numerical-guarantees paragraph, fast-vs-direct equivalence tests) but still lacks a stated precision/acceptable-error number
* [ ] LPC — convention, stability behavior, and panics are documented; precision/acceptable-error still implicit-only
* [ ] filters — pre-existing, only partially audited this session; has some `# Panics` sections but not comprehensive
* [ ] resampling — pre-existing; has more `# Panics` sections than filters, but same precision/acceptable-error gap
* [ ] VQ — convention, determinism, and panics documented; same precision/acceptable-error gap
* [ ] psychoacoustic calculations — convention and panics documented (this is a modeling toolkit, not a pure-math transform, so "precision"/"acceptable error" apply less cleanly here than elsewhere)

---

# 13. TPT-DSP — SIMD

* [x] Retain SIMD in `tpt-dsp` — `tpt-dsp-core/src/simd.rs` (`core::simd`-based complex mul/add/magnitude, FFT butterfly)
* [x] Remove planned `tpt-media-simd` — never created; no `tpt-media-simd` directory or crate exists
* [x] Establish scalar reference implementations — `simd_scalar.rs`, the default/fallback path
* [x] Establish SIMD equivalence tests — `simd_mul_matches_scalar`, `simd_add_matches_scalar`, `simd_magnitude_matches_scalar`, `simd_fft_matches_scalar_fft`
* [x] Benchmark SIMD kernels — `dsp_bench.rs` `bench_simd_helpers` group
* [x] Document supported architectures — added `tpt-dsp` `ARCHITECTURE.md` §10 with an explicit x86/x86_64 (SSE2 baseline), ARM/AArch64 (NEON baseline), wasm32 and future-RISC-V support matrix, plus matching notes in `simd.rs`'s and `lib.rs`'s doc comments; also fixed a stale `ARCHITECTURE.md` line that still called the vectorised path "not started"
* [x] Ensure runtime feature detection where appropriate — judged not appropriate and documented why in the same §10: `tpt-dsp-core` is `#![forbid(unsafe_code)]`, and the standard `is_x86_feature_detected!`/target-feature-multiversioning pattern requires an unsafe call site; separately, `core::simd` has no runtime-dispatch mode to begin with — wider ISAs (AVX2/AVX-512/SVE) are a build-time choice (`-C target-cpu`/`target-feature`) for whoever compiles the final binary, not something the library selects. The existing `build.rs` check remains a toolchain (nightly-vs-stable) probe, not a CPU-feature probe — noted explicitly as such
* [x] Verify fallback behavior — `build.rs` only enables `tpt_portable_simd` on nightly; stable builds provably use the scalar path (documented in `dsp_bench.rs`'s own comment: "which is scalar by default")

Potential targets:

```text
scalar
portable SIMD
x86/x86_64
ARM/AArch64
future RISC-V
```

---

# 14. TPT-DSP — Real-Time Contracts

Define explicit execution profiles.

## Real-time

* [ ] no allocation — `tpt-dsp/ARCHITECTURE.md` §3.2/§11.2 documents real caveats (async adapters allocate a `Vec` per frame; `IirFilter`/`Eq` reallocate scratch the first time a block exceeds every prior size seen), so this is true in steady state for most paths but not unconditionally
* [ ] no blocking — `ARCHITECTURE.md` §11.2 documents two real exceptions: `SpscQueue`/`Producer`/`Consumer` deliberately also expose blocking `send`/`recv` alongside the real-time-safe `try_send`/`try_recv`, and `tpt-dsp-analysis::async_adapters` blocks on an async runtime (`block_on`/`.await`) by design as a streaming/offline adapter
* [ ] no I/O — `ARCHITECTURE.md` §11.2 confirms this holds unconditionally for `tpt-dsp-core`/`-audio`/`-analysis`/`-control` (no file/socket/serial/audio-device access in any of the four), but not for the workspace as a whole: `tpt-dsp-io` is entirely I/O by design (`audio::run_output`/`run_input`, `serial::SerialReader`, `tcp::serve_iq`) — those are driver/setup functions that call into a real-time-safe processing closure, not processing functions themselves
* [ ] no unbounded work — `tpt-dsp/ARCHITECTURE.md` §3.2/§11.2 documents that `OutlierDetector::push` is O(n log n) per sample, not O(1); also names `tpt-dsp-io::IqStream::feed`'s unbounded buffer growth (if `drain` is never called) as the other exception in the workspace
* [x] caller-owned memory — the dominant idiom across both `tpt-dsp` API patterns (free functions on slices; structs with pre-allocated scratch), per `ARCHITECTURE.md` §3.1/§11.2, with no known exception anywhere in the workspace
* [x] deterministic execution bounds where possible — `ARCHITECTURE.md` §11.2 confirms every loop in `tpt-dsp-core`/`-audio`/`-analysis`/`-control` terminates within a bound computable from the input size or a fixed constant (FFT/DCT bounded by `n`, `lsp.rs`'s root search bounded by a fixed `grid_points` plus an unconditional 40-iteration bisection, `simd.rs`'s vectorised loops by `len`); no polling loop, `while true`, or unbounded recursion exists — this is a worst-case-boundedness criterion, so it holds even where the stricter "no unbounded work" criterion above does not (e.g. `OutlierDetector`'s data-independent O(n log n) sort)

## Offline

* [x] dynamic allocation permitted — defined as an explicit Offline-profile permission in `ARCHITECTURE.md` §11.1
* [x] larger scratch buffers permitted — defined as an explicit Offline-profile permission in `ARCHITECTURE.md` §11.1
* [x] parallel processing permitted — defined as an explicit Offline-profile permission in `ARCHITECTURE.md` §11.1 (not yet exercised by any current API, but explicitly allowed)
* [x] expensive RDO permitted — defined as an explicit Offline-profile permission in `ARCHITECTURE.md` §11.1 (not yet exercised by any current API, but explicitly allowed)

Document which DSP APIs satisfy which profile. — satisfied: `tpt-dsp/ARCHITECTURE.md` §11 defines both profiles explicitly (§11.1), states where each Real-time criterion currently holds or doesn't with the specific exception (§11.2), and classifies every public API across all five crates plus `tpt-dsp-viz` into Real-time or Offline in a per-crate table (§11.3), using this section's own Real-time/Offline framing directly.

---

# 15. TPT-MEDIA-CORE

Define fundamental media data types.

* [x] `Timestamp`
* [x] `Duration`
* [x] `Timebase`
* [x] `Packet`
* [x] `Frame`
* [x] `AudioFrame`
* [x] `VideoFrame`
* [x] `Track`
* [x] `Stream`
* [x] `CodecId`
* [x] `CodecConfig`
* [x] `PixelFormat`
* [x] `SampleFormat`
* [x] `ChannelLayout`
* [x] `MediaType`
* [x] `Metadata`

Support:

* [x] PTS
* [x] DTS
* [x] duration
* [x] keyframe/discontinuity flags
* [x] side data
* [x] codec-private data
* [x] per-track timebases

---

# 16. Timestamp Architecture

* [x] Define rational timebase
* [x] Avoid floating-point timestamps
* [x] Define timestamp conversion
* [x] Define overflow behavior — `rescale()` corrected 2026-09-07 (see Known Issues); `i128` intermediates with explicit `checked_mul` on every step, returns `None` on any overflow, identity-rescaling short-circuited. `rescale_overflow_returns_none` replaced with one whose inputs actually overflow `i64` (1 Hz → 90 kHz, 90 000× expansion); a sanity test (`1 s → 90 kHz == 90 000`) guards against the previous factor-swap bug
* [x] Define negative timestamp handling — `Add`/`Sub` (Timestamp + Duration, Timestamp − Duration) now **saturate** to `i64::MAX`/`i64::MIN` in the direction of the signed Duration, instead of silently returning the unchanged left-hand side via `unwrap_or(self.ticks)`. New fallible variants `Timestamp::checked_add_duration` / `checked_sub_duration` return `Option<Timestamp>` for callers that genuinely need to detect overflow. Saturation direction is signed-duration-aware, not left-hand-side-aware (a positive duration saturates upward; a negative duration saturates downward), so `0 - Duration{-1} == +1` and `i64::MIN - Duration{+1} == i64::MIN`. Tests: `timestamp_add_duration_saturates_on_overflow`, `timestamp_sub_duration_saturates_on_overflow`, `timestamp_checked_{add,sub}_duration_propagates_none`, `timestamp_add_sub_negative_duration_round_trips`
* [x] Define discontinuities — `PacketFlags::DISCONTINUITY` was already present in `tpt-media-core` but undocumented and not round-tripped through the v1 `.tpt` wire format (the "second flags bit is actually `END_OF_FRAME`, not discontinuity" finding in Known Issues). Now: full semantic doc comment on the flag, `Packet::is_discontinuity()` / `is_end_of_frame()` accessors added, `tpt-container` muxer and demuxer both round-trip bit 0x04. Test: `discontinuity_flag_round_trips_through_container`, plus `discontinuity_flag_accessor`, `end_of_frame_flag_accessor`, `packet_flags_are_independent`
* [x] Define clock domains — new `tpt-media-core::sync::ClockDomain` (named `(String, Timebase)` pair; equality is by *name only*, deliberately, so two accidentally-similar timebases in different domains can't be silently equated). See `crates/tpt-media-core/src/sync.rs`
* [x] Define synchronization model — `tpt-media-core::sync::{SyncPoint, SyncMap, SyncError}`. `SyncPoint` pins one moment in one `ClockDomain` to one moment in another; `SyncMap` collects pins and translates timestamps between domains by **linear interpolation** between adjacent pins, deliberately **rejecting extrapolation** (returns `SyncError::OutOfRange`) to prevent A/V drift bugs; `f64` arithmetic is confined to the API boundary (seconds), with `i64` ticks everywhere else. `DomainMismatch` covers wrong-timebase inputs. Test coverage: 12 sync tests including exact-pin-match, two-pin interpolation, three-pin local interpolation, before/after-pin rejection, wrong-timebase rejection, zero-length-interval (no divide-by-zero), negative-time, and wallclock→video
* [x] Add audio/video sync tests — `audio_video_sync_end_to_end` exercises a realistic 48 kHz-audio × 90 kHz-video scenario with two pins (t=0 and t=60 s), covering exact-pin match, interpolated values, and out-of-range rejection
* [x] Add long-duration timestamp tests

---

# 17. Bitstream Layer

Create `tpt-media-bitstream`.

## Bit reader

* [x] arbitrary bit-width reads
* [x] bounded reads
* [x] alignment
* [x] peek
* [x] skip
* [x] EOF handling
* [x] malformed-input handling

## Bit writer

* [x] arbitrary bit-width writes
* [x] alignment
* [x] flush
* [x] bounded capacity
* [x] deterministic output

## Compression primitives

* [x] Huffman
* [x] Rice coding
* [x] range coder — re-verified 2026-09-07: all 4 range coder tests pass consistently (5 runs); see Known Issues' correction to the earlier "broken" finding
* [ ] entropy utilities
* [x] canonical representations
* [x] malformed-input handling

Do not move these into `tpt-dsp`.

---

# 18. Codec Framework

Create `tpt-media-codec`.

Define common interfaces:

```text
Codec
Encoder
Decoder
CodecConfig
Packet
Frame
```

* [x] define codec discovery
* [x] define encoder lifecycle
* [x] define decoder lifecycle
* [x] define flush semantics
* [ ] define drain semantics — only `flush()` exists; no distinct drain concept
* [x] define error model
* [x] define configuration model
* [x] define capability discovery
* [x] define hardware/SIMD capability exposure
* [x] define real-time capability
* [ ] define supported formats

---

# 19. Resource Limits

All decoders must support bounded resources.

* [ ] maximum frame dimensions — `check_frame_bytes` exists but is never called by a real codec
* [x] maximum packet size
* [ ] maximum metadata size
* [ ] maximum allocation — `check_allocation` exists but is never called by a real codec
* [ ] maximum reference frames — `check_reference_frames` exists but is never called by a real codec
* [ ] maximum recursion depth — `check_recursion_depth` exists but is never called by a real codec
* [ ] maximum decode work where practical — `max_decode_micros` exists but is never called by a real codec
* [ ] cancellation support — `CodecError::Cancelled` exists but nothing produces or checks a cancellation token
* [x] malformed-input termination guarantees

---

# 20. TPT Container

Create the native `.tpt` media container.

## 20.1 File header

Initial structure:

```text
magic:       TPTM
version
header_size
global_timebase
```

* [x] finalize binary representation
* [x] define endianness
* [ ] define versioning — `version` field exists but `from_bytes` never checks it
* [ ] define reserved fields
* [ ] define compatibility rules

---

# 21. Container Track Structure

Track metadata must include:

* [x] track ID
* [ ] media type — `index_trak` hardcodes `MediaType::Audio` for every track regardless of chunk contents
* [x] codec ID
* [ ] codec version — not written as a separate field, only inside the opaque codec-private blob
* [x] codec-private data
* [ ] timebase — not serialized by TRAK; would have to be smuggled into codec-private data
* [ ] sample rate where applicable
* [ ] channel layout where applicable
* [ ] dimensions where applicable
* [ ] pixel format where applicable
* [ ] metadata

---

# 22. Container Cluster Structure

Clusters should support:

* [x] timestamp
* [ ] duration — not present in the CLUS layout
* [x] track ID
* [ ] PTS — one timestamp field stands in for both PTS and DTS; not distinctly modeled
* [ ] DTS — `next_packet` sets `pkt.dts = pkt.pts` unconditionally, not an independently stored value
* [x] keyframe flag
* [ ] discontinuity flag — the second flags bit is actually `END_OF_FRAME`, not discontinuity
* [x] payload
* [ ] optional checksum

---

# 23. Container Index

Do not require scanning the entire file for normal seeking.

* [x] define index structure
* [x] index keyframes
* [x] index cluster offsets
* [ ] support per-track indexes — code's own comment says this is reserved, not yet consumed
* [x] support time-based lookup
* [ ] support offset-based lookup
* [ ] define index versioning
* [ ] define recovery behavior

---

# 24. Container Robustness

* [x] corruption detection
* [x] truncated-file handling
* [ ] incomplete final cluster handling — only incidentally covered by the generic truncation path, no dedicated logic/test
* [x] unknown chunk skipping
* [ ] forward-compatible chunk types — only incidentally covered by the generic unknown-chunk path, no dedicated logic/test
* [ ] checksum support
* [ ] append mode
* [ ] live-stream mode
* [ ] recovery/index rebuilding
* [ ] fuzz testing — no fuzz harness exists anywhere in the repo

---

# 25. Container Streaming

Support files that are not yet complete.

* [ ] fragmented/clustered writing
* [ ] incremental parsing
* [ ] live append
* [ ] missing final index
* [ ] streaming playback
* [ ] graceful termination
* [ ] optional footer/index reconstruction

---

# 26. Media Pipeline

Create `tpt-media-pipeline`.

Architecture:

```text
Source
  ↓
Demux
  ↓
Decoder
  ↓
Filter / Transform
  ↓
Encoder
  ↓
Mux
  ↓
Sink
```

* [ ] define node abstraction
* [ ] define stream negotiation
* [ ] define format negotiation
* [ ] define backpressure
* [ ] define buffering
* [ ] define cancellation
* [ ] define error propagation
* [ ] define flushing
* [ ] define end-of-stream
* [ ] define timestamps through pipeline
* [ ] define threading model
* [ ] define realtime scheduling profile

---

# 27. Media I/O

Create `tpt-media-io`.

Reuse `tpt-dsp-io` rather than duplicating functionality.

* [ ] define PCM integration
* [ ] integrate WAV
* [ ] integrate raw audio
* [ ] define image/video frame I/O
* [ ] define streaming sources
* [ ] define streaming sinks
* [ ] define async integration where appropriate

---

# 28. Native Codec Family

All codecs must implement the generic codec framework.

---

# 29. Pulse

**Purpose:** low-latency real-time audio.

Target:

* low latency
* low complexity
* robust packet loss behavior
* real-time safe operation

## DSP

* [x] MDCT via `tpt-dsp`
* [x] windowing via `tpt-dsp`
* [x] quantisation
* [ ] optional perceptual weighting

## Codec

* [x] define frame sizes
* [x] define bitstream
* [ ] define modes — Pulse has no distinct "modes" concept
* [x] define quantisation
* [x] implement encoder
* [x] implement decoder
* [x] implement PLC
* [ ] test packet loss — the PLC test only checks `conceal()` returns a frame of the right sample count, not an actual drop-and-conceal stream scenario
* [ ] test jitter
* [ ] benchmark latency
* [ ] benchmark CPU usage

Initial frame targets:

```text
128
256 samples
```

---

# 30. Aura

**Purpose:** high-fidelity / archival audio.

Modes:

```text
lossless
lossy
```

## Lossless

* [x] LPC
* [x] residual
* [ ] Rice coding — actually implements Exp-Golomb (`write_exp_golomb`/`read_exp_golomb`); doc comment corrected 2026-09-07 to match the wire format (see Known Issues + `CHANGELOG.md` `[Unreleased] / Fixed`). Switching the wire format to actual Rice coding is still pending and would be a breaking change to the v0.1 packet format
* [x] block structure
* [ ] checksum/integrity
* [x] bit-exact round trip

## Lossy

* [x] MDCT
* [ ] psychoacoustic model
* [ ] Bark bands
* [ ] masking
* [x] quantisation
* [ ] noise shaping
* [ ] stereo model if required
* [x] encoder
* [x] decoder

Use `f64` only where justified by measurable quality requirements.

---

# 31. Vox

**Purpose:** extremely low-bandwidth speech.

Initial target:

```text
8 kHz
16 kHz
mono
```

* [ ] LPC analysis
* [ ] pitch estimation
* [ ] voiced/unvoiced classification
* [ ] excitation model
* [ ] VQ
* [ ] LSP/LSF if beneficial
* [ ] fixed-point implementation
* [ ] `no_std` support
* [ ] encoder
* [ ] decoder
* [ ] packet-loss concealment
* [ ] deterministic decoding
* [ ] embedded benchmark

Initial codebook target:

```text
1024 entries
```

---

# 32. Livelink

**Purpose:** low-latency video communication.

Design:

```text
I/P coding
intra refresh
simple prediction
low-complexity transforms
```

* [ ] define pixel formats
* [ ] define colour model
* [ ] define frame structure
* [ ] define I-frame
* [ ] define P-frame
* [ ] implement intra prediction
* [ ] implement inter prediction
* [ ] motion estimation
* [ ] full-pixel SAD
* [ ] DCT
* [ ] quantisation
* [ ] entropy coding
* [ ] intra refresh
* [ ] deblocking
* [ ] decoder
* [ ] encoder
* [ ] latency benchmark

Initial intra-refresh target:

```text
~5% of frame per frame
```

---

# 33. Vault

**Purpose:** high-compression video.

* [ ] define superblock structure
* [ ] 64×64 top-level blocks
* [ ] recursive partitioning
* [ ] minimum 8×8 blocks
* [ ] motion vectors
* [ ] full-pixel search
* [ ] half-pixel interpolation
* [ ] reference frames
* [ ] 2D transforms
* [ ] quantisation
* [ ] RDO
* [ ] entropy coding
* [ ] loop filtering
* [ ] decoder
* [ ] encoder

Initial target:

```text
up to 16 reference frames
```

RDO must use an explicit objective such as:

```text
J = D + λR
```

where:

* `D` = distortion
* `R` = bitrate
* `λ` = rate/distortion tradeoff

Do not claim an absolute global minimum.

---

# 34. Whisper

**Purpose:** extreme edge / IoT video.

* [ ] define supported dimensions
* [ ] define patch representation
* [ ] define dictionary
* [ ] implement VQ
* [ ] implement encoder
* [ ] implement decoder
* [ ] define reconstruction model
* [ ] investigate temporal prediction
* [ ] define quality metric
* [ ] benchmark bandwidth
* [ ] benchmark CPU
* [ ] benchmark memory
* [ ] test extremely low bitrate

Initial target:

```text
≤480p
4096-entry dictionary
12-bit codebook index
```

The bandwidth target must be benchmark-defined rather than treated as a universal guarantee.

---

# 35. Codec Interoperability

Native TPT codecs need:

* [ ] versioned bitstreams
* [ ] codec identification
* [ ] configuration serialization
* [ ] decoder capability negotiation
* [ ] graceful unsupported-mode handling
* [ ] forward-compatible extension mechanism

---

# 36. Codec Quality Testing

Create reference corpora.

## Audio

* [ ] speech
* [ ] music
* [ ] transient-heavy material
* [ ] synthetic signals
* [ ] silence
* [ ] pathological inputs

## Video

* [ ] talking head
* [ ] camera motion
* [ ] static scenes
* [ ] high-motion scenes
* [ ] animation
* [ ] screen content
* [ ] synthetic patterns
* [ ] pathological frames

---

# 37. Objective Quality Metrics

Audio:

* [ ] SNR
* [ ] spectral error
* [ ] perceptual metrics where practical
* [ ] bitrate/quality curves
* [ ] CPU/quality curves

Video:

* [ ] PSNR
* [ ] SSIM
* [ ] bitrate
* [ ] frame latency
* [ ] encode CPU
* [ ] decode CPU
* [ ] memory

---

# 38. Testing

## Unit tests

* [ ] every public primitive
* [ ] edge cases
* [ ] malformed input
* [ ] overflow
* [ ] empty input
* [ ] maximum values

## Property tests

* [ ] encode/decode invariants
* [ ] timestamp invariants
* [ ] bitstream invariants
* [ ] container invariants
* [ ] transform properties

## Fuzzing

* [ ] bitstream fuzzing
* [ ] container fuzzing
* [ ] codec fuzzing
* [ ] probe fuzzing
* [ ] malformed metadata
* [ ] truncated packets
* [ ] random packet ordering

---

# 39. Determinism

* [ ] deterministic decoding
* [ ] deterministic container generation where practical
* [ ] deterministic scalar DSP
* [ ] document SIMD differences
* [ ] deterministic test corpus
* [ ] reproducible benchmark configuration

---

# 40. Scalar / SIMD Equivalence

For all optimized DSP kernels:

* [ ] scalar reference
* [ ] SIMD implementation
* [ ] same-input comparison
* [ ] tolerance definition
* [ ] randomized testing
* [ ] architecture-specific CI

---

# 41. Performance

Benchmark:

* [ ] MDCT
* [ ] IMDCT
* [ ] FFT
* [ ] DCT
* [ ] LPC
* [ ] VQ
* [ ] psychoacoustics
* [ ] entropy coding
* [ ] Pulse encode/decode
* [ ] Aura encode/decode
* [ ] Vox encode/decode
* [ ] Livelink encode/decode
* [ ] Vault encode/decode
* [ ] Whisper encode/decode
* [ ] container mux/demux
* [ ] pipeline throughput

Record:

```text
throughput
latency
CPU
memory
allocations
```

---

# 42. Memory Testing

* [ ] allocation profiling
* [ ] real-time allocation assertions
* [ ] peak memory measurements
* [ ] hostile-input memory tests
* [ ] maximum-frame tests
* [ ] maximum-packet tests

---

# 43. CLI

Create `tpt-cli`.

Commands:

```text
tpt probe
tpt encode
tpt decode
tpt transcode
tpt inspect
tpt benchmark
```

* [ ] codec selection
* [ ] container selection
* [ ] quality configuration
* [ ] bitrate configuration
* [ ] metadata manipulation
* [ ] streaming stdin/stdout
* [ ] machine-readable output
* [ ] error reporting

---

# 44. Probe

Create `tpt-media-probe`.

* [ ] identify container
* [ ] identify codec
* [ ] inspect tracks
* [ ] inspect duration
* [ ] inspect timestamps
* [ ] inspect dimensions
* [ ] inspect sample rate
* [ ] inspect bitrate
* [ ] inspect metadata
* [ ] report corruption
* [ ] report codec capabilities

---

# 45. WASM

Create `tpt-web`.

* [ ] WASM-compatible decoder
* [ ] `.tpt` parser
* [ ] streaming playback
* [ ] browser audio output
* [ ] browser video output
* [ ] JavaScript bindings
* [ ] memory limits
* [ ] browser demo
* [ ] performance benchmark

---

# 46. FFI

Create `tpt-media-sys`.

The FFI should expose the **stable media abstraction**, not internal implementation details.

Expose:

```text
Context
Packet
Frame
Codec
Encoder
Decoder
Container
Demuxer
Muxer
```

* [ ] C ABI
* [ ] opaque handles
* [ ] explicit ownership
* [ ] explicit allocation/free functions
* [ ] error codes
* [ ] ABI version
* [ ] thread-safety documentation
* [ ] safety documentation
* [ ] C header generation
* [ ] C integration tests

---

# 47. Security

* [ ] threat model
* [ ] hostile media-input model
* [ ] integer-overflow audit
* [ ] bounds-check audit
* [ ] allocation exhaustion protection
* [ ] recursion limits
* [ ] decompression-bomb protection
* [ ] malformed timestamp handling
* [ ] fuzzing CI
* [ ] security disclosure process

---

# 48. Provenance / IP

Maintain:

```text
algorithm
reference
source
derivation
implementation
license
patent review
status
```

For each non-trivial algorithm:

* [ ] identify mathematical source
* [ ] record public references
* [ ] record implementation derivation
* [ ] record contributor provenance
* [ ] review patent concerns
* [ ] document uncertainty

Source classifications:

```text
GREEN
Public-domain mathematics
Textbooks
Academic papers
Expired patents
Public standards where legally appropriate

YELLOW
Open-source implementations
Modern patents
Ambiguous licensing
Vendor documentation

RED
Proprietary implementation source
Confidential information
Restricted specifications
Unclear ownership
```

Do not copy implementation code from external projects.

---

# 49. Clean-Room Development

For native codec implementations:

* [ ] document algorithm sources
* [ ] maintain implementation provenance
* [ ] avoid copying implementation structure from reference codecs
* [ ] do not derive code from proprietary source
* [ ] document external references used
* [ ] maintain codec-specific design notes

The purpose is provenance and independent implementation, not an absolute legal guarantee.

---

# 50. Documentation

Create:

* [ ] README
* [ ] ARCHITECTURE
* [ ] codec design guide
* [ ] container specification
* [ ] pipeline specification
* [ ] DSP dependency guide
* [ ] performance guide
* [ ] security model
* [ ] provenance/IP policy
* [ ] contributor guide
* [ ] API documentation
* [ ] examples

---

# 51. Cross-Repository Integration

## `tpt-dsp`

`tpt-media` should consume:

```text
FFT
DCT
MDCT
LPC
pitch
VQ
psychoacoustics
filters
resampling
SIMD
```

* [ ] establish versioning strategy
* [ ] establish MSRV compatibility
* [ ] establish feature flags
* [ ] establish benchmark ownership
* [ ] establish API stability expectations

## `tpt-kinetix`

`tpt-media` should not duplicate standards implementation unnecessarily.

Potential future relationship:

```text
tpt-kinetix
    │
    └── standards codecs

tpt-media
    │
    └── TPT-native codecs
```

Shared generic DSP comes from:

```text
tpt-dsp
```

---

# 52. Dependency Graph

Target:

```text
                  tpt-media-core
                         │
          ┌──────────────┼──────────────┐
          │              │              │
   bitstream          codec        container
          │              │              │
          └──────────────┼──────────────┘
                         │
                      pipeline
                         │
                       apps
                         │
                         ▼
                      tpt-dsp
```

More accurately, DSP should be an independent lower-level dependency:

```text
              tpt-dsp
                 ▲
                 │
          ┌──────┴──────┐
          │             │
      tpt-media     tpt-kinetix
```

Avoid cyclic dependencies.

---

# 53. Release Gates

Before 0.1:

* [x] workspace builds — `cargo check --workspace` is clean
* [ ] CI operational — workflows exist but `cargo test --workspace` currently fails 2 of 12 test binaries; would be red today
* [x] basic media model
* [ ] timestamp model — blocked on the `rescale` overflow bug, see Known Issues
* [x] bitstream primitives — the range coder finding was corrected 2026-09-07 (all bitstream tests pass); this gate item was blocked only on that
* [x] container prototype
* [x] one working codec — Pulse and Aura both have real, signal-preserving encode/decode round trips
* [ ] fuzz infrastructure
* [ ] documentation baseline — substantial docs exist but describe the aspirational architecture, not current state; not yet audited for accuracy

Before 0.2:

* [ ] MDCT
* [ ] LPC
* [ ] VQ
* [ ] improved DSP APIs
* [ ] Pulse prototype
* [ ] Vox prototype
* [ ] container streaming
* [ ] basic pipeline

Before 0.3:

* [ ] Aura prototype
* [ ] Livelink prototype
* [ ] WASM decoder
* [ ] stable container specification
* [ ] benchmark suite

Before 0.4:

* [ ] Vault prototype
* [ ] Whisper prototype
* [ ] advanced quality testing
* [ ] security hardening

Before 1.0:

* [ ] stable public APIs
* [ ] stable `.tpt` specification
* [ ] stable codec specifications
* [ ] compatibility policy
* [ ] comprehensive fuzzing
* [ ] reproducible benchmarks
* [ ] documented security model
* [ ] documented provenance
* [ ] FFI stability
* [ ] WASM stability
* [ ] production-quality implementations

---

# 54. Final Architectural Acceptance Criteria

`tpt-media` is architecturally complete when:

```text
                    TPT MEDIA
                        │
       ┌────────────────┼────────────────┐
       │                │                │
    Container         Codec           Pipeline
       │                │                │
       │         ┌──────┴──────┐         │
       │         │             │         │
       │      Native       Standards     │
       │       codecs        codecs      │
       │         │             │         │
       └─────────┴─────────────┴─────────┘
                         │
                      tpt-dsp
                         │
          ┌──────────────┼──────────────┐
          │              │              │
       Transforms      Signal       Analysis
```

The architecture must satisfy:

* [ ] no duplicated DSP implementations
* [ ] no codec-specific DSP in `tpt-dsp`
* [ ] no media/container abstractions in `tpt-dsp`
* [ ] no cyclic dependencies
* [ ] real-time paths are allocation-free
* [ ] malformed media cannot cause unbounded resource consumption
* [ ] native codecs have explicit versioned specifications
* [ ] `.tpt` is independently implementable
* [ ] public APIs have documented contracts
* [ ] numerical algorithms have documented conventions
* [ ] optimized implementations have reference implementations
* [ ] provenance is recorded
* [ ] security testing is continuous
* [ ] the resulting crates are reusable outside `tpt-media`

---

# 55. Guiding Principle

The ultimate separation is:

```text
tpt-dsp
    = transform and understand signals

tpt-media
    = represent, encode, transport, synchronize and store media

tpt-kinetix
    = implement established media standards
```

A future TPT application should therefore be able to depend on:

```text
tpt-dsp
```

without importing media infrastructure, and:

```text
tpt-media
```

without needing to know how its underlying DSP algorithms are implemented.

This is the intended long-term architecture of the TPT media stack.

This gives you a single master checklist, while still making it clear to an agent that some [ ] items belong to the tpt-dsp repository rather than being implemented inside tpt-media.

---

# 56. Representation Research Platform

Investigate whether natural video can be represented more sparsely/efficiently
by a fundamentally different mathematical representation than block-transform
codecs, rather than only by improving prediction/transform/quantisation/
entropy coding within that paradigm. Livelink/Vault/Whisper (§32-34) already
assume the conventional block/motion-vector/DCT paradigm; this section builds
a swappable representation abstraction first, so those codecs can later become
instances of it rather than the only option ever tried. See
`docs/representation-research.md` for the full methodology once written.

This is explicitly *not* an attempt to build a better codec. The deliverable
is the experimental apparatus plus enough concrete representations to prove
it isn't a speculative abstraction with nothing validating it.

## Architecture

* [ ] `tpt-media-representation` crate — `Representation`/`Quantizer`/`RateEstimator`/`DistortionMetric` trait contracts + `SignalView`/`SignalBuffer` spatiotemporal (`V(x,y,t)`) signal type, kept separate from `tpt-media-core`'s packed-pixel `VideoFrame` because the research question is mathematical (real-valued signals, arbitrary transforms), not about production pixel formats
* [ ] `tpt-media-research` crate — synthetic signal generators, entropy estimation, the RD/sparsity experiment harness, and the concrete representations below
* [ ] "no free decoder-side information" rate-accounting rule enforced structurally: `Coeffs`/`Quantized` types carry the complete decoder-required state (dictionaries, motion parameters, mode choices included), so a `RateEstimator` summing bits over the whole value cannot silently omit side information
* [ ] primary/secondary/diagnostic metric hierarchy is the documented (and implemented) methodology: bits/sample vs. distortion is the RD frontier (primary); entropy, non-zero count, energy compaction are secondary diagnostics; coefficient-retention curves are diagnostic-only and never used to declare a representation "better"
* [ ] `tpt-dsp` is not modified — 2D transforms are built as separable passes over the existing 1D `dct_ii`/`dct_iii`; entropy estimation, synthetic signals, and VQ-codebook *training* (tpt-dsp only has VQ *search*) live in `tpt-media-research`

## Representations

* [ ] `baseline` — raw/uncompressed identity representation, the RD anchor and Phase 0 reference point
* [ ] `block_dct` — Avenue A: separable 2D DCT-II over a configurable block size, via row+column passes of `tpt-dsp-core::dct::{dct_ii,dct_iii}`
* [ ] `temporal_dct` — previous-frame prediction (open-loop) + block DCT on the residual; not one of the "3 representations," just the 3rd oracle-experiment condition
* [ ] `patch_vq` — Avenues C/D: patch extraction + a Lloyd/k-means-trained codebook (new, local to `tpt-media-research`) + `tpt-dsp-core::vq` search/encode/decode; dictionary transmission cost explicitly counted in the rate
* [ ] `motion_aligned` — Avenue F: coordinate warp `x' = x - vt` (bilinear, toroidal) before representation; velocity is embedded in `Coeffs` so its transmission cost is counted even though the oracle experiment hands it the *true* velocity — knowing it exactly doesn't make it free to decode

## Oracle experiment

* [ ] four-condition comparison — raw pixels, spatial DCT, DCT + previous-frame prediction, motion-aligned using the *true* velocity — on a synthetic moving edge `I(x,t) = f(x - vt)` with known `v`, compared at matched rate points
* [ ] the test harness asserts only measurement validity (every reported value finite; each representation's own round trip reconstructs within tolerance; matched-rate comparison interpolates within the sampled range, never extrapolates; identical inputs reproduce byte-identical output) — it never asserts the hypothesized direction of the result, so the experiment is allowed to conclude "motion alignment didn't help under these conditions"
* [ ] actual result, recorded honestly once run: *(not yet run)*

## Phases

* [ ] Phase 0 — establish the truth: raw vs. DCT, prove the RD/measurement harness is honest
* [ ] Phase 2 — challenge locality: blocks (`block_dct`) vs. patches (`patch_vq`)
* [ ] Phase 4 — challenge coordinates: stationary vs. motion-aligned (the oracle experiment)
* [ ] Phase 5 — challenge representation: coefficients (`block_dct`) vs. dictionary/VQ (`patch_vq`)
* Phase 1 (DCT vs. alternative transforms), Phase 3 (2-D frame vs. 3-D space-time), and Phase 6 (combine winners) are enabled by the abstraction but explicitly deferred, not implemented this milestone

## Compatibility with Vault/Whisper (§33-34)

* [ ] no code changes to Vault/Whisper needed yet — neither has any implementation on disk. Recorded intent only: Vault's eventual prediction → partition → transform → quantisation → entropy pipeline and Whisper's eventual patches → dictionary → VQ → temporal-prediction pipeline should each become expressible as one `Representation` impl, without forcing either codec's internals to look identical to the other

One thing I particularly like about this arrangement is that if you later decide to build something like RF codecs, scientific signal codecs, telemetry compression, or robotics signal processing, the DSP work we've done for media isn't trapped inside tpt-media; it becomes part of the reusable TPT foundation.