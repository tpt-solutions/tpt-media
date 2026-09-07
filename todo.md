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
* **`Timestamp::rescale` overflow detection is wrong.** It multiplies by
  `self.timebase.num` twice instead of by `target.num`, so
  `rescale_overflow_returns_none` fails — a case that should overflow
  does not.
* **Aura's residual coding is Exp-Golomb, not Rice**, despite the doc
  comment claiming Rice coding delegated to `tpt-media-bitstream`. Fix
  the code or the comment.
* **`CHANGELOG.md`'s `[Unreleased]` entry overstates scope.** It lists
  `apps/`, `ffi/`, `tpt-cli`, `tpt-web`, `tpt-media-sys`, and
  `tpt-video-{livelink,vault,whisper}` as added; none of these exist on
  disk or in `Cargo.toml`'s workspace members. Reconcile before next
  release notes.
* `cargo test --workspace` currently has 1 failing test binary
  (`tpt-media-core`, the `rescale` bug above) — `tpt-media-bitstream` now
  passes in full (see the range-coder correction above).
* **2026-09-07 tpt-dsp reconciliation**: verified §3–§14 against the
  sibling `tpt-dsp` repo's actual code (`tpt-dsp-core/src/*.rs`). MDCT
  (§5.1) and LPC (§6) are genuinely implemented and consumed by
  Pulse/Aura — flipped below. Pitch analysis (§7), VQ (§8), LSP/LSF
  (§9), psychoacoustics (§10), and noise shaping (§11) remain **entirely
  absent** from `tpt-dsp-core` — confirmed by grep, no matches. §12
  (Numerical Contracts) is left unchecked as a cross-cutting gate since
  it can't be satisfied while VQ/psychoacoustics don't exist yet.
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

* [ ] Define pitch-analysis API
* [ ] Implement autocorrelation pitch estimator
* [ ] Implement AMDF estimator
* [ ] Define pitch period range
* [ ] Define voiced/unvoiced detection
* [ ] Add confidence metric
* [ ] Add tests using synthetic signals
* [ ] Add speech corpus tests
* [ ] Benchmark

---

# 8. TPT-DSP — Vector Quantisation

Required by Vox and Whisper.

* [ ] Define generic VQ API
* [ ] Implement nearest-vector search
* [ ] Implement distance metrics
* [ ] Implement fixed-size codebooks
* [ ] Implement encoder
* [ ] Implement decoder
* [ ] Support caller-owned buffers
* [ ] Add deterministic behavior
* [ ] Add optional accelerated search
* [ ] Benchmark linear search
* [ ] Investigate tree/product quantisation for larger codebooks

VQ itself belongs in DSP.

The codec-specific codebook belongs in the codec.

---

# 9. TPT-DSP — LSP / LSF

Useful for speech coding.

* [ ] Define LSP representation
* [ ] Define LSF representation
* [ ] LPC → LSP conversion
* [ ] LSP → LPC conversion
* [ ] Stability validation
* [ ] Interpolation
* [ ] Quantisation support
* [ ] Tests

---

# 10. TPT-DSP — Psychoacoustics

Required primarily by Aura.

## 10.1 Frequency scales

* [ ] Bark scale
* [ ] critical-band mapping
* [ ] ERB utilities if useful
* [ ] frequency ↔ Bark conversion
* [ ] band aggregation

## 10.2 Psychoacoustic primitives

* [ ] threshold-in-quiet model
* [ ] tonal/noise classification
* [ ] spectral spreading
* [ ] simultaneous masking
* [ ] masking threshold
* [ ] perceptual weighting
* [ ] model validation
* [ ] benchmark

These should be generic models, not Aura-specific implementations.

---

# 11. TPT-DSP — Noise Shaping

* [ ] Define noise-shaping filter API
* [ ] Implement generic noise-shaping filter
* [ ] Support configurable orders
* [ ] Validate stability
* [ ] Add tests
* [ ] Add benchmark

---

# 12. TPT-DSP — Numerical Contracts

For every transform and numerical algorithm:

* [ ] Define input domain
* [ ] Define output domain
* [ ] Define normalization
* [ ] Define precision
* [ ] Define acceptable error
* [ ] Define valid sizes
* [ ] Define failure behavior
* [ ] Define determinism expectations
* [ ] Add reference tests

Required for:

* [ ] FFT
* [ ] DCT
* [ ] MDCT
* [ ] LPC
* [ ] filters
* [ ] resampling
* [ ] VQ
* [ ] psychoacoustic calculations

---

# 13. TPT-DSP — SIMD

* [x] Retain SIMD in `tpt-dsp` — `tpt-dsp-core/src/simd.rs` (`core::simd`-based complex mul/add/magnitude, FFT butterfly)
* [x] Remove planned `tpt-media-simd` — never created; no `tpt-media-simd` directory or crate exists
* [x] Establish scalar reference implementations — `simd_scalar.rs`, the default/fallback path
* [x] Establish SIMD equivalence tests — `simd_mul_matches_scalar`, `simd_add_matches_scalar`, `simd_magnitude_matches_scalar`, `simd_fft_matches_scalar_fft`
* [x] Benchmark SIMD kernels — `dsp_bench.rs` `bench_simd_helpers` group
* [ ] Document supported architectures — `core::simd` is portable/architecture-agnostic by design; no explicit x86/ARM support matrix is documented
* [ ] Ensure runtime feature detection where appropriate — the existing detection (`build.rs`) is a compile-time nightly-vs-stable toolchain check, not a runtime CPU-feature (AVX2/NEON/etc.) check
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

* [ ] no allocation — `tpt-dsp/ARCHITECTURE.md` §3.2 documents real caveats (async adapters allocate a `Vec` per frame; `AudioGraph` allocates at construction), so this is true in steady state for most paths but not unconditionally
* [ ] no blocking
* [ ] no I/O
* [ ] no unbounded work — `tpt-dsp/ARCHITECTURE.md` §3.2 documents that `OutlierDetector::push` is O(n log n) per sample, not O(1)
* [x] caller-owned memory — the dominant idiom across both `tpt-dsp` API patterns (free functions on slices; structs with pre-allocated scratch), per `ARCHITECTURE.md` §3.1
* [ ] deterministic execution bounds where possible

## Offline

* [ ] dynamic allocation permitted
* [ ] larger scratch buffers permitted
* [ ] parallel processing permitted
* [ ] expensive RDO permitted

Document which DSP APIs satisfy which profile. — partially satisfied: `tpt-dsp/ARCHITECTURE.md` §3 ("The real-time-safe / zero-allocation contract") documents per-API allocation behavior and known caveats, but doesn't use this section's Real-time/Offline profile framing explicitly.

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
* [ ] Define overflow behavior — buggy: `rescale()` multiplies by `self.timebase.num` twice instead of `target.num`; `rescale_overflow_returns_none` fails
* [ ] Define negative timestamp handling — `Add`/`Sub` silently fall back to `unwrap_or(self.ticks)` on overflow instead of saturating/erroring
* [ ] Define discontinuities
* [ ] Define clock domains
* [ ] Define synchronization model
* [ ] Add audio/video sync tests
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
* [ ] Rice coding — actually implements Exp-Golomb (`write_exp_golomb`/`read_exp_golomb`); doc comment incorrectly claims Rice coding via `tpt-media-bitstream`, see Known Issues
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

One thing I particularly like about this arrangement is that if you later decide to build something like RF codecs, scientific signal codecs, telemetry compression, or robotics signal processing, the DSP work we've done for media isn't trapped inside tpt-media; it becomes part of the reusable TPT foundation.