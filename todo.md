# TPT Media — Master Implementation TODO

**Repository:** `tpt-media`
**Organization:** TPT Solutions
**Language:** Rust
**Status:** Early Implementation
**Document version:** 2.1
**Last updated:** 2026-09-09

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
* **2026-09-11 — a real bug in `tpt-media-bitstream`'s `BitReader::skip`,
  caught by a new Pulse test, not by review.** Skipping a non-byte-aligned
  span of bits that crossed multiple bytes computed the right number of
  bytes to advance past, but then re-fetched the *following* byte (one
  past the skip region) to recover the last skipped byte's unconsumed
  trailing bits, instead of the correct byte — silently corrupting every
  read after the first multi-byte `skip` in a stream. It went unnoticed
  because the only production caller (Aura's `read_exp_golomb`) only ever
  calls `skip(1)`, which never takes the multi-byte code path, and the
  crate's own pre-existing `skip_across_bytes` test happened to skip
  exactly the number of bits already sitting in the accumulator, also
  never exercising the buggy path. Found while adding a Pulse test that
  parses a real encoded packet's header by skipping over each band's
  coefficients with `skip(n_coeffs * bits_per_coef)` — a genuine
  multi-byte, non-aligned skip. Fixed by rewriting `skip` to advance
  `pos` past whole bytes directly (no accumulator involvement) and only
  reload one byte for a genuine partial-byte remainder, discarding its
  already-skipped leading bits rather than fetching the wrong byte
  entirely. New regression tests:
  `skip_past_a_fresh_reader_leaves_trailing_bits_of_the_last_byte_readable`,
  `skip_an_exact_multiple_of_8_bits_consumes_no_extra_byte`,
  `skip_beyond_available_bits_is_end_of_input`.
* **2026-09-11 — two more real bugs, both caught by new §38 fuzz sweeps,
  not by review.** (1) `tpt-media-bitstream::range::RangeDecoder::decode`
  divides by zero on a *second* call after an earlier call already
  returned `EndOfInput`: running out of input partway through
  renormalization leaves `range` below `TOP` (and potentially below
  `total`) without ever restoring it, so `range/total` on the next call
  can be `0`. Fixed by rejecting `total > self.range` up front (which
  also covers the general case of a caller-supplied `total` genuinely
  exceeding `range`, not just this poisoned-state case). (2)
  `tpt-audio-pulse`'s decoder read `n_bands` and each band's `n_coeffs`
  directly from the packet with no bound relative to `frame_size`, so a
  malformed/adversarial packet claiming more bands/coefficients than
  actually fit in the decoded spectrum buffer indexed it out of bounds
  and panicked instead of returning `CodecError::MalformedPacket`. Fixed
  by bounds-checking `start + n_coeffs <= spec.len()` (and rejecting
  `n_bands == 0`) before writing. Neither bug was reachable by any
  existing unit test — both were found by the first fuzz-style
  corruption sweep written for their respective crates, underscoring why
  §38's fuzzing checklist matters even for code that already has
  conventional unit tests.

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
  So Pulse's *previous* code called the direct `mdct`/`imdct`, which already missed this target at its larger supported frame size, and only barely cleared it at the smaller one, before accounting for anything else in the encode/decode path. **Follow-up done 2026-09-08:** `codecs/pulse/src/lib.rs` now uses `FastMdctPlan` — `PulseEncoder` holds one plan sized to its fixed `frame_size`; `PulseDecoder` holds one plan per supported size (128 and 256, selected per-packet via `mdct_plan_for`, since the wire format self-describes frame size independent of the decoder's own configured default) so a size-128 packet decoded on a size-256-configured decoder still inverts correctly. Plans are built once at construction (the one-time FFT-plan/twiddle allocation) and only `forward`/`inverse` run in the hot path, which allocate nothing. All 6 existing `tpt-audio-pulse` tests still pass unchanged. `FastMdctPlan` clears the ≤10% target by ~3 orders of magnitude at both sizes, so the codec now meets its own real-time claim in practice, not just in the benchmark suite.
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

**Audited 2026-09-07**, docs completed 2026-09-08, against the actual code
in `tpt-dsp-core/src/*.rs`, now that all 8 required algorithm families
genuinely exist. This section's 9 items describe a *formal, uniform
contract* across all 8 families:

* [x] Define input domain — every module now has `# Panics` sections
  documenting its domain restrictions; `fft.rs`'s `twiddles`/`fft_inplace`/
  `fft_inplace_f32`/`ifft_inplace`/`fft`/`ifft` (the one gap found in the
  2026-09-07 audit) got explicit `# Panics` sections added 2026-09-08
* [x] Define output domain — every one of the 8 families now has an
  explicit "**Output domain**" line in a `# Numerical contract` module-doc
  section, added 2026-09-08 (e.g. "unbounded, finite input → finite
  output" for the linear transforms; a defined index range for VQ; a
  non-negative range for `db_to_power`)
* [x] Define normalization — FFT's forward/inverse convention (previously
  only demonstrated via tests) and DCT's "unnormalized, round trip needs
  an explicit `N/2` divide" convention are now stated in prose in each
  module's `# Numerical contract` section, alongside the pre-existing
  MDCT/LPC/VQ convention docs
* [x] Define precision — each of the 8 families' `# Numerical contract`
  section now states a concrete expected relative/absolute error, backed
  by an actual named test (e.g. FFT ~1e-3/1e-4 in `f32`, ~1e-9/1e-10 in
  `f64`; MDCT direct path ~1e-6 in `f64`, `FastMdctPlan` ~1e-3/1e-4 in
  `f32`)
* [x] Define acceptable error — the stated policy, applied uniformly
  across all 8: "the tightest tolerance the test suite currently passes
  at the sizes exercised, not a derived theoretical bound" — documented
  explicitly as such in each family's contract section so it isn't
  mistaken for a formal error-analysis result. VQ and LPC's `predict`/
  `residual` are the two genuine exceptions, correctly documented as
  *exact* (finite arithmetic on caller data, not an approximation with a
  tolerance to state)
* [x] Define valid sizes — genuinely uniform: every module (including
  `fft.rs`) asserts its size constraints (power-of-two, even order,
  matching lengths, etc.) and panics rather than producing garbage
* [x] Define failure behavior — uniform: every algorithm here panics on
  invalid input (no silent wrong answers); `lpc_to_lsp` is the one
  exception with a real non-panic failure mode (`bool` return when the
  input isn't stable enough to find all roots), and that's documented
* [ ] Define determinism expectations — implicit everywhere (pure
  functions, no RNG, so trivially deterministic) but still not *stated*
  per-module; left unchecked deliberately — §39/§40 elsewhere in this
  document cover determinism/SIMD-equivalence at the workspace level,
  which is arguably the right place for this, making a per-algorithm
  restatement here low-value rather than a real gap
* [x] Add reference tests — genuinely uniform and the strongest item:
  every one of the 8 families below has a real reference/round-trip/
  equivalence test with an explicit tolerance (FFT vs. naive DFT,
  MDCT/DCT-IV fast-vs-direct, LPC round-trip through LSP, VQ
  accelerated-vs-linear, psychoacoustic model validation, filters/
  resampling have their own pre-existing tests)

Required for (all 8 families' `# Numerical contract` doc sections added
2026-09-08 in `tpt-dsp-core/src/{fft,dct,mdct,lpc,filters,resample,vq,
psychoacoustic,bark}.rs`; `cargo test -p tpt-dsp-core --all-features` (117
tests + 3 doctests) and `cargo clippy --all-features --all-targets -D
warnings` both still pass, and `cargo doc --all-features --no-deps`
introduces no new broken/private intra-doc links):

* [x] FFT — `# Panics` added to every function; output domain, normalization (forward unnormalized / inverse `1/N`), and precision (~1e-3/1e-4 `f32`, ~1e-9/1e-10 `f64`) now stated in prose
* [x] DCT — output domain, the "unnormalized, round trip needs an explicit `N/2`" convention, and precision (direct ~1e-3/1e-4, `FastDctIvPlan` ~1e-4 relative) now stated
* [x] MDCT — was already the best-documented family (convention/normalization/panics); precision/acceptable-error now added (~1e-6 `f64` direct, ~1e-3/1e-4 `f32` `FastMdctPlan`)
* [x] LPC — precision/acceptable-error added, correctly distinguishing `predict`/`residual`'s *exact* arithmetic from `levinson_durbin`'s ~1e-9 approximation
* [x] filters — input/output domain and precision now stated, correctly scoped: RBJ cookbook designs are spot-checked (1e-3/1e-6), but `process_biquad`/`Fir`/`IirFilter` themselves have no general error bound since they're exact arithmetic on caller-supplied coefficients
* [x] resampling — input/output domain and precision (chunked-vs-one-shot equivalence, 1e-6) now stated, explicit that stopband attenuation is a per-design property, not a machinery-wide contract
* [x] VQ — documented as *exact* search (no approximation tolerance), with the one real numerical claim (accelerated search never disagrees with linear scan) called out as bit-identical, not merely close
* [x] psychoacoustic calculations — documented honestly as a modeling toolkit where most validation is *relational* (ordering, sign, decay-with-distance), not a numeric tolerance; the one genuine round-trip identity (`db_to_power(power_to_db(p))`) does get a stated 1e-9 tolerance

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
* [x] entropy utilities — `entropy.rs` (`shannon_entropy_bits`/`_nats`, `normalize`, `histogram`), 10 tests, wired into `lib.rs` (2026-09-09)
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
* [x] define drain semantics — `Decoder::drain` / `Encoder::drain` (graceful EOS distinct from hard `flush`), plus `DynDecoder::drain`/`DynEncoder::drain` and `Capabilities::supports_drain`/`supports_encoder_drain`; default forwards to `flush` for codecs with no buffered output (2026-09-09)
* [x] define error model
* [x] define configuration model
* [x] define capability discovery
* [x] define hardware/SIMD capability exposure
* [x] define real-time capability
* [x] define supported formats — `SupportedFormat` (audio/video) + `CodecInfo::supported_formats` + `supports_format()`, letting pipelines validate track→codec pairings up front (2026-09-09)

---

# 19. Resource Limits

All decoders must support bounded resources.

* [x] maximum frame dimensions — Pulse/Aura decode paths now call `limits.check_frame_bytes(...)` before producing output (2026-09-09)
* [x] maximum packet size
* [x] maximum metadata size — 2026-09-11: `tpt_media_codec::ResourceLimits`
  gained `max_metadata_bytes` (`permissive()` 16MiB, `strict()`/`Default`
  64KiB) + `ResourceLimitKind::MetadataSize` + `check_metadata`, following
  the existing `check_packet`/`check_allocation` pattern. Wired into
  `tpt-container`'s `TptDemuxer`: a new `limits: ResourceLimits` field
  (new `with_limits`/`new_streaming_with_limits` constructors alongside
  the existing `new`/`new_streaming`, which now default to
  `ResourceLimits::default()`), checked in `index_tmta` against the raw
  `TMTA` chunk body length *before* parsing it, returning the
  pre-existing-but-previously-unused `TptError::ResourceExceeded`. Test:
  `oversized_tmta_metadata_is_rejected_by_default_but_allowed_with_permissive_limits`
  (a ~100KB metadata entry is rejected under strict/default limits but
  accepted and recovered intact under permissive limits).
* [x] maximum allocation — Pulse (`check_allocation` on the decoded spectrum buffer) and Aura (lossless coefficient/sample buffers, lossy MDCT spec+synthesis buffers) now call `limits.check_allocation(...)` before allocating; both have tests asserting a tight budget yields `ResourceLimit(Allocation)` on a structurally valid packet (2026-09-09)
* [ ] maximum reference frames — still no video codec to exercise it
* [ ] maximum recursion depth — no recursive codec decode paths yet
* [ ] maximum decode work where practical — `max_decode_micros` still uncalled (would need a measured loop budget)
* [ ] cancellation support — `CodecError::Cancelled` exists but no codec produces/checks a cancellation token yet
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
* [x] define versioning — `version` is major<<8|minor; `from_bytes` rejects version 0, future minor, or higher major with `UnsupportedVersion` (2026-09-08)
* [x] define reserved fields — 20-byte header gains a `reserved u32`, zero when writing, ignored on read; header_size may grow (2026-09-08)
* [x] define compatibility rules — unknown/reserved chunks skipped, header reserved bytes ignored, version major bumps reserved for binary-incompatible changes (2026-09-08)

---

# 21. Container Track Structure

Track metadata must include:

* [x] track ID
* [x] media type — TRAK carries `media_type` u8 (audio/video/subtitle/metadata); `index_trak` decodes it instead of hardcoding Audio (2026-09-08)
* [x] codec ID
* [x] codec version — `codec_version u16` is a first-class TRAK field (2026-09-08)
* [x] codec-private data
* [x] timebase — TRAK serializes the per-track timebase (num/den); falls back to the global timebase when absent (2026-09-08)
* [x] sample rate where applicable — `sample_rate u32` in TRAK (2026-09-08)
* [x] channel layout where applicable — `channel_layout u8` tag + `channel_mask u16` (2026-09-08)
* [x] dimensions where applicable — `width/height u32` in TRAK (2026-09-08)
* [x] pixel format where applicable — `pixel_format u8` (0xFF = none) (2026-09-08)
* [x] metadata — typed key/value pairs in a TMTA chunk (string/int/bool/bytes) (2026-09-08)

---

# 22. Container Cluster Structure

Clusters should support:

* [x] timestamp
* [x] duration — optional `duration u32` field, enabled by `CLUS` flags bit 3 (2026-09-08)
* [x] track ID
* [x] PTS — distinct `pts u64` field (2026-09-08)
* [x] DTS — distinct `dts u64` field; `next_packet` restores it instead of forcing `dts = pts` (2026-09-08)
* [x] keyframe flag
* [x] discontinuity flag — flags bit 2 is `DISCONTINUITY` (bits: 0 keyframe, 1 end-of-frame, 2 discontinuity); round-trip test added (2026-09-08)
* [x] payload
* [x] optional checksum — `CLUS` flags bit 4 enables a per-cluster CRC-32 over the body up to the checksum; verified by `verify_cluster_checksums()` (2026-09-08)

---

# 23. Container Index

Do not require scanning the entire file for normal seeking.

* [x] define index structure
* [x] index keyframes
* [x] index cluster offsets
* [x] support per-track indexes — `track_index(track_id)` + `seek_to_keyframe_on_track`; per-track entries in the on-disk `IDX ` chunk (2026-09-08)
* [x] support time-based lookup
* [x] support offset-based lookup — `seek_to_offset(offset)`; on-disk entries carry absolute byte offsets (2026-09-08)
* [x] define index versioning — `index_version u8`; an unknown version is ignored and seeking falls back to the scan index (2026-09-08)
* [x] define recovery behavior — missing/corrupt/newer `IDX ` is ignored; `rebuild_index()` regenerates the in-memory index from a scan (2026-09-08)

---

# 24. Container Robustness

* [x] corruption detection
* [x] truncated-file handling
* [x] incomplete final cluster handling — dedicated tests: strict parse errors `Truncated`, streaming parse pauses and resumes via `feed()` (2026-09-08)
* [x] unknown chunk skipping
* [x] forward-compatible chunk types — reserved range (first byte 0x00..=0x1F) and unknown IDs skipped; dedicated test (2026-09-08)
* [x] checksum support — per-cluster CRC (optional) and container `CRC1` footer (always on finalize) (2026-09-08)
* [x] append mode — muxer appends chunks without seek-back; `append_raw` splices pre-serialized chunks (2026-09-08)
* [x] live-stream mode — `new_streaming` + `feed()` continue parsing a growing buffer (2026-09-08)
* [x] recovery/index rebuilding — `rebuild_index()` and `reconstruct_trailer()` (2026-09-08)
* [x] fuzz testing — deterministic corruption/truncation sweeps (every truncation point + 512 seeded bit-flips) never panic; no external fuzz harness deps (2026-09-08)

---

# 25. Container Streaming

Support files that are not yet complete.

* [x] fragmented/clustered writing — muxer appends complete TLV chunks as they are produced (2026-09-08)
* [x] incremental parsing — `new_streaming` tolerates a partial trailing chunk and resumes on `feed()` (2026-09-08)
* [x] live append — `feed` continues scanning a growing buffer; tests feed 8 bytes at a time end-to-end (2026-09-08)
* [x] missing final index — streams without `IDX `/`CRC1` still reproduce every packet; EOF is signalled by `Truncated` after the last cluster (2026-09-08)
* [x] streaming playback — packets are served from the scan index while the buffer is still being appended to (2026-09-08)
* [x] graceful termination — `finalize()` writes the `IDX ` + `CRC1` trailer (2026-09-08)
* [x] optional footer/index reconstruction — `reconstruct_trailer()` returns the trailer bytes for a trailer-less stream (2026-09-08)

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

* [x] define node abstraction
* [x] define stream negotiation
* [x] define format negotiation
* [x] define backpressure
* [x] define buffering
* [x] define cancellation
* [x] define error propagation
* [x] define flushing
* [x] define end-of-stream
* [x] define timestamps through pipeline
* [x] define threading model
* [x] define realtime scheduling profile
  * 2026-09-08: implemented in `crates/tpt-media-pipeline`; docs in
    `docs/pipeline-design.md`. `Node`/`NodeEvent`/`NodeError` contract,
    lockstep `Chain<L,R>` with cascading flush, `Pipeline` pull/drive with
    stall-bound backpressure guard, `Bounded`/`Cancellable`/`Map`/`Sink`/
    `Source` nodes, `Timestamped<T>` passthrough, `MediaFormat`/
    `StreamSpec`/`FormatAware` negotiation, `Executor` (`Inline`,
    `ThreadedExecutor` std-only), `RealtimeProfile`/`ScheduledRun`/
    `LatencyObserver`/`LatencyLog` with clock injection. 21 unit + 1
    container integration test (`tests/container_flow.rs`, packets flow
    with PTS intact); crates/tpt-media-pipeline compiles `no_std`, clippy
    clean, workspace tests green.

---

# 27. Media I/O

Create `tpt-media-io`.

Reuse `tpt-dsp-io` rather than duplicating functionality.

* [x] define PCM integration
* [x] integrate WAV
* [x] integrate raw audio
* [x] define image/video frame I/O
* [x] define streaming sources
* [x] define streaming sinks
* [x] define async integration where appropriate

Done 2026-09-08. `crates/tpt-media-io` (no_std + alloc; `std` default
feature). New `IoError` model (UnexpectedEof/InvalidHeader/Unsupported/
Overflow/Other). PCM: `PcmSpec` + `encode_pcm`/`decode_pcm` with chunked
monotonic PTS + `normalize_to_f32`/`denormalize_from_f32` (f32 bit-exact for
≤24-bit int). WAV: byte-exact `parse_wav`/`decode_wav`/`write_wav` for the
codec pipeline plus a std-gated `f32` bridge that **reuses `tpt-dsp-io`**
(`decode_wav_f32`/`write_wav_f32`) — no duplication. Raster frame I/O:
dependency-free PPM P6 / PGM P5 + `checkerboard` fixtures. Streaming
`FrameSource`/`MediaItem` pull and `FrameSink` push traits with concrete
readers/writers (slice/vec in no_std; file/stdin/stdout under `std`). Async
integration: `AsyncFrameSource`/`AsyncFrameSink` (RPITIT + `Send`) and a
sync `BlockingAdapter`; dependency-free. Design: `docs/media-io-design.md`.
32 unit tests.

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
* [x] optional perceptual weighting — 2026-09-11:
  `PulseEncoderConfig::perceptual_weighting` (default `false`, so every
  existing caller keeps its exact previous behavior). Unlike Aura's full
  simultaneous-masking model (§10), Pulse deliberately uses only the
  cheapest piece of that toolkit — the absolute threshold of hearing
  (`tpt-dsp-core::threshold_in_quiet`) — evaluated once per band at
  construction time (not per frame, since it depends only on the fixed
  band layout), consistent with Pulse's low-latency/low-complexity
  design goal. Each band's configured step is scaled by that band's
  threshold relative to a 1kHz reference; the scaling only ever coarsens
  a step, never sharpens below the configured value, since the wire
  format's step field is an integer with no sub-configured granularity.
  No wire-format change (Pulse already transmits the actual per-band
  step used). Tests: `perceptual_weighting_off_by_default_leaves_step_unchanged`,
  `perceptual_weighting_coarsens_less_sensitive_bands_never_below_configured`,
  `perceptual_weighting_round_trip_carries_signal`. Writing a test that
  inspects the transmitted per-band steps surfaced a real, previously
  latent bug in `tpt-media-bitstream`'s `BitReader::skip` (multi-byte
  skips silently corrupted all subsequent reads) — fixed separately, see
  `CHANGELOG.md`.

## Codec

* [x] define frame sizes
* [x] define bitstream
* [ ] define modes — Pulse has no distinct "modes" concept
* [x] define quantisation
* [x] implement encoder
* [x] implement decoder
* [x] implement PLC
* [x] test packet loss — `plc_conceals_mid_stream_packet_loss` (2026-09-09): encodes a 4-frame tone, drops the middle packet, substitutes `conceal()` at its slot, and asserts the concealed frame carries the right sample count/PTS and non-zero energy (realised from the last decoded spectrum), with clean decode before and after the gap
* [x] test jitter — `jittered_pts_are_preserved_not_reordered` (2026-09-09): delivers valid packets in arrival order 0,2,3,1; the decoder must not panic/reorder on its own and each output frame echoes its own packet's PTS with full-length, non-silent output
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
* [x] checksum/integrity — every packet ends with a CRC-32 trailer (`crc32`, `CRC_TRAILER_BYTES = 4`) over all preceding payload bytes; the decoder verifies before parsing and rejects mismatches as `MalformedPacket`. Tests: trailer-round-trip, body-bit-flip rejected, checksum-bit-flip rejected, undersized packet rejected (2026-09-09). Note: this is a v0.1 wire-format addition — packets encoded before this change lack the trailer and are rejected by the new decoder
* [x] bit-exact round trip

## Lossy

* [x] MDCT
* [x] psychoacoustic model — 2026-09-11: `codecs/aura/src/lib.rs`'s lossy
  path now consumes `tpt-dsp-core`'s §10 psychoacoustic toolkit
  end-to-end (`hz_to_bark`, `classify_tonal_bins`, `simultaneous_masking`,
  `combined_threshold_db`, `threshold_in_quiet`) to derive a per-band
  quantization step, replacing the previous flat 16-bit-per-coefficient
  scheme. See `CHANGELOG.md` `[Unreleased] / Added` for the exact
  pipeline; this is a breaking change to the v0.1 lossy wire format
  (pre-existing lossy packets are not decodable by the new decoder).
* [x] Bark bands — same change: each MDCT bin is mapped to a Bark
  critical band via a new `mdct_bin_bark_bands`/`mdct_num_bark_bands`
  pair, deterministic from `(sample_rate, n_half)` alone so the mapping
  itself needs no wire representation; tested by
  `mdct_bin_bark_bands_is_deterministic_and_nondecreasing`
* [x] masking — `simultaneous_masking`/`combined_threshold_db` set each
  band's quantization step from its masking threshold; tested
  structurally by `lossy_step_table_reflects_masking_not_flat` (a tone's
  per-band step table must vary, not stay flat)
* [x] quantisation
* [x] noise shaping — first-order spectral noise shaping
  (`tpt-dsp-core::noise_shape_step`, feedback `[1.0]`) applied across
  MDCT bins in frequency order during lossy quantization, shaping the
  residual error from the per-band step above rather than leaving it
  independent per bin
* [ ] stereo model if required — not applicable yet: Aura is mono-only
  (`ChannelLayout::Mono` in `static_info`'s `supported_formats`); no
  consumer requires stereo, so left unimplemented rather than speculative
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

* [x] versioned bitstreams — 2026-09-11: Aura and Pulse packets both now
  lead with a 16-bit wire-format version field (`VERSION`, already
  existed as a constant but was never actually written to either
  codec's stream before this). A decoder reads it first and returns
  `CodecError::Unsupported` immediately on any mismatch, before
  interpreting any further bits as if they were its own layout — this is
  itself a breaking change to both v0.1 wire formats (packets from
  before this change have no version field and are misread as such;
  see `CHANGELOG.md`), but it's the last one that *can't* be handled
  gracefully — every future version bump can now be. Tests:
  `decode_rejects_unsupported_wire_version` (both codecs).
* [x] codec identification — each native codec has a fixed 4-byte
  `CODEC_ID` (`AURA`, `PULS`, ...), carried in the container's `TRAK`
  chunk (§21) and used by `Registry::find_decoder`/`find_encoder` (§18)
  to look up the right factory
* [x] configuration serialization — `CodecConfig::extra` (framework
  config) plus `TRAK`'s codec-private `data` + `codec_version` fields
  (§21) round-trip codec-specific configuration through the container
* [x] decoder capability negotiation — `CodecInfo::supported_formats` +
  `Capabilities` + `CodecInfo::supports_format()` (§18), consumed by
  `tpt-media-probe`'s capability table (§44)
* [x] graceful unsupported-mode handling — `Registry::find_decoder`/
  `find_encoder` return `Option` (no panic on an unknown codec ID);
  `DynEncoder::encode` returns `CodecError::Unsupported` for a
  frame/format a codec doesn't handle (e.g. Aura/Pulse reject
  non-audio `DynFrame`s); and the new versioned-bitstream check above
  extends this to unrecognized wire versions
* [ ] forward-compatible extension mechanism — the container format has
  one (reserved chunk ID ranges, §24), but the native codecs'
  *packet-level* bitstreams do not: every Aura/Pulse wire-format change
  so far (CRC trailer, lossy Bark-band quantization, the version field
  above) has been a breaking change with no reserved/extension bits.
  Left honestly unchecked rather than retrofitting an unused mechanism
  with no real requirement driving its shape yet.

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

**Strategy note (2026-09-11):** this project's established fuzzing
approach — set by `tpt-container`'s
`corruption_never_panics_and_truncation_never_panics` (§24) — is a
deterministic, dependency-free corruption/truncation sweep (a fixed-seed
LCG driving bit-flips and truncation over a real fixture, asserting
`Result`/no-panic, never `cargo-fuzz`/libFuzzer) rather than an external
fuzz harness, so results are reproducible without extra tooling. The
items below extend that same approach to every crate that parses
untrusted input; none of them add an external fuzz-harness dependency.

* [x] bitstream fuzzing — 2026-09-11: `tpt-media-bitstream::fuzz_like`
  (new module) sweeps `BitReader::{read,peek,skip}`, `HuffmanTable::decode`,
  `rice::decode_riced`, and `RangeDecoder::{new,decode}` over arbitrary/
  corrupted bytes and widths. **Found and fixed a real bug**: `RangeDecoder::decode`
  divided by zero on a *second* call after a prior call had already
  returned `EndOfInput` (renormalization can leave `range` below `total`
  without restoring it) — see `CHANGELOG.md` `[Unreleased] / Fixed` and
  the new `decode_keeps_returning_err_after_running_out_of_input_instead_of_panicking`
  regression test.
* [x] container fuzzing — already done 2026-09-08, see §24
  (`corruption_never_panics_and_truncation_never_panics`); this item was
  just never checked off here
* [x] codec fuzzing — 2026-09-11: `decode_never_panics_on_corrupted_or_arbitrary_packets`
  added to both `tpt-audio-aura` and `tpt-audio-pulse` (arbitrary bytes +
  full truncation sweep + bit-flip sweep of a real packet). **Found and
  fixed a real out-of-bounds write in Pulse**: `n_bands`/`n_coeffs` are
  read straight from the packet with no bound relative to `frame_size`,
  so a malformed/adversarial packet could index the decoded spectrum
  buffer out of range and panic instead of returning
  `CodecError::MalformedPacket` — see `CHANGELOG.md` and the new
  `decode_rejects_n_coeffs_that_would_overrun_the_spectrum` /
  `decode_rejects_zero_bands` regression tests
* [x] probe fuzzing — 2026-09-11: `probe_never_panics_on_corrupted_or_arbitrary_bytes`
  in `tpt-media-probe` (truncation sweep, bit-flip sweep, arbitrary
  bytes); no new bugs found (`probe()` is built on the already-hardened
  `TptDemuxer`)
* [x] malformed metadata — covered by §19's
  `oversized_tmta_metadata_is_rejected_by_default_but_allowed_with_permissive_limits`
  plus the container-level corruption sweep (§24), which includes `TMTA`
  chunks in its fixture
* [x] truncated packets — covered by the container truncation sweep (§24)
  and the new per-codec truncation sweeps above (Aura/Pulse)
* [x] random packet ordering — `tpt-audio-pulse`'s
  `jittered_pts_are_preserved_not_reordered` (§29, added 2026-09-09)
  delivers packets out of PTS order and asserts no reordering/panic;
  Aura has no cross-packet decode state at all (`AuraDecoder` holds no
  mutable state between `decode()` calls), so packet ordering is
  structurally a non-issue there, not just untested

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

* [x] codec selection
* [x] container selection
* [x] quality configuration
* [x] bitrate configuration
* [x] metadata manipulation
* [x] streaming stdin/stdout
* [x] machine-readable output
* [x] error reporting

Done 2026-09-08. `apps/tpt-cli` (`tpt` binary + lib core). `probe` /
`inspect` / `list-codecs` / `--help` fully implemented; `encode` and
`decode` are now live end-to-end (WAV → `pulse`/`aura` codec → `.tpt`, and
back), using the §27 media I/O (`decode_wav`/`write_wav`) and the native
codec encoder/decoder plumbing. Remaining: `transcode` and `benchmark`
validate their config then exit 3. Design: `docs/cli-design.md`. Added to
workspace members (`apps/` now enabled).

---

# 44. Probe

Create `tpt-media-probe`.

* [x] identify container — magic sniff (`TPTM`, `RIFF`/`WAVE`, unknown)
* [x] identify codec — codec ID + bitstream version from `TRAK`
* [x] inspect tracks — audio / video / subtitle / metadata track lists
* [x] inspect duration — longest track (last PTS + duration), seconds
* [x] inspect timestamps — per-track first/last PTS (track timebase)
* [x] inspect dimensions — width/height/pixel format from `TrackMedia`
* [x] inspect sample rate — sample rate + channel layout from `TrackMedia`
* [x] inspect bitrate — payload bits ÷ duration (per-track and overall)
* [x] inspect metadata — track name/language and typed `TMTA` pairs
* [x] report corruption — parse errors, truncation, version, container
      `CRC1` mismatch, per-cluster checksum failures; never panics
* [x] report codec capabilities — framework `Registry` availability plus
      the native codec family table (2026-09-08)

The probe consumes `tpt-container` directly via `TptDemuxer` (tracks +
full packet scan); the dependency edge was inverted so probe → container
(removed the unused `tpt-media-probe` edge from `tpt-container`, §52).

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
* [x] integer-overflow audit — 2026-09-11: audited every length-prefixed
  field decoded from untrusted bytes in `tpt-container/src/format.rs`
  (`decode_tmta`/`decode_idx`/`decode_trak`/`decode_clus`) for
  wraparound-prone `usize` arithmetic on attacker-controlled `u32`/`u64`
  lengths. Found and fixed one real gap: `decode_tmta`'s `META_BYTES`
  branch used plain `p + len` (`len` a full attacker-controlled `u32`),
  which is safe on 64-bit `usize` but can wrap on a 32-bit target (this
  workspace explicitly builds/targets 32-bit — `thumbv7em-none-eabi` for
  Vox, and wasm32 for the planned §45 WASM target) and pass a bounds
  check it should have failed. Fixed with `checked_add`. Every other
  length field in that file is either itself narrow enough (`u8`/`u16`)
  to make wraparound unreachable on any realistic target, or already
  goes through `checked_add` (the chunk-size field in `read_chunk_header`
  / `TptDemuxer::scan`). Test:
  `decode_tmta_rejects_bytes_value_length_that_would_overrun_the_body`
* [x] bounds-check audit — 2026-09-11: covered together with §38's fuzz
  sweeps (Aura/Pulse OOB write fix) and the integer-overflow audit above;
  see `CHANGELOG.md` for the full list of fixes this produced
* [x] allocation exhaustion protection — 2026-09-11: found and fixed a
  real gap in `tpt-container`'s `decode_idx`: `entry_count` is a fully
  attacker-controlled `u32` (up to ~4.29 billion) that was passed
  straight to `Vec::with_capacity` with no check against the actual
  (possibly tiny) input size, so a malformed `IDX ` chunk of only a few
  bytes could trigger a multi-gigabyte allocation attempt. Fixed by
  rejecting `entry_count` up front unless it could actually fit in the
  remaining body (each entry is a fixed 17 bytes). Test:
  `decode_idx_rejects_entry_count_that_would_overrun_the_body`. §19's
  `max_metadata_bytes` (added earlier this session) covers the same class
  of risk for `TMTA` at the chunk-size level.
* [ ] recursion limits — still no recursive codec decode paths to bound
  (same status as §19's own recursion-depth item)
* [x] decompression-bomb protection — the `decode_idx` fix above *is* a
  decompression-bomb fix (a few input bytes claiming a multi-gigabyte
  logical size); §19's metadata/allocation/packet/frame size limits
  cover the rest of this surface
* [ ] malformed timestamp handling — `Timestamp::rescale` overflow
  handling (§16) covers arithmetic overflow during timebase conversion,
  but no decode path currently validates a PTS/DTS read from a `CLUS`
  chunk against any sanity bound (e.g. absurdly large values that are
  merely *stored* correctly but would misbehave in a downstream
  consumer) — left honestly unchecked, no concrete bug found here this
  session, just not yet audited
* [x] fuzzing CI — `cargo test --workspace --locked` (already the
  existing CI gate, `.github/workflows/ci.yml`) runs every `#[test]`
  including all of §38's new fuzz-style sweeps; no separate workflow
  step needed since these are plain deterministic unit tests, not an
  external fuzz-harness job
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
* [x] CI operational — `.github/workflows/ci.yml` gates (`fmt --check`, clippy `--all-targets -- -D warnings`, `cargo build/test --workspace --locked`) all pass, and the no_std `tpt-audio-vox` job (`--target thumbv7em-none-eabi --no-default-features`) now builds: workspace `default-features = false` on `tpt-media-{core,bitstream,codec}` with members opting back into `std`, thiserror 1.x→2.x (1.x hard-requires `std`), `String`/no_std-gate fixes in `tpt-media-codec`, and `libm` for the required float math (2026-09-09)
* [x] basic media model
* [x] timestamp model — rational timebase, `rescale` overflow bug fixed 2026-09-07 (see Known Issues), signed-duration saturation + `checked_{add,sub}_duration`, disciplined `ClockDomain`/`SyncMap` (§16 all green)
* [x] bitstream primitives — the range coder finding was corrected 2026-09-07 (all bitstream tests pass); this gate item was blocked only on that
* [x] container prototype
* [x] one working codec — Pulse and Aura both have real, signal-preserving encode/decode round trips
* [x] fuzz infrastructure — 2026-09-11: §38's deterministic
  corruption/truncation sweeps now cover `tpt-media-bitstream` (new,
  found a real bug), `tpt-container` (pre-existing, §24),
  `tpt-audio-aura`/`tpt-audio-pulse` (new, found a real bug), and
  `tpt-media-probe` (new). No external fuzz-harness dependency; see §38's
  strategy note for why
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
instances of it rather than the only option ever tried. Full methodology and
the honest recorded results live in `docs/representation-research.md`.

This is explicitly *not* an attempt to build a better codec. The deliverable
is the experimental apparatus plus enough concrete representations to prove
it isn't a speculative abstraction with nothing validating it.

## Architecture

* [x] `tpt-media-representation` crate — `Representation`/`Quantizer`/`RateEstimator`/`DistortionMetric` trait contracts + `SignalView`/`SignalBuffer` spatiotemporal (`V(x,y,t)`) signal type, kept separate from `tpt-media-core`'s packed-pixel `VideoFrame` because the research question is mathematical (real-valued signals, arbitrary transforms), not about production pixel formats
* [x] `tpt-media-research` crate — synthetic signal generators, entropy estimation, the RD/sparsity experiment harness, and the concrete representations below
* [x] "no free decoder-side information" rate-accounting rule enforced structurally: `Coeffs`/`Quantized` types carry the complete decoder-required state (dictionaries, motion parameters, mode choices included), so a `RateEstimator` summing bits over the whole value cannot silently omit side information
* [x] primary/secondary/diagnostic metric hierarchy is the documented (and implemented) methodology: bits/sample vs. distortion is the RD frontier (primary); entropy, non-zero count, energy compaction are secondary diagnostics; coefficient-retention curves are diagnostic-only and never used to declare a representation "better"
* [x] `tpt-dsp` is not modified — 2D transforms are built as separable passes over the existing 1D `dct_ii`/`dct_iii`; entropy estimation, synthetic signals, and VQ-codebook *training* (tpt-dsp only has VQ *search*) live in `tpt-media-research`

## Representations

* [x] `baseline` — raw/uncompressed identity representation, the RD anchor and Phase 0 reference point
* [x] `block_dct` — Avenue A: separable 2D DCT-II over a configurable block size, via row+column passes of `tpt-dsp-core::dct::{dct_ii,dct_iii}`
* [x] `temporal_dct` — previous-frame prediction (open-loop) + block DCT on the residual; not one of the "3 representations," just the 3rd oracle-experiment condition
* [x] `patch_vq` — Avenues C/D: patch extraction + a Lloyd/k-means-trained codebook (new, local to `tpt-media-research`) + `tpt-dsp-core::vq` search/encode/decode; dictionary transmission cost explicitly counted in the rate
* [x] `motion_aligned` — Avenue F: coordinate warp `x' = x - vt` (bilinear, toroidal) before representation; velocity is embedded in `Coeffs` so its transmission cost is counted even though the oracle experiment hands it the *true* velocity — knowing it exactly doesn't make it free to decode

## Oracle experiment

* [x] four-condition comparison — raw pixels, spatial DCT, DCT + previous-frame prediction, motion-aligned using the *true* velocity — on a synthetic moving edge `I(x,t) = f(x - vt)` with known `v`, compared at matched rate points
* [x] the test harness asserts only measurement validity (every reported value finite; each representation's own round trip reconstructs within tolerance; matched-rate comparison interpolates within the sampled range, never extrapolates; identical inputs reproduce byte-identical output) — it never asserts the hypothesized direction of the result, so the experiment is allowed to conclude "motion alignment didn't help under these conditions"
* [x] actual result, recorded honestly once run: at the single common rate 0.06787 b/s, motion-aligned is lossless (psnr inf) vs. spatial DCT 12.06 dB, temporal pred 10.48 dB, raw 3.32 dB — see `docs/representation-research.md` §2. The full matrix of recorded comparisons (incl. the honest losses: previous-frame pred *underperforming* spatial DCT, and the fixed-K codebook's hard distortion floor) is in that document.

## Phases

* [x] Phase 0 — establish the truth: raw vs. DCT, prove the RD/measurement harness is honest
* [x] Phase 2 — challenge locality: blocks (`block_dct`) vs. patches (`patch_vq`)
* [x] Phase 4 — challenge coordinates: stationary vs. motion-aligned (the oracle experiment)
* [x] Phase 5 — challenge representation: coefficients (`block_dct`) vs. dictionary/VQ (`patch_vq`)
* Phase 1 (DCT vs. alternative transforms), Phase 3 (2-D frame vs. 3-D space-time), and Phase 6 (combine winners) are enabled by the abstraction but explicitly deferred, not implemented this milestone

## Compatibility with Vault/Whisper (§33-34)

* [x] no code changes to Vault/Whisper needed yet — neither has any implementation on disk. Recorded intent only: Vault's eventual prediction → partition → transform → quantisation → entropy pipeline and Whisper's eventual patches → dictionary → VQ → temporal-prediction pipeline should each become expressible as one `Representation` impl, without forcing either codec's internals to look identical to the other

One thing I particularly like about this arrangement is that if you later decide to build something like RF codecs, scientific signal codecs, telemetry compression, or robotics signal processing, the DSP work we've done for media isn't trapped inside tpt-media; it becomes part of the reusable TPT foundation.