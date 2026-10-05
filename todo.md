# TPT Media — Master Implementation TODO

**Repository:** `tpt-media`
**Organization:** TPT Solutions
**Language:** Rust
**Status:** Early Implementation
**Document version:** 2.3
**Last updated:** 2026-09-29

---

## Known issues (as of 2026-09-07)

* **2026-09-14 — Cinder has a structural ≈3 dB SNR ceiling on every
  signal** (found by the new §36/§37 corpus quality tests, not by
  review). The encoder MDCTs *non-overlapping* 2N-sample windows (input
  hop = 2N) and the decoder emits a windowed IMDCT per packet with no
  overlap-add, so the TDAC reconstruction that a Princen–Bradley
  sine-window MDCT requires never happens: half of each window's
  samples time-alias away and every input — silence-except-edges, tone,
  noise, clicks — decodes at ≈1.5–3 dB SNR regardless of quantization
  quality (measured: sine 3.01, square 3.03, sweep 2.93, noise 2.95,
  clicks 1.59, mixed 3.00, DC 2.67, alternating 2.69 dB). This is not
  fixable decoder-side (the edge regions of each window exist only in
  that window's own coefficients); a real fix means re-establishing hop-N
  analysis (2× packet rate) plus decoder OLA and reworking the
  frame/packet semantics and the tests that pin them (`§29`'s tests
  currently assert 2N-sample frames in → one packet → 2N samples out).
  Recorded as a known defect with the §37 corpus floors pinned at
  measured values as regression guards; the redesign is its own future
  change, not something to slip in unnoticed.
* **2026-09-14 — `RangeDecoder` desynchronized from the encoder for any
  `freq > 1` symbol — found and fixed by the new §38 property tests.**
  `decode(total)` narrowed by `range/total` regardless of the decoded
  symbol's frequency, while `encode(cum, freq, total)` narrows by
  `(range/total)·freq`; every non-uniform model diverged after the
  first symbol. Uncaught because every pre-existing range-coder test
  used `freq = 1` and the §38 fuzz sweep only fed corrupted bytes
  (never an encode→decode *pair*). Fixed with the standard two-phase
  API (`decode_peek(total)` + `update(cum, freq, total)`), with
  `decode(total)` retained for the uniform `freq = 1` case it got
  right. No production consumer existed (Aura's lossless path is
  Exp-Golomb), so this is not a wire-format change. See
  `CHANGELOG.md [Unreleased] / Fixed`.
* **2026-09-14 — `tpt-cli`'s JSON writer emitted invalid JSON for every
  probe with tracks** (found while adding `benchmark --json`). Three
  related defects: `object()`/`arr()` double-emitted the field
  separator (`"key":,{...}`), `close()` picked its bracket from the
  *parent* container (an object inside an array closed with `]`), and
  the track-list serializers never opened the array they populated
  (`"audio":,{...}`, empty lists as `"video":,`). All fixed with
  exact-shape regression tests; `probe --json` output changed (for the
  better) — anything that parsed the old output was parsing garbage.
* **2026-09-14 — probe's first/last PTS envelope is computed on
  *unsigned* ticks** (found by the §47 malformed-timestamp audit, kept
  as documented behavior): `Acc` stores `u64`, so a stream mixing
  negative and positive timestamps reports its min/max envelope in
  unsigned order (a `-1` tick PTS sorts above every positive one).
  Overflow-safe and panic-free either way — for such hostile inputs the
  duration/bitrate numbers are absurd-but-finite, which is considered
  an acceptable answer to nonsense. If real mixed-sign timestamps ever
  matter, switching `Acc` to `i64` with `saturating_sub` in the
  duration math is the fix.
* **2026-09-28 — probe's audio track line printed raw ticks under a
  `{:.3} s` label** (found by smoke-testing the new Reed CLI path, not
  by review — the codec was innocent; the display bug affected every
  `.tpt` file with a non-1/1 track timebase, i.e. everything `tpt
  encode` produces): the human-readable `Audio #0: ...` line subtracted
  first/last PTS ticks and printed them as if they were seconds (a
  1-second 16 kHz file reported "15680.000 s"), while the container
  `Duration:` line above it was correct and `probe --json` was already
  correct (it emits ticks *and* timebase explicitly). Fixed by
  converting the envelope through the track timebase in the `Display`
  impl; the existing probe tests still pass (they never asserted the
  seconds line, which is why this survived).

* **2026-09-29 — the declared MSRV was never true** (found by
  *testing it* for the first time, while establishing the §51
  policies): `rust-version = "1.75"` dated from the workspace's
  creation and had never been verified against the real dependency
  tree. The committed `Cargo.lock` cannot even be *parsed* by Cargo
  1.75 — `clap_builder` 4.6 ships an edition-2024 manifest — and 1.85
  falls one minor short (`criterion` 0.8.2 declares rustc 1.86).
  1.86 builds the whole workspace clean (`cargo check --workspace
  --locked --all-targets`). Corrected `rust-version` to 1.86, added an
  `msrv` job to CI so the claim is enforced rather than presumed, and
  wrote the bump policy (CONTRIBUTING §9.3). The lesson is the usual
  one for this list: a `rust-version` nobody tests is a plausible
  falsehood, and plausible falsehoods survive every review.

* **2026-09-29 — `tpt-media-probe` had never actually been built for
  its own no_std target** (found by testing §1's "WASM-compatible core
  components" instead of assuming them): the crate carries the
  `cfg_attr` no_std switch and was assumed compatible, but the first
  real `cargo check --target wasm32-unknown-unknown
  --no-default-features` failed with six errors — `format!` and
  `ToString` were reachable only through the std prelude, so every
  std build masked the missing `alloc` imports. Two bytes of imports
  fixed it, and a new CI `wasm` job now keeps all six foundation
  crates compiling for wasm32 so the claim stays tested rather than
  presumed. Same-day sibling finding: two latent clippy failures (a
  hand-rolled no-op waker in `tpt-media-io`'s tests, a `map_or`
  clippy 1.97 now wants as `is_none_or`) had been hiding behind
  cached check results and surfaced only when the caches were
  invalidated — a green `cargo clippy` is only as fresh as its cache,
  which is exactly why CI re-runs it from scratch.

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
  `tpt-media-sys`, and `tpt-video-{thrum,vault,mote}`, replaced
  with an honest note that they're target structure pending
  implementation, and added a `### Fixed` section summarising the
  `Timestamp::rescale` and Aura doc-comment corrections.
* `cargo test --workspace` currently has 0 failing test binaries
  (the `rescale` bug above was the last one — `tpt-media-core`'s
  timebase module and `tpt-media-bitstream` both pass in full).
* **2026-09-07 tpt-dsp reconciliation**: verified §3–§14 against the
  sibling `tpt-dsp` repo's actual code (`tpt-dsp-core/src/*.rs`). MDCT
  (§5.1) and LPC (§6) are genuinely implemented and consumed by
  Cinder/Aura — flipped below. Pitch analysis (§7), VQ (§8), LSP/LSF
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
  `tpt-dsp-core` that affects Cinder/Aura directly.** Implementing
  `FastMdctPlan`/`FastDctIvPlan` (FFT-backed, see §5.1/§5.2 below) and
  cross-checking them against the existing direct `mdct`/`imdct`/`dct_iv`
  turned up a real disagreement at N=1024+ (Cinder/Aura's actual block
  sizes) — not a bug in the new fast path, but in the **existing** direct
  reference: it evaluated `cos()` at the raw, unreduced `(π/N)·x·y` angle,
  which reaches thousands of radians at these block sizes and is poorly
  conditioned for `f32`/`f64` `cos()`. Fixed in `tpt-dsp` via an
  integer-domain angle-reduction helper (`complex::cos_pi_over_n`) applied
  to all five direct transforms (`dct_ii`/`iii`/`iv`, `mdct`/`imdct`);
  `tpt-media`'s Cinder/Aura tests still pass against the corrected
  reference. This means Cinder/Aura at 1024+ block sizes were silently
  producing a measurably wrong spectrum before this fix — worth a mental
  note if `tpt-dsp`'s version isn't yet bumped/pulled everywhere this
  workspace's `Cargo.lock` might have cached the old code.
* **2026-09-11 — a real bug in `tpt-media-bitstream`'s `BitReader::skip`,
  caught by a new Cinder test, not by review.** Skipping a non-byte-aligned
  span of bits that crossed multiple bytes computed the right number of
  bytes to advance past, but then re-fetched the *following* byte (one
  past the skip region) to recover the last skipped byte's unconsumed
  trailing bits, instead of the correct byte — silently corrupting every
  read after the first multi-byte `skip` in a stream. It went unnoticed
  because the only production caller (Aura's `read_exp_golomb`) only ever
  calls `skip(1)`, which never takes the multi-byte code path, and the
  crate's own pre-existing `skip_across_bytes` test happened to skip
  exactly the number of bits already sitting in the accumulator, also
  never exercising the buggy path. Found while adding a Cinder test that
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
  `tpt-audio-cinder`'s decoder read `n_bands` and each band's `n_coeffs`
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

Audited 2026-09-29 against the code, with the same evidence-first
convention as the rest of this document.

* [x] Rust-first — the entire ecosystem (thirteen workspace crates plus
      the sibling `tpt-dsp`) is Rust; no C/C++ implementation code
      exists anywhere
* [x] Safe Rust wherever practical — every library crate now carries
      `#![forbid(unsafe_code)]` (2026-09-29), so the claim is enforced
      by the compiler rather than asserted; the only `unsafe` in the
      workspace is `tpt-cli`'s `memprobe` `GlobalAlloc` (a measurement
      harness, and apps are std-by-design) plus the per-codec tests'
      counting allocators (test code, outside the forbidden libs)
* [x] `unsafe` isolated and documented — with the forbid pass above,
      isolation is compiler-checked: the single non-test site is the
      documented `memprobe` module, whose docs state what is measured
      and what the harness cannot see
* [x] `no_std` capability for appropriate components — eight crates
      carry the `cfg_attr` no_std switch (core, bitstream, codec,
      pipeline, io, probe, Reed, Thrum); Reed is CI-verified on
      `thumbv7em-none-eabi`; CONTRIBUTING §10 records the convention
      and the honest exceptions (`tpt-container`, Cinder, Aura are
      std-only today despite their feature scaffolding)
* [x] deterministic behavior where practical — §39: decode, muxing,
      and scalar DSP are pinned deterministic by tests; encoding is
      fully deterministic; the SIMD caveat is build-time, not runtime
* [ ] zero allocation in real-time hot paths — deliberately *not*
      claimed: the tested reality is *bounded* allocation (§42's
      `realtime_alloc` + `memory_budget` suites), and the strict
      reading waits on caller-buffer APIs (the matching open item in
      §54)
* [x] caller-owned buffers where practical — `tpt-dsp` transforms and
      filters take caller output slices; `BitWriter`/`BitReader`
      operate on caller-provided byte buffers; decoders return frames
      rather than filling caller state; resource limits are checked
      *before* an allocation is attempted (§19)
* [x] bounded resource usage — §19's frame/packet/metadata/allocation/
      reference-frame limits are enforced in the decoders, hardened by
      the §47 audits (three real overflow/OOB/bomb bugs found and
      fixed), and quantified per codec by §42's measured budgets
* [x] explicit numerical contracts — `tpt-dsp` §12: every algorithm
      family documents convention, normalization, precision, valid
      sizes, and failure behavior
* [x] fuzz-resistant parsing — §38's deterministic
      corruption/truncation/bit-flip sweeps cover bitstream, container,
      codecs, and probe, and never panic; the container parses
      incrementally and tolerates a partial trailing chunk (§25)
* [x] corruption-tolerant decoding where practical — unknown/reserved
      container chunks are skipped, missing trailers degrade to
      scan-based reads, indexes rebuild, and both PLC-capable codecs
      conceal (§29/§31/§32); the deliberate boundary: unknown
      *bitstream* versions fail fast rather than guess (§35)
* [x] forward-compatible formats — the container is
      forward-compatible by mechanism (reserved chunk ranges,
      unknown-chunk skipping, versioned index, reserved header bytes);
      codec bitstreams are versioned but deliberately fail-fast, not
      forward-compatible — recorded as §35's honest gap rather than
      claimed here
* [x] clean architectural boundaries — the §52 graph is acyclic and
      cargo-enforced; the tpt-dsp dependency direction is verified
      (§3.1); §54's boundary criteria all pass
* [x] reusable crates — every crate is consumable standalone (no_std
      foundations, no app concepts below `apps/`, no codec logic in
      shared crates); the stronger "reused outside tpt-media by
      external projects" is §54's open item — a matter of actual
      external users, not structure
* [x] WASM-compatible core components — verified 2026-09-29: all six
      no_std foundation crates compile for `wasm32-unknown-unknown`
      with std off (probe had never actually been built for the
      target and was missing `alloc` imports — fixed), and a new CI
      `wasm` job keeps that verified; §45's `tpt-web` was built on
      top of them on 2026-09-30, and the *components* claim is tested fact
* [ ] stable FFI boundary — the ABI now *exists* (§46 v0.1, 2026-09-29)
      with a stated stability rule, but "stable" is the 1.0 gate (§53's
      "FFI stability"): the version holds until a signature, error-code
      meaning, or struct layout forces a bump, and the no-surprises
      commitment is made at 1.0 alongside the stable codec specs
* [x] reproducible benchmarks — §41's configuration is fixed in code
      so plain `cargo bench` reproduces it; ownership and the
      record-the-numbers rule live in CONTRIBUTING §11
* [x] provenance tracking — §48/§49: complete per-codec records for
      all four implemented codecs, maintained in the same change as
      the algorithm
* [x] patent-conscious design — GREEN-only classification with dated
      reviews, CONTRIBUTING §3's avoided-technique list, and the
      "royalty-free design objective" terminology rule followed
      throughout (no absolute patent-free claims anywhere)

Avoid — all verified 2026-09-29:

* [x] unnecessary dependencies — thiserror and bitflags in the
      foundations, clap only in the CLI, criterion only in dev-deps,
      libm only where no_std float math requires it; no serde, no
      external fuzz harness, no allocator crates
* [x] hidden heap allocation — allocation is visible and measured:
      caller buffers where practical, Thrum's single reused scratch
      buffer, and §42's byte-level working-set tests catching any
      hidden growth by assertion rather than by review
* [x] global mutable state — no library crate has mutable global
      program state; the only interior-mutability statics are the
      memprobe counters (process-scoped measurement instrumentation,
      documented) and test allocators
* [x] codec-specific DSP duplicated across codecs — §54's criterion:
      transforms, masking, LPC, and noise shaping are consumed from
      `tpt-dsp`; the codecs contain codec logic only
* [x] application-specific logic in core crates — `tpt-media-core`
      has no CLI, app, or I/O concepts; those live in `apps/` and
      `tpt-media-io`
* [x] undocumented numerical assumptions — §12's contracts, plus the
      wire formats now specified field-by-field (§54)
* [x] claims of absolute patent-free status — every provenance record
      and document carries the jurisdictional disclaimer; the
      "patent-conscious / royalty-free design objective" wording is
      used consistently

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
│   ├── cinder/
│   ├── aura/
│   ├── reed/
│   ├── thrum/
│   ├── vault/
│   └── mote/
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
* [x] Prevent codec-specific APIs from leaking into `tpt-dsp` — `tpt-dsp-core`'s public API (`fft`, `dct_*`, `mdct`/`imdct`, `lpc::*`, `Biquad`, `RingBuffer`, ...) is entirely generic; no Cinder/Aura/Reed-specific types

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

Required by Cinder and Aura.

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
* [x] Define performance targets — **target (2026-09-07): the MDCT/DCT-IV transform (forward on encode, inverse on decode) must consume ≤10% of the codec's own real-time frame budget, `frame_size / sample_rate`, at its default operating point** — leaving ≥90% headroom for windowing, quantization, entropy coding, and system jitter. This is derived from Cinder's own committed real-time contract (§29, §1 "zero allocation in real-time hot paths" / deterministic execution), not an arbitrary number; it only binds codecs that claim real-time (Cinder — §30's Aura is explicitly "high-fidelity/archival", i.e. the Offline profile in §14, so this target doesn't apply to it).
  Measured against `tpt-dsp-core/benches/dsp_bench.rs::bench_mdct` (2026-09-07, `--quick`, noisy per `tpt-dsp/BENCHMARKS.md`'s own caveat):
  | Cinder `frame_size` (N) | frame budget (48kHz) | direct `mdct()` (what Cinder actually calls) | vs. 10% budget | `FastMdctPlan` |
  | ---: | ---: | ---: | :--- | ---: |
  | 128 | 2.667 ms | 214 µs | 8.0% — **passes**, thinly | 0.34 µs (0.01%) |
  | 256 | 5.333 ms | 822 µs | 15.4% — **fails** | 0.69 µs (0.01%) |
  So Cinder's *previous* code called the direct `mdct`/`imdct`, which already missed this target at its larger supported frame size, and only barely cleared it at the smaller one, before accounting for anything else in the encode/decode path. **Follow-up done 2026-09-08:** `codecs/cinder/src/lib.rs` now uses `FastMdctPlan` — `CinderEncoder` holds one plan sized to its fixed `frame_size`; `CinderDecoder` holds one plan per supported size (128 and 256, selected per-packet via `mdct_plan_for`, since the wire format self-describes frame size independent of the decoder's own configured default) so a size-128 packet decoded on a size-256-configured decoder still inverts correctly. Plans are built once at construction (the one-time FFT-plan/twiddle allocation) and only `forward`/`inverse` run in the hot path, which allocate nothing. All 6 existing `tpt-audio-cinder` tests still pass unchanged. `FastMdctPlan` clears the ≤10% target by ~3 orders of magnitude at both sizes, so the codec now meets its own real-time claim in practice, not just in the benchmark suite.
* [x] Implement optimized DCT-IV — `FastDctIvPlan` (added 2026-09-07): same FFT/chirp technique as `FastMdctPlan`, specialized to DCT-IV's `θ(m,k)=(π/N)(m+½)(k+½)` (zero-padded to `2N` since both indices range only `0..N`, unlike MDCT)
* [x] Preserve reference implementation for verification — the direct `dct_iv` stays; `FastDctIvPlan` is checked against it, not a replacement
* [x] Add scalar/SIMD equivalence tests — `fast_dct_iv_matches_reference` (direct `dct_iv` vs `FastDctIvPlan` across 4 sizes) and `fast_dct_iv_is_self_inverse_up_to_scale`; `FastDctIvPlan` is `FftPlan`/RustFFT-backed (same SIMD-dispatch rationale as `FastMdctPlan`)
* [x] Verify numerical error bounds — `dct_ii_of_constant_is_scaled`, `dct_ii_then_iii_roundtrips`, `dct_iv_orthogonality_roundtrip` all assert explicit tolerances; also fixed a real precision bug this uncovered — see Known Issues

---

## 5.3 2D transforms

Required primarily by video codecs.

_Implemented 2026-09-30+ in the `tpt-dsp` repository (this section
always allowed that — see §4's preamble), audited here 2026-10-05
against `tpt-dsp-core/src/dct.rs` at its current HEAD and against the
`thrum`/`vault` crates that consume it. A 2026-09-07 note here said no
2D transform code existed anywhere; that was true then and is obsolete
now — kept for the audit trail rather than deleted._

* [x] Define 2D DCT API — `tpt-dsp-core::dct::dct_2d`/`idct_2d`:
      explicit transform formula in the doc comment, in-place over a
      row-major block with caller-owned scratch (`2·max(rows, cols)`),
      length-generic over any `rows, cols ≥ 1`, `# Panics` sections in
      the §12 contract style
* [x] Implement separable 2D DCT — `dct_2d`: rows through 1D
      `dct_ii`, then columns (JPEG-style composition)
* [x] Implement inverse 2D DCT — `idct_2d`: columns through `dct_iii`
      then rows; the pair is unnormalized — round trip recovers
      `(rows·cols/4)·x`, documented and pinned by
      `dct_2d_round_trips_up_to_scale`
* [x] Support 4×4 — the kernels are length-generic, and 4×4 is a real
      consumer size: Vault's partitioner codes 4×4 chroma tiles up to
      32×32 luma (`codecs/vault/src/transform.rs`), so every size in
      this list is exercised by a running codec, not just tests
* [x] Support 8×8 — tested (8×8 in both `dct_2d` tests), benched
      (`bench_dct_2d::forward_f32/8x8`), and Thrum's coded block size
* [x] Support 16×16 — tested and benched at 16×16
* [x] Benchmark — `tpt-dsp-core/benches/dsp_bench.rs::bench_dct_2d`,
      forward + inverse at 8×8 and 16×16 with element throughput;
      results in `tpt-dsp`'s `BENCHMARKS.md`
* [ ] SIMD optimize common block sizes — deliberately not done: the 1D
      kernels are direct scalar O(N²) sums, which is already cheap at
      these sizes (an 8×8 block is 64 coefficients), and the codecs'
      encode cost is dominated by motion search and RDO rather than
      the transform. Revisit only if a profile says otherwise
* [x] Verify against 1D decomposition —
      `dct_2d_matches_naive_double_sum`: the separable implementation
      agrees with an independently written double-sum reference at
      8×8, 16×16, and a non-square 4×16 (proving row/column
      genericity), with relative tolerances

---

# 6. TPT-DSP — LPC

Required by Reed and Aura.

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

Required primarily by Reed and potentially useful elsewhere.

* [x] Define pitch-analysis API — `tpt-dsp-core/src/pitch.rs`: `PitchEstimate<F>{period_samples: Option<usize>, confidence, voiced}` + `frequency_hz()` helper
* [x] Implement autocorrelation pitch estimator — `autocorrelation_pitch`, normalized `Σx[n]x[n+lag]/Σx[n]²`, argmax over the search range. Biases the argmax toward the *shortest* near-tied lag (an absolute margin) to avoid the classic "octave error": a signal periodic at N is, by construction, also periodic at 2N/3N/..., and a naive global argmax can pick a harmonic instead of the fundamental on floating-point noise alone — this was caught by the test below actually returning the wrong (doubled) period before the fix
* [x] Implement AMDF estimator — `amdf_pitch`, `mean(|x[n]-x[n+lag]|)`, argmin over the search range. Same octave-error concern and fix, but scaled to the AMDF range observed so far rather than a fixed margin, since AMDF's "good" values cluster near zero rather than near a bounded ceiling
* [x] Define pitch period range — `hz_range_to_period_samples(sample_rate, min_f0_hz, max_f0_hz)` + `SPEECH_MIN_F0_HZ`/`SPEECH_MAX_F0_HZ` (50-500Hz) constants
* [x] Define voiced/unvoiced detection — threshold on `confidence` against a caller-supplied `voiced_threshold` (`DEFAULT_VOICED_THRESHOLD = 0.3`, literature-typical); `period_samples` is `None` when unvoiced so callers can't accidentally use a meaningless lag
* [x] Add confidence metric — autocorrelation: normalized peak value; AMDF: `1 - min/max` of the AMDF values seen across the search range
* [x] Add tests using synthetic signals — `autocorrelation_pitch_recovers_known_period`/`amdf_pitch_recovers_known_period` (known-frequency sine), `silence_is_unvoiced_for_both_estimators`, `hash_noise_is_unvoiced_for_both_estimators` (SplitMix64 PRNG — a simpler per-index hash was tried first and rejected because it still had short-range structure `autocorrelation_pitch` could lock onto, a real false-positive risk worth documenting)
* [ ] Add speech corpus tests — no real speech corpus is available in this environment; only synthetic signals are tested. Left honestly unchecked rather than substituting a synthetic proxy for what this item actually asks
* [x] Benchmark — `tpt-dsp-core/benches/dsp_bench.rs::bench_pitch`, both estimators, at a Reed-realistic 16kHz/40ms frame over the full speech F0 range; ~53µs each (0.13% of the frame budget) — see `BENCHMARKS.md`

---

# 8. TPT-DSP — Vector Quantisation

Required by Reed and Mote.

* [x] Define generic VQ API — `tpt-dsp-core/src/vq.rs`: flat-codebook convention (`entries*dim`), `DistanceMetric` enum, free functions + `VqCodebook` owning wrapper (`alloc`-gated)
* [x] Implement nearest-vector search — `nearest_vector`, linear scan, deterministic lowest-index tie-break
* [x] Implement distance metrics — `DistanceMetric::{SquaredEuclidean, Manhattan}`; `metrics_can_disagree_on_ranking` test proves they're genuinely distinct, not accidentally identical
* [x] Implement fixed-size codebooks — flat `&[F]` convention (`codebook.len() == entries*dim`) for the free functions; `VqCodebook` for owned storage
* [x] Implement encoder — `vq_encode`
* [x] Implement decoder — `vq_decode`
* [x] Support caller-owned buffers — all free functions take caller-provided output slices, no internal allocation
* [x] Add deterministic behavior — `ties_break_toward_lowest_index_and_are_deterministic` (exact-tie input, repeated calls agree)
* [x] Add optional accelerated search — `nearest_vector_accelerated`: early-exit once a codeword's partial distance sum can't beat the current best (both metrics accumulate non-negative terms, so this is always safe). Proven bit-identical to the linear scan (`accelerated_matches_linear_search`, 2 metrics × 20 random queries against a 40-entry codebook), not just "usually agrees" — ~3-3.6× faster at Reed's/Mote's target codebook sizes (data-dependent, see `BENCHMARKS.md`)
* [x] Benchmark linear search — `tpt-dsp-core/benches/dsp_bench.rs::bench_vq`, linear vs accelerated, at (1024,10) and (4096,16) — Reed's/Mote's own target codebook sizes (§31/§34); results in `BENCHMARKS.md`
* [ ] Investigate tree/product quantisation for larger codebooks — intentionally not implemented: no concrete consumer (Mote doesn't exist yet) to validate requirements against, same reasoning as holding off on §5.3's 2D transforms
      _Update 2026-09-30: Mote (§34) is now that consumer, and it uses the
      accelerated linear scan at 4096 entries (key frame 27-38 ms at
      320x240 mono, ~200 ms at 640x480, shared-machine timings). Tree or
      product quantisation stays unimplemented and unchecked; it is the
      candidate if key-frame encode cost ever matters._

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

* [x] maximum frame dimensions — Cinder/Aura decode paths now call `limits.check_frame_bytes(...)` before producing output (2026-09-09)
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
* [x] maximum allocation — Cinder (`check_allocation` on the decoded spectrum buffer) and Aura (lossless coefficient/sample buffers, lossy MDCT spec+synthesis buffers) now call `limits.check_allocation(...)` before allocating; both have tests asserting a tight budget yields `ResourceLimit(Allocation)` on a structurally valid packet (2026-09-09)
* [x] maximum reference frames — 2026-09-28: `ThrumDecoder` keeps
  exactly one reference picture
  (`ThrumDecoder::REFERENCE_FRAMES = 1`) and calls
  `limits.check_reference_frames(...)` on every packet, so the knob is
  now exercised by a real decoder rather than being aspirational
* [x] maximum recursion depth — 2026-09-30: Vault's partition decode is the
  first recursive codec path. Its depth is structurally bounded (64→32→16→8,
  `MAX_PARTITION_DEPTH = 3`; a node below 8×8 cannot exist), and
  `VaultDecoder::decode` calls `limits.check_recursion_depth(4)` on every
  packet, so a caller who sets `max_recursion_depth` below 4 gets
  `ResourceLimit(RecursionDepth)` (`resource_limits_are_honoured`). No other
  codec has a recursive decode path
* [ ] maximum decode work where practical — `max_decode_micros` still uncalled (would need a measured loop budget). Thrum's per-packet work is bounded structurally (one pass over a fixed macroblock grid, each block's entropy walk capped at 64 positions), so there is no loop budget to measure — the bound is the frame geometry, which `max_frame_bytes` already covers
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

# 29. Cinder

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
  `CinderEncoderConfig::perceptual_weighting` (default `false`, so every
  existing caller keeps its exact previous behavior). Unlike Aura's full
  simultaneous-masking model (§10), Cinder deliberately uses only the
  cheapest piece of that toolkit — the absolute threshold of hearing
  (`tpt-dsp-core::threshold_in_quiet`) — evaluated once per band at
  construction time (not per frame, since it depends only on the fixed
  band layout), consistent with Cinder's low-latency/low-complexity
  design goal. Each band's configured step is scaled by that band's
  threshold relative to a 1kHz reference; the scaling only ever coarsens
  a step, never sharpens below the configured value, since the wire
  format's step field is an integer with no sub-configured granularity.
  No wire-format change (Cinder already transmits the actual per-band
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
* [ ] define modes — Cinder has no distinct "modes" concept
* [x] define quantisation
* [x] implement encoder
* [x] implement decoder
* [x] implement PLC
* [x] test packet loss — `plc_conceals_mid_stream_packet_loss` (2026-09-09): encodes a 4-frame tone, drops the middle packet, substitutes `conceal()` at its slot, and asserts the concealed frame carries the right sample count/PTS and non-zero energy (realised from the last decoded spectrum), with clean decode before and after the gap
* [x] test jitter — `jittered_pts_are_preserved_not_reordered` (2026-09-09): delivers valid packets in arrival order 0,2,3,1; the decoder must not panic/reorder on its own and each output frame echoes its own packet's PTS with full-length, non-silent output
* [x] benchmark latency — 2026-09-14: `tpt benchmark` (§43) reports median/min encode+decode wall time and real-time factor per input, and `codecs/cinder/benches/codec_bench.rs` (criterion) pins the per-block cost; release numbers in `docs/performance.md` (encode ≈ 9.4 µs, decode ≈ 10.6 µs per 512-sample block ≈ 3 orders of magnitude inside the real-time budget)
* [x] benchmark CPU usage — same two tools: the criterion bench is the CPU-cost source (`docs/performance.md`), and `codecs/cinder/tests/realtime_alloc.rs` (§42) additionally pins the hot path's allocation behavior, the other half of the real-time cost model

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

# 31. Reed

**Purpose:** extremely low-bandwidth speech.

Initial target:

```text
8 kHz
16 kHz
mono
```

* [x] LPC analysis — `tpt-dsp-core`'s autocorrelation + Levinson-Durbin
      over a Hamming-windowed 20 ms frame (order 2..=16, default 10),
      same §6 primitives Aura lossless uses
* [x] pitch estimation — `tpt-dsp-core`'s normalized autocorrelation
      estimator (§7) over the speech F0 range, 10-bit period on the
      wire (50 Hz–1 kHz searchable at either rate)
* [x] voiced/unvoiced classification — the same estimator's
      confidence-thresholded `voiced` flag (1 bit on the wire)
* [x] excitation model — voiced: imcinder train at the transmitted
      period (classic LPC-10-style, amplitude set so its RMS matches
      the quantized residual RMS); unvoiced: one 1024-entry stochastic
      codebook shape scaled by the same gain. Gain (residual RMS) is
      quantized in 1 dB steps over 62 dB with index 0 as exact mute, so
      digital silence round-trips to digital silence
* [x] VQ — shape-gain VQ for the unvoiced excitation over the §31
      1024-entry codebook target. The codebook is *never transmitted*:
      both sides derive identical unit-RMS entries from a SplitMix64
      stream seeded by the index (the CELP fixed-codebook idea, with
      the training corpus problem sidestepped entirely — the entry
      distribution is defined by construction, not fitted to data)
* [x] LSP/LSF if beneficial — beneficial and load-bearing: the
      spectral envelope is transmitted as uniformly quantized LSP
      indices (`lpc_to_lsp` → round → `lsp_to_lpc`), with the config
      enforcing `2^lsf_bits > lpc_order` (a strictly-increasing index
      set must exist) and the decoder re-enforcing ordering before
      `lsp_is_stable`, so no bit stream can synthesize through an
      unstable filter
* [ ] fixed-point implementation — deliberately not attempted: the
      implementation is `f64` (analysis) / `f64` (synthesis), `no_std`
      via `libm` on `thumbv7em-none-eabi`. A Q-format port is its own
      change (the crate description previously said "fixed-point"
      aspirationally; fixed to match the code)
* [x] `no_std` support — `alloc`-only build for
      `thumbv7em-none-eabi` is the existing CI job and now builds with
      the real codec in it (float math via `libm`, matching
      `tpt-media-core`)
* [x] encoder — `ReedEncoder` (§41 bench: ~266 µs/frame median over a
      half-voiced/half-noise input; dominated by the unvoiced codebook
      search, and deliberately *not* claimed real-time-safe)
* [x] decoder — `ReedDecoder` (~5.9 µs/frame; `real_time_decoder` is
      claimed and pinned by `codecs/reed/tests/realtime_alloc.rs`)
* [x] packet-loss concealment — `conceal()` repeats the last good
      envelope/voicing/pitch with gain decayed 6 dB per concealed
      frame, fading gaps to exact silence; first-ever concealment
      (no prior state) is silence
* [x] deterministic decoding — pinned byte-for-byte across independent
      instances (`encoding_and_decoding_are_deterministic` and the §39
      property test); the SplitMix64 codebook is pure integer math
* [x] embedded benchmark — `codecs/reed/benches/codec_bench.rs`
      (criterion, 64×320-sample frames @ 16 kHz); results in
      `docs/performance.md`. Decode at ~3400× realtime is comfortably
      inside an embedded budget; encode's search is the part a
      fixed-point/embedded port would have to budget for

Initial codebook target:

```text
1024 entries
```

---

# 32. Thrum

**Purpose:** low-latency video communication.

Design:

```text
I/P coding
intra refresh
simple prediction
low-complexity transforms
```

2026-09-28: `tpt-video-thrum` is a working implementation of all of
the below. Design notes in `docs/codec-design.md`; measured numbers in
`docs/performance.md`; algorithm sources in `docs/provenance.md`.

* [x] define pixel formats — `Gray8` and planar 4:2:0 `Yuv420` (the two
      coded formats; anything else is refused with
      `CodecError::Unsupported` rather than converted). Frames must be
      non-zero multiples of 16 so the picture is exactly
      macroblock-aligned — no padding, no partial macroblocks
* [x] define colour model — BT.601-family luma/chroma, stored full-range
      8-bit unsigned; 4:2:0 means one chroma sample spans two luma
      samples, so chroma motion vectors are the luma vector halved
      (`chroma_mv`), recomputed on both sides rather than signalled
* [x] define frame structure — a macroblock is 16x16 luma = four 8x8
      luma blocks, plus one 8x8 U and one 8x8 V block for 4:2:0;
      macroblocks are coded in raster order and one frame is exactly one
      packet (`END_OF_FRAME` on every packet, no reordering buffer)
* [x] define I-frame — every macroblock intra; each 8x8 luma block
      carries a 2-bit mode (DC / vertical / horizontal / diagonal)
      chosen by SAD, chroma always DC; prediction is built from the
      *reconstructed* neighbours, so encoder and decoder cannot drift
* [x] define P-frame — 1-bit intra flag per macroblock (the intra-refresh
      cursor), then either skip (1 bit, copy the reference at `(0, 0)`)
      or a 6-bit biased motion vector per axis plus the residual blocks
* [x] implement intra prediction — the four modes above;
      `intra_mode_available` gates them on geometry and the decoder
      re-checks, so an unavailable mode in a hostile packet degrades to
      DC instead of reading out of bounds
* [x] implement inter prediction — full-pixel integer motion
      compensation from a single reference frame, edge-clamped on both
      sides
* [x] motion estimation — exhaustive integer search over the whole
      `±search_range` window
* [x] full-pixel SAD — the search cost is the sum of absolute luma
      differences, with an exact early exit once the running total
      reaches the best found so far (the SAD is monotonically
      increasing, so the exit is not a heuristic)
* [x] DCT — 8x8 separable DCT-II/DCT-III from `tpt-dsp-core`'s
      `dct_2d`/`idct_2d` (todo.md §3.1: transforms live in `tpt-dsp`,
      the codec consumes them)
* [x] quantisation — one flat dead-zone uniform quantiser; the step
      comes from a 6-bit `qp` sent in every frame header, via a fixed
      table (`quant_step`) so no float `pow` is needed and the step is
      bit-identical on every target
* [x] entropy coding — real Rice coding
      (`tpt-media-bitstream::rice`), a run/level token stream over the
      zigzag scan, with a per-block Rice parameter `k` chosen by the
      encoder from `{0,1,2,3}` to minimise the coded length
* [x] intra refresh — `DEFAULT_INTRA_REFRESH_PERCENT` = 5% of a P-frame's
      macroblocks re-coded intra, on a sliding window that sweeps the
      whole picture in `ceil(total / per_frame)` frames
      (`intra_refresh_sweeps_every_macroblock`). The first version used
      a strided cursor that was only exhaustive when the stride happened
      to be coprime with the macroblock count; with 48 macroblocks and
      9 per frame the stride 6 shares a factor and the sweep silently
      covered 1/6 of the frame
* [x] deblocking — a 4x4-grid loop filter in the classic H.263 shape
      (a `d < alpha` test with a `d < beta` wide path and a
      `tc`-limited narrow path), applied to the whole reconstructed
      frame, with the *filtered* picture becoming the next reference
* [x] decoder — `ThrumDecoder`; one frame out per packet in, no
      reordering, no look-ahead
* [x] encoder — `ThrumEncoder`; I-frame every
      `KEYFRAME_INTERVAL` (30) frames and the first
* [x] latency benchmark — `codecs/thrum/benches/codec_bench.rs`
      (criterion) measures I-frame and P-frame encode and decode
      separately, because the encoder never holds a frame back: the
      encode + decode cost of a single P-frame *is* the one-way latency
      contribution. Results in `docs/performance.md`

Additional work not on the original list, and why:

* [x] versioned bitstream + CRC-32 trailer — §35's family convention
      (16-bit version rejected up front; a 4-byte CRC-32 trailer over
      the payload, verified before parsing). This is the v0.1 wire
      format; there is nothing to be backward compatible *with* yet
* [x] packet-loss concealment — `ThrumDecoder::conceal` repeats the
      last decoded frame (a freeze), which is the right concealment for
      a codec with no temporal hierarchy; mid-grey before the first
      frame
* [x] `DynEncoder`/`DynDecoder` factories — so `LIVE` is reachable
      through `Registry` (§18). A video factory carries the frame
      geometry, because geometry is part of a video codec's
      configuration and `CodecConfig` only carries the track's private
      bytes
* [x] provable packet-size bound — `ThrumConfig::max_packet_bits`
      derives the worst case from `MAX_LEVEL` and `MAX_RICE_BITS` rather
      than estimating it, so the encoder's single allocation can never
      overflow
* [x] no worst-case packet retention — originally each packet carried
      the full `max_packet_bits` capacity (megabytes for a mid-size
      frame) despite holding a few kilobytes (caught by
      `tests/realtime_alloc.rs`); the first fix (`shrink_to_fit`) was
      itself superseded 2026-09-28 when the §42 byte-level measurement
      showed the encoder still *allocated* that buffer every frame —
      it is now allocated once at construction and reused, with each
      packet emitted as an exact-size copy, so the steady-state path
      does not allocate it at all
* [x] rate-aware skip threshold — `skip_threshold(step)`: skip when the
      mean absolute prediction error is below one quantiser step, i.e.
      when the residual would quantise to zero anyway. A fixed SAD
      constant disabled skip entirely at coarse `qp`, exactly where it
      is worth the most
* [x] CLI encode/decode — the CLI carries video through a byte-exact
      YUV4MPEG2 bridge (`apps/tpt-cli/src/y4m.rs`), not through
      `tpt-media-io`: `encode`/`benchmark` take a `.y4m` input with
      `--codec thrum` and encode one packet per frame, `decode` writes a
      video track back out as `.y4m`, `transcode` re-encodes it, and
      `tpt video-corpus` measures the codec over the §36 corpus without
      reading files. The bridge is deliberately narrow: progressive only
      (`Ip`), `C420*`/`Cmono` only, aspect ratio parsed and discarded
      (the `.tpt` track model has no aspect field). A video codec on a
      non-video path — a WAV into `encode`, an audio track into
      `transcode` — still fails with an explicit explanation rather than a
      generic unknown-codec error

Initial intra-refresh target:

```text
~5% of frame per frame
```

---

# 33. Vault

**Purpose:** high-compression video.

2026-09-30: `tpt-video-vault` (`codecs/vault`, codec id `VAUL`) is a
working implementation of the checklist below, as an I/P codec with no
B-frames. Design notes in `docs/codec-design.md`; measured numbers in
`docs/performance.md`; algorithm sources in `docs/provenance.md` (every
row `under-review`: no maintainer patent review has happened yet). 38
tests (15 unit, 18 property, 4 coding-tool census, 1 memory) plus
`tests/measure.rs` (an `#[ignore]`d reproducible measurement harness).

* [x] define superblock structure — raster-order 64×64 superblocks, each
      a quadtree of 1-bit `split` flags and leaves; a node overhanging the
      picture edge splits implicitly with no flag, so any size that is a
      multiple of 8 needs no padding (`docs/codec-design.md`)
* [x] 64×64 top-level blocks — `SUPERBLOCK`; a 64×64 leaf is coded as four
      32×32 transform tiles (the largest transform is 32×32, a stated
      limit, not the block size)
* [x] recursive partitioning — RDO-chosen quadtree, 64→32→16→8. Census
      test `recursive_partitioning_uses_large_blocks_where_flat_and_small_where_busy`
      pins that both large and small leaves actually occur
* [x] minimum 8×8 blocks — `MIN_BLOCK`; chroma of an 8×8 leaf is 4×4
* [x] motion vectors — half-pel units, `|v| ≤ 65`, coded as a difference
      from a left-else-top neighbour predictor (not a median); chroma
      vectors derived, never signalled
* [x] full-pixel search — a shrinking-step 8-neighbour *pattern* search
      seeded from the zero and predicted vectors, **not exhaustive** (it
      can settle in a local minimum), costed by SAD plus a vector-cost term
* [x] half-pixel interpolation — 2-tap bilinear (4-sample on the diagonal)
      per CONTRIBUTING §3; known-answer and edge-clamp tests, and
      `half_pel_motion_is_used_and_predicts_well` shows the search picks
      half-pel vectors on a half-pel shift
* [x] reference frames — up to 16 (`MAX_REFERENCE_FRAMES`), the last
      `max_refs` loop-filtered pictures, per-leaf reference index. Drift-free
      at 1, 2, 3, 5, 8 and 16 references; used and paying off on an A/B/A/B
      cut (`multiple_references_are_used_and_pay_off`). Default is 4 so the
      strict default limits (8) decode it; 16 needs a raised limit. No
      long-term references, reordering or B-frames
* [x] 2D transforms — 4×4…32×32 separable DCT from `tpt-dsp-core`'s
      `dct_2d`/`idct_2d`, quantised in orthonormal units so one `qp` means
      the same step at every size
* [x] quantisation — one flat dead-zone quantiser (rounding offset 0.4)
      on the shared 6-bit `qp` table, level clamp ±8192 derived from the
      transform bound. No per-block `qp`, no coefficient-level RDO
* [x] RDO — explicit `J = D + λR` with `λ = LAMBDA_SCALE·step²` (empirical,
      untuned), `D` = SSE against the decoder's own reconstruction, `R` =
      the exact bit count of the real writer. Chooses partition, skip /
      inter / intra, reference and vector. **A greedy per-node comparison
      over a heuristically pruned candidate set with an early-termination
      rule — no global optimum is claimed.** Tested only as far as
      `partition_freedom_improves_the_rd_objective` (freeing the partition
      is not worse on `J` than forcing 8×8 on two test frames)
* [x] entropy coding — static Rice codes, per-tile parameter, run/level
      over a diagonal scan with `last` as terminator (no context modelling)
* [x] loop filtering — H.263 Annex J deblocking driven by a boundary
      strength, applied only at real coding-structure edges; the filtered
      picture is the next reference. **Not RD-decided** (a per-frame header
      bit): measured to help a textured clip and to be neutral-to-harmful
      on a smooth-gradient clip (`docs/performance.md`)
* [x] decoder — `VaultDecoder`; hostile-input tested (see below)
* [x] encoder — `VaultEncoder`; I-frame every `KEYFRAME_INTERVAL` (60)
      frames, an I-frame clears the reference store
* [x] per-crate documentation — `codecs/vault/README.md` and
      `CHANGELOG.md`, plus its own `keywords`/`categories`, land **in the
      same change** that adds the crate as a workspace member
      (CONTRIBUTING §9.4). The CI `metadata` job fails until they exist,
      so this cannot be forgotten silently. Landed with the member
      entry; `ci/check-metadata.py` passes.

Additional work not on the original list, and why:

* [x] versioned bitstream + CRC-32 trailer — §35's family convention (16-bit
      version rejected up front, CRC verified before parsing)
* [x] provable packet-size bound — `VaultConfig::max_packet_bits` derives
      the worst case from `MAX_LEVEL`/`MAX_RICE_BITS`; the encoder checks
      every packet against it and `packets_never_exceed_the_derived_bound`
      drives the finest quantiser over uniform noise
* [x] hostile-input safety (§38/§47) and resource limits (§19) — the
      corruption sweep repairs the CRC after corrupting the body (random
      bytes are stopped by the CRC and never reach the parser); header,
      reference-index and vector range checks; a failed decode leaves the
      reference store untouched; limits honoured for packet, frame,
      allocation, reference frames and recursion depth
      (`resource_limits_are_honoured`)
* [x] determinism (§39) — `encoding_and_decoding_are_deterministic`; the
      decoder's output is bit-identical to the encoder's own reconstruction
      (`decoder_matches_encoder_reconstruction_and_shape`,
      `every_reference_count_is_drift_free`) on the same build/target
* [x] `DynEncoder`/`DynDecoder` factories (geometry-carrying, like Thrum's),
      registered in the CLI's probe registry so a `.tpt` carrying a `VAUL`
      track probes as decodable
* [x] measured peak memory (§42-style) — `tests/memory.rs`: 482 784 B at
      176×144 4:2:0 with 4 references, independent of stream length; two
      frames' worth per extra reference (`docs/performance.md`)
* [x] `VaultDecoder::last_stats()` — a per-frame census of the tools used,
      so tests can prove partitioning/half-pel/multi-reference are exercised
      rather than merely present

Not done, and why (left honestly open):

* [x] CLI `encode`/`decode`/`benchmark`/`transcode` through the y4m bridge
      (2026-10-01) — `VideoCodec` now covers thrum, vault and mote, and the
      five video command paths erase at `Encoder/Decoder<Frame = VideoFrame>`
      rather than Thrum's concrete types. **Deliberately *not* the type-erased
      `Dyn*` contracts the FFI and the web player use**: `DynFrame` carries no
      PTS and the `Dyn*` wrappers substitute `Timestamp::zero` for it, so a
      stream muxed through them loses every frame timestamp. The CLI's y4m
      bridge sets PTS and the container needs it, so erasure happens at the
      concrete frame type — enough to drop the per-codec types, without
      losing timestamps. `video_packets_keep_the_pts_the_bridge_assigned`
      pins that, and fails if the path is ever rerouted through `Dyn*`.
      The Thrum-specific `% 16` geometry check is gone: each codec's own
      `validate()` refuses a frame it cannot code and names the real
      constraint, where the hard-coded rule would have been wrong for at
      least two of the three (Vault codes in 8x8 units and accepts 40x24,
      which Thrum and Mote reject). Five new tests: a y4m round trip per
      codec through `decode` (which re-derives the decoder from the track
      header alone, so it covers the codec-private bytes too), PTS
      preservation, per-codec geometry refusal, `benchmark` naming the codec
      it measured in both human and JSON output, and a vault->mote
      `transcode`. `todo.md` §33's probe-registry-only caveat no longer
      applies
* [x] §36/§37 video-corpus quality and a bitrate comparison against another
      codec (2026-10-01) — `tpt video-corpus` takes `--codec` and now
      measures thrum, vault or mote over the same eight scenes at the same
      geometry, reporting the codec it actually measured in both the table
      and the JSON. QCIF 176x144 4:2:0, 4 frames/scene, release build on a
      shared machine, mean PSNR / bytes per scene (luma):

      | scene | thrum | vault | mote |
      |---|---|---|---|
      | talking-head | 34.0 dB / 422 B | 44.1 dB / 317 B | 44.8 dB / 10 342 B |
      | camera-motion | 52.2 / 312 | 54.8 / 87 | 58.6 / 10 116 |
      | static-scene | 52.3 / 312 | 55.3 / 87 | exact / 10 116 |
      | high-motion | 18.3 / 29 333 | 28.6 / 37 401 | 15.7 / 11 635 |
      | screen-content | 25.0 / 1 222 | 36.2 / 1 849 | 35.7 / 10 133 |

      Vault is uniformly higher-quality than Thrum at the same `qp`, and on
      the three predictable scenes it is *also* smaller (87 B vs 312 B at
      camera-motion) — but it pays for it in encode CPU: 17-104 ms/frame
      against Thrum's 4-7, so it blows the 33.3 ms frame budget on every
      scene except the two easiest. Mote is the opposite: ~0.5-5 ms/frame and
      ~10 KB per scene regardless of content, the latter dominated by its
      dictionary packet amortised over only four frames; it also collapses on
      high-motion (15.7 dB, SSIM 0.28), which is the expected result for a
      codec built for static cameras. **No cross-codec "high-compression"
      claim is made**: these are synthetic clips, all three ran at the same
      `qp` which does not mean equal quality, and Mote's figures are
      dominated by a one-off dictionary. Real-content numbers still do not
      exist in this repo.

      The reported `frames` is decoded *pictures*, not packets — Mote
      prefixes its first frame with a dictionary packet that carries none,
      and because `frames` is the divisor in the bitrate/bpp maths, counting
      packets understated Mote's rate by 25% (496 → 621 kbps on
      talking-head). Pinned by `corpus_counts_decoded_pictures_not_packets`.
* [ ] B-frames, bi-prediction, weighted prediction, long-term references and
      reference reordering
* [ ] rate control (`qp` is a constant), scene-cut detection, per-block `qp`
* [ ] coefficient-level RDO (RDOQ / trellis), a tuned `λ`, and an RD-decided
      loop-filter flag
* [ ] a faster encoder — the direct `O(n³)` DCT evaluated at every partition
      level is the suspected dominant cost, not profiled (about 45-55 ms/frame at QCIF); the encoder is not
      real-time and the decoder is not claimed real-time either
* [x] bare-metal `no_std` verification — 2026-10-05, and the claim was
      false until it was tested (the recurring pattern): `cargo build
      -p tpt-video-vault --target thumbv7em-none-eabi
      --no-default-features` failed inside `bitflags` because both
      video codecs' dependency blocks forced `features = ["std"]` onto
      `tpt-media-core`/`-bitstream`/`-codec`/`tpt-dsp-core`
      *unconditionally*, so no-std resolution never happened. Fixed to
      reed's shape (`default-features = false` + `alloc`-only
      `tpt-dsp-core`, `std` opt-in through the crate's own feature);
      no `libm` needed — the codecs' float math stays inside `core`.
      Both vault and mote now build bare-metal locally and in the CI
      `no_std` job alongside reed
* [x] a WASM build and the web player (§45) now include Vault —
      2026-10-05, via the exact route §8 of `docs/web-design.md`
      recorded: the codec became a `tpt-web` dependency and got a
      `DecoderTable::native()` line plus a builder mirroring the CLI's
      track reconstruction. Mote came along in the same change (it had
      the same gap); the rebuilt wasm is ~368 KB and the Node checks
      pass against it
* [ ] maximum decode work (§19 `max_decode_micros`) — per-packet work is
      bounded structurally by the frame geometry, but no time budget is
      checked

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

# 34. Mote

**Purpose:** extreme edge / IoT video.

All items below implemented 2026-09-30 in `codecs/mote` (`tpt-video-mote`,
`MOTE`, bitstream v1); design in `docs/codec-design.md`, measurements in
`docs/performance.md`, sources in `docs/provenance.md`. 55 tests (42 unit,
7 property/hostile-input, 4 low-bitrate, 1 temporal, 1 memory).

* [x] define supported dimensions — `Gray8`/`Yuv420`, width and height
      non-zero multiples of 16, long edge <= 848, short edge <= 480
      (`picture::validate_dimensions`; test `dimension_rules`)
* [x] define patch representation — 8x8 samples of one plane (dimension
      64), numbered Y then U then V in raster order; all planes share one
      dictionary (`patch_counts_match_the_geometry`,
      `every_patch_loads_and_stores_back_exactly`)
* [x] define dictionary — 1..=4096 entries of 64 bytes, index width
      `ceil(log2(entries))` (12 bits at 4096), 16-bit dictionary id in every
      frame packet, sent in-band once or provisioned out of band; deterministic
      seeded integer Lloyd trainer (`training_is_reproducible`,
      `full_size_dictionary_uses_twelve_bit_indices`)
* [x] implement VQ — reuses `tpt_dsp_core::nearest_vector_accelerated`
      (§8); only codebook training is Mote's own. Tree/product quantisation
      (§8's open item) is **not** implemented
* [x] implement encoder — key and delta frames, Rice-coded skip runs,
      provable packet bound (`packets_never_exceed_the_provable_bound`)
* [x] implement decoder — CRC first, versioned, `ResourceLimits`, every
      count/index validated, state committed only on success
      (`corruption_sweep_never_panics_and_a_clean_stream_still_decodes`,
      `an_out_of_range_index_is_malformed_and_does_not_disturb_the_reference`)
* [x] define reconstruction model — decoded picture is dictionary entries,
      no post-filter; a skipped patch keeps the previous decoded patch bit
      for bit; decoder output equals the encoder's own reconstruction
* [x] investigate temporal prediction — conditional replenishment built and
      measured (`tests/temporal.rs`; table in `docs/codec-design.md`: ~25-100x
      fewer bytes than independent coding on the test scenes). **Not
      investigated**, still open: motion-compensated candidates and
      index-stream context/delta coding
* [x] define quality metric — luma PSNR primary, all-plane PSNR and mean
      absolute error secondary (`src/quality.rs`,
      `quality_metric_is_defined_as_documented`); a fidelity yardstick only
* [x] benchmark bandwidth — `benches/codec_bench.rs` prints key/delta sizes,
      kbps at 1/5/15 fps and PSNR; numbers in `docs/performance.md`.
      Benchmark-defined, not a guarantee: a key-frame-only 640x480 stream at
      1 fps is 57.7 kbps, so sub-50 kbps at 480p depends on temporal skip
* [x] benchmark CPU — same bench (key/delta encode and decode, training).
      Caveat recorded with the numbers: measured on a machine shared with
      concurrent builds, so timings are roughly +/-2x
* [x] benchmark memory — `tests/memory.rs` (counting allocator, 4096-entry
      dictionary); the old Mote memory-budget row was replaced by measured
      figures
* [x] test extremely low bitrate — `tests/low_bitrate.rs`: exact 12-bit key
      frame, 11-byte heartbeat, 2-entry-dictionary floor, static-camera
      bandwidth regression bound
* [x] per-crate documentation — `codecs/mote/README.md` and
      `CHANGELOG.md`, plus its own `keywords`/`categories`, land **in the
      same change** that adds the crate as a workspace member
      (CONTRIBUTING §9.4; enforced by the CI `metadata` job).

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

* [x] versioned bitstreams — 2026-09-11: Aura and Cinder packets both now
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
  frame/format a codec doesn't handle (e.g. Aura/Cinder reject
  non-audio `DynFrame`s); and the new versioned-bitstream check above
  extends this to unrecognized wire versions
* [ ] forward-compatible extension mechanism — the container format has
  one (reserved chunk ID ranges, §24), but the native codecs'
  *packet-level* bitstreams do not: every Aura/Cinder wire-format change
  so far (CRC trailer, lossy Bark-band quantization, the version field
  above) has been a breaking change with no reserved/extension bits.
  Left honestly unchecked rather than retrofitting an unused mechanism
  with no real requirement driving its shape yet.

---

# 36. Codec Quality Testing

Create reference corpora.

## Audio

* [ ] speech — no real speech corpus is available in this environment;
      not approximated synthetically (consistent with §7's honest
      pitch-corpus stance)
* [ ] music — same: no real recordings; `signals::mixed` is labelled as
      a synthetic stand-in, not a substitute
* [x] transient-heavy material — 2026-09-14: synthetic click trains
      (`signals::click_train`, sparse imcinders with exponential
      ring-down) driven through both codecs in the §37 corpus tests;
      synthetic rather than recorded, noted as such
* [x] synthetic signals — 2026-09-14: `apps/tpt-cli/src/signals.rs`
      generates the corpus deterministically (SplitMix64 + closed-form
      signals; no committed binary fixtures), and
      `apps/tpt-cli/tests/quality.rs` drives every signal through real
      Cinder and Aura round trips (Aura bit-exact on all of them,
      including full-range noise)
* [x] silence — 2026-09-14: corpus case; both codecs must decode it
      back to exact digital silence (asserted)
* [x] pathological inputs — 2026-09-14: full-scale DC and full-scale
      alternating (Nyquist) square, plus full-range random noise as the
      worst-case lossless input; all finite, full-length, no panics

## Video

All eight categories below are 2026-09-28 work:
`apps/tpt-cli/src/video.rs::corpus` generates each as a deterministic
seeded scene (SplitMix64 + closed-form fields, no committed fixtures —
same rule as the audio corpus), and
`apps/tpt-cli/tests/video_quality.rs` drives every scene through real
Thrum round trips in both coded formats, pinning per-scene rate/quality
floors and the corpus-wide bitrate ordering. The module doc carries the
same honesty note as the audio corpus: the real-world names describe
the *measurable structure* each case contributes (a locally-smooth
drifting region for a head, a global translation for a camera pan), not
genuine photographic content, which cannot be synthesized faithfully.

* [x] talking head — `video::talking_head`: slow-drifting smooth oval on
      a textured background; the regime a real call spends its life in
* [x] camera motion — `video::camera_motion`: the static field under a
      global translation; must compress like a static scene because
      integer motion estimation cancels it
* [x] static scenes — `video::static_scene`: identical frames; the skip
      path should carry the whole frame (pinned near-zero rate)
* [x] high-motion scenes — `video::high_motion`: per-frame decorrelated
      content; the regime inter prediction cannot help
* [x] animation — `video::animation`: flat regions with hard edges and
      step-wise motion
* [x] screen content — `video::screen_content`: text-like glyph rows
      with high-contrast edges and long static runs
* [x] synthetic patterns — `video::synthetic_patterns`: checkerboards,
      ramps, grids — known-answer geometry a DCT codec must reproduce
      exactly
* [x] pathological frames — `video::pathological_frames`: full-scale
      noise, full-scale DC, alternating (Nyquist) patterns — rate
      explosions and worst-case PSNR, asserted only as bounds/ordering,
      never as floors

---

# 37. Objective Quality Metrics

Audio (all in `apps/tpt-cli/src/quality.rs`, 2026-09-14; pure
deterministic functions, unit-tested, and consumed by `tpt benchmark`
and the §36 corpus tests):

* [x] SNR — `snr_db`: classic 10·log10(Σref²/Σerr²), `None` for silent
      references (undefined, not infinite), `Some(inf)` for bit-exact
      round trips (callers report `bit_exact` instead, since JSON
      cannot carry `inf`)
* [x] spectral error — `log_spectral_distance_db`: per-frame
      Hann-windowed FFT magnitude comparison in dB (RMS per bin,
      averaged over 50%-overlapped frames), via `tpt-dsp-core`'s FFT;
      self-tests verify identical signals → ~0 and that heavier
      spectral damage scores strictly worse
* [ ] perceptual metrics where practical — deliberately not attempted:
      `tpt-dsp`'s masking toolkit is available, but a real perceptual
      metric (weighting, spreading integration, AOD) is a design task
      with no consumer driving its shape yet; SNR + LSD are the honest
      current measurement
* [x] bitrate/quality curves — `tpt benchmark --quality q` emits the
      (payload bitrate, bits/sample, SNR, spectral distance) point as
      JSON per run; a curve is the sequence over q = 0..100 (points,
      no plotting)
* [x] CPU/quality curves — same mechanism: `encode_ms`/`decode_ms`
      alongside the quality numbers in the same JSON record

Video (all 2026-09-28, in `apps/tpt-cli/src/quality.rs` and
`apps/tpt-cli/src/video.rs`; consumed by
`apps/tpt-cli/tests/video_quality.rs`, which drives every §36 video
scene through real Thrum round trips):

* [x] PSNR — `psnr_db`: `None` for bit-exact frames (undefined, not
      infinite; the corpus has real bit-exact cases — static scenes on
      the skip path — so this is exercised, not theoretical)
* [x] SSIM — `ssim`: 8×8-window luminance/contrast/structure over the
      luma plane, `None` when the inputs are identical
* [x] bitrate — `video_bitrate_bits_per_second` + `bits_per_pixel`, and
      `VideoReport::bitrate_bps`/`bpp` accessors; the corpus-wide
      ordering test pins that rate tracks inter-frame predictability
* [x] frame latency — `Timing::latency_ms`/`realtime_factor`/
      `fits_frame_budget` against a caller-supplied fps; Thrum never
      buffers frames, so one P-frame's encode+decode is the one-way
      latency contribution (§41)
* [x] encode CPU — `VideoReport.encode` (`Timing`, per-frame)
* [x] decode CPU — `VideoReport.decode` (`Timing`, per-frame)
* [x] memory — `VideoReport.memory`: a `MemoryReport` carrying the
      measured peak live heap and cumulative churn, filled by the
      `tpt-cli::memprobe` tracking allocator when the process installed
      it (§42) and honestly `None` when it did not — an absent
      measurement and a measurement of zero are different facts

---

# 38. Testing

## Unit tests

* [ ] every public primitive
* [ ] edge cases
* [ ] malformed input
* [ ] overflow
* [ ] empty input
* [ ] maximum values

Audited crate by crate against the categories above, since the umbrella items
can only be checked when every crate has been swept. **Every foundation crate
has now been swept** (2026-09-30 → 2026-10-01):
`tpt-media-bitstream`, `tpt-container`, `tpt-media-codec`,
`tpt-media-pipeline`, `tpt-media-representation`, `tpt-media-research`,
`tpt-media-container` (1 → 16 tests; its single previous test asserted nothing
that could fail), `tpt-media-io` (`sink.rs` 0 → 12 tests; it held the 64 KiB
auto-flush logic and nothing tested it) and `apps/tpt-web`.

The umbrella items stay `[ ]` deliberately: "every public primitive" is a
stronger claim than "every crate has been swept", and a sweep is a point-in-time
audit that new public items immediately invalidate. Each one so far has found
real defects — two profile-dependent answers in `tpt-media-representation` and
`tpt-media-pipeline`, two division-by-zero paths in the research crate, and one
documented-but-unimplemented `no_std` claim in `tpt-media-container` — which is
the reason for doing them one crate at a time.

## Property tests

All five, 2026-09-14, following the project's dependency-free seeded
LCG strategy (§38 note above) rather than a property-framework crate:

* [x] encode/decode invariants — `codecs/{cinder,aura}/tests/property.rs`:
      full-range random audio round trips (Aura lossless bit-exact on
      arbitrary S16 including worst-case noise; Cinder within its
      measured floors), silence stays silent, length preserved, finite
      output
* [x] timestamp invariants — `crates/tpt-media-core/tests/property.rs`:
      rescale identity, weak monotonicity, finer-then-back round trip
      within 2 ticks (the truncation bound), checked-vs-saturating
      duration agreement, overflow → `None`
* [x] bitstream invariants — `crates/tpt-media-bitstream/tests/property.rs`:
      seeded-random write/read round trips through `BitWriter`/
      `BitReader` (mixed widths), `peek` ≡ next `read`, Rice/Huffman/
      range round trips (the range one found the `freq > 1` decoder
      desync — see Known Issues)
* [x] container invariants — `containers/tpt-container/tests/property.rs`:
      seeded-random packet streams (payloads, flags, PTS) round-trip
      exactly through mux→demux, including extreme/duplicate
      `i64::MAX`-scale timestamps (§47)
* [x] transform properties — owned by `tpt-dsp`, where the transform
      tests live: FFT vs naive DFT, MDCT/DCT-IV fast-vs-direct
      equivalence and TDAC round trips, DCT orthogonality, LPC/LSP
      round trips (§5, §6, §9, §12 there); `tpt-media`'s codec-level
      property tests pin the composed result end-to-end

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
  added to both `tpt-audio-aura` and `tpt-audio-cinder` (arbitrary bytes +
  full truncation sweep + bit-flip sweep of a real packet). **Found and
  fixed a real out-of-bounds write in Cinder**: `n_bands`/`n_coeffs` are
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
  and the new per-codec truncation sweeps above (Aura/Cinder)
* [x] random packet ordering — `tpt-audio-cinder`'s
  `jittered_pts_are_preserved_not_reordered` (§29, added 2026-09-09)
  delivers packets out of PTS order and asserts no reordering/panic;
  Aura has no cross-packet decode state at all (`AuraDecoder` holds no
  mutable state between `decode()` calls), so packet ordering is
  structurally a non-issue there, not just untested

---

# 39. Determinism

All six, 2026-09-14:

* [x] deterministic decoding — `encoding_and_decoding_are_deterministic`
      in `codecs/{cinder,aura}/tests/property.rs`: independent
      encoder/decoder instances produce byte-identical packets and
      byte-identical decodes; both codecs are pure functions of
      (bytes, config) — no RNG, wall-clock, or thread state
* [x] deterministic container generation where practical —
      `muxing_is_deterministic`
      (`containers/tpt-container/tests/property.rs`): the muxer writes
      no timestamps, counters, or padding entropy; same stream in,
      byte-identical `.tpt` out
* [x] deterministic scalar DSP — `tpt-dsp`'s kernels are pure functions
      over fixed iteration orders (§12/§13 there); this workspace pins
      that end-to-end at the media layer via the codec determinism
      tests above
* [x] document SIMD differences — `docs/performance.md` §Determinism
      records the build-time-dispatch story and points at
      `tpt-dsp/ARCHITECTURE.md` §10's support matrix; same-binary
      results are deterministic, cross-`target-cpu` results may differ
      within documented tolerances
* [x] deterministic test corpus — §36's `apps/tpt-cli/src/signals.rs`:
      every corpus signal is closed-form or SplitMix64-seeded; same
      seed, same bytes, no committed binary fixtures
* [x] reproducible benchmark configuration — the criterion benches fix
      `sample_size`/`warm_up_time`/`measurement_time` in code so plain
      `cargo bench` reproduces the configuration; `docs/performance.md`
      records the measured tables, their machine caveat, and the
      same-CPU/same-toolchain reproducibility policy

---

# 40. Scalar / SIMD Equivalence

For all optimized DSP kernels. All the kernels live in `tpt-dsp` (§3),
and §13 there already records the scalar reference + equivalence-test
work — this section was silently re-asking for it; reconciled
2026-09-14, with the two genuinely-open items left open:

* [x] scalar reference — `tpt-dsp-core/src/simd_scalar.rs`, the default
      fallback path (§13)
* [x] SIMD implementation — `tpt-dsp-core/src/simd.rs` (`core::simd`
      complex mul/add/magnitude, FFT butterfly; §13)
* [x] same-input comparison — `simd_mul_matches_scalar`,
      `simd_add_matches_scalar`, `simd_magnitude_matches_scalar`,
      `simd_fft_matches_scalar_fft` (§13)
* [x] tolerance definition — per-kernel-family tolerances stated in
      each module's `# Numerical contract` section (§12 there: ~1e-3/
      1e-4 `f32`, ~1e-9/1e-10 `f64` for the FFT-backed transforms)
* [ ] randomized testing — the equivalence tests use fixed vectors;
      seeded-random scalar-vs-SIMD sweeps are the remaining gap
      (recorded in `docs/performance.md` §SIMD Equivalence)
* [ ] architecture-specific CI — 2026-10-05: no longer x86_64-only —
      CI cross-checks every Rust crate for `aarch64-unknown-linux-gnu`
      (`cargo check`, no linker needed; `tpt-media-sys` excluded
      because its build script compiles the C integration test with a
      target-unknown compiler, and criterion's `alloca` keeps
      `--all-targets` off). Still open: *executing* the test suite on
      AArch64 (QEMU or a runner) and cross-`target-feature` builds,
      which is what the SIMD-equivalence story really wants

---

# 41. Performance

Benchmarked 2026-09-14 (criterion benches added to this workspace;
results + machine caveats recorded in `docs/performance.md`):

* [x] MDCT — `tpt-dsp-core/benches/dsp_bench.rs::bench_mdct` (direct vs
      `FastMdctPlan`, all 6 target sizes; §5.1)
* [x] IMDCT — same bench, inverse direction (§5.1)
* [x] FFT — `tpt-dsp-core/benches/dsp_bench.rs` (§5.1's FFT backing)
* [x] DCT — `bench_dct` (DCT-II/III/IV; §5.2)
* [x] LPC — `bench_lpc` at Aura's orders 8/16/32 (§6)
* [x] VQ — `bench_vq`, linear vs accelerated at Reed/Mote codebook
      sizes (§8)
* [x] psychoacoustics — `bench_psychoacoustic`: masking + tonal
      classification (§10)
* [x] entropy coding — `crates/tpt-media-bitstream/benches/
      bitstream_bench.rs`: bit I/O, Rice, range (peek/update), Huffman
      at codec batch sizes
* [x] Cinder encode/decode — `codecs/cinder/benches/codec_bench.rs`
      (64×512-sample blocks @ 48 kHz; also `tpt benchmark` for
      end-to-end wall time on a real WAV — §29)
* [x] Aura encode/decode — `codecs/aura/benches/codec_bench.rs`
      (lossless, 32×1024-sample blocks @ 48 kHz)
* [x] Reed encode/decode — 2026-09-28:
      `codecs/reed/benches/codec_bench.rs` (encode/decode of 64×
      320-sample 20 ms frames @ 16 kHz, alternating voiced/noise
      blocks); results in `docs/performance.md` (§31)
* [x] Thrum encode/decode — 2026-09-28:
      `codecs/thrum/benches/codec_bench.rs` (encode/decode of 8-frame
      clips at 176x144 4:2:0 and 640x480 mono, I-frames and P-frames
      measured separately); results in `docs/performance.md` (§32).
      P-frame figures are the latency-relevant ones: Thrum never
      holds a frame back, so one P-frame's encode + decode *is* the
      one-way latency contribution
* [x] Vault encode/decode — 2026-09-30:
      `codecs/vault/benches/codec_bench.rs` (I-frame and P-frame encode and
      decode of 4-frame clips at 176x144 and 352x288 4:2:0), plus the
      `#[ignore]`d `tests/measure.rs` harness for the rate/quality/tool
      tables; results and machine caveats in `docs/performance.md` (§33)
* [x] Mote encode/decode — 2026-09-30: `codecs/mote/benches/codec_bench.rs`
      (key/delta encode + decode of 8-frame fixed-camera clips at 320x240
      mono/4:2:0 and 640x480 mono; bandwidth and PSNR printed; dictionary
      training); results in `docs/performance.md` (§34)
* [x] container mux/demux — `containers/tpt-container/benches/
      container_bench.rs` (200-packet stream both directions)
* [x] pipeline throughput — `crates/tpt-media-pipeline/benches/
      pipeline_bench.rs` (1000 packets through source→map→sink)

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

* [x] allocation profiling — 2026-09-28:
      `apps/tpt-cli/src/memprobe.rs` factors the counting-allocator
      mechanism the per-codec `realtime_alloc.rs` tests each carried
      (four near-identical copies) into one reusable harness:
      `TrackingAlloc` (a `GlobalAlloc` mirroring live heap bytes into
      atomics — the one `unsafe` block in `tpt-cli`, confined there on
      purpose) plus `MemoryProbe`, a mutually-exclusive measurement
      window reporting the high-water mark of *live* bytes and
      cumulative churn, saturating on frees that predate the window so
      a stale `dealloc` cannot wrap the counters into a nonsense peak.
      The bound-style assertions stay in the codec crates where they
      guard real-time paths; the container/pipeline layers are not yet
      measured by this harness (still `criterion`-profiler follow-up)
* [x] real-time allocation assertions — 2026-09-14:
      `codecs/cinder/tests/realtime_alloc.rs` runs the steady-state
      encode/decode path under a counting global allocator and asserts
      peak allocations stay within 512 KiB over 31 blocks (~25× the
      measured working set) — catches per-frame allocation growth or
      accidental multi-MB scratch, which is the property the real-time
      claim actually needs
* [x] peak memory measurements — 2026-09-28:
      `apps/tpt-cli/tests/memory_budget.rs` measures every implemented
      codec's steady-state streaming working set (Cinder 6 645 B, Aura
      25 330 B, Reed 8 140 B, Thrum 152 503 B — the last pinned
      *length-independent*: 8× the clip does not cost 8× the memory)
      against the budgets documented in `docs/performance.md`
      ("Memory Budgets", now a measured table, with the stale
      "recorded in each codec's `PROVENANCE.md`" pointer removed —
      those files do not exist). The measurement reshaped two things:
      Reed's documented budget rose from a pre-measurement "< 8 KiB"
      (the measured peak filled 99% of it — a flake in waiting) to
      "< 16 KiB", and Thrum's encoder stopped allocating its ~2.4 MB
      worst-case packet buffer per frame (now allocated once and
      reused), which is what the measured ~149 KiB peak actually
      reflects
* [x] hostile-input memory tests — covered: the §38 corruption/
      truncation/bit-flip sweeps (asserting no panic on hostile bytes)
      plus the §19/§47 resource-limit and bomb tests (asserting hostile
      *sizes* are rejected before allocation) are exactly these
* [x] maximum-frame tests — §19: Cinder/Aura `check_frame_bytes` /
      `check_allocation` tests assert a tight budget yields
      `ResourceLimit(Allocation)` on structurally valid packets;
      `docs/security.md` records the limit table
* [x] maximum-packet tests — §19 `check_packet` (decoders) and the
      container-side `TMTA`/`IDX ` body-length rejections
      (`oversized_tmta_metadata_is_rejected_by_default...`,
      `decode_idx_rejects_entry_count_that_would_overrun_the_body`)

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

Done 2026-09-08, completed 2026-09-14. `apps/tpt-cli` (`tpt` binary +
lib core). `probe` / `inspect` / `list-codecs` / `--help` fully
implemented; `encode`/`decode` live end-to-end (WAV → `cinder`/`aura` →
`.tpt` and back) since 2026-09-08. **2026-09-14: `transcode` and
`benchmark` are live too** — `transcode` decodes the input `.tpt` and
re-encodes with the `--codec` target (metadata carried, `--set-meta`
overrides); `benchmark` measures encode/decode wall time (median/min
over `--runs` passes after an untimed warm-up), payload bitrate, and
§37 quality (SNR + spectral distance), with `--json`. Both use the
shared `EncodeJob`/`demux_and_decode` paths that `encode`/`decode` use,
so the four commands stay behaviorally identical where they overlap.
Design: `docs/cli-design.md`.

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

* [x] WASM-compatible decoder — 2026-09-30: `apps/tpt-web` builds for
      `wasm32-unknown-unknown` (`cdylib`, 328 KiB release) with Cinder, Aura,
      Reed and Thrum (`DecoderTable::native`); Vault/Mote are not
      dependencies and enter through `DecoderTable::register` (extension
      point, tested with a fake codec). The wasm build decodes the demo file
      exactly (192 000 samples, 60 frames) and survives 300 hostile files
      with zero traps, under Node (`web/check.mjs`)
* [x] `.tpt` parser — `StreamParser`: bounded incremental push parser reusing
      `tpt-container`'s chunk decoders; sizes checked from the chunk header
      before buffering, unknown chunks skipped unbuffered, bad CRC-32
      clusters dropped; byte-at-a-time output identical to whole-file;
      deterministic mutation/truncation sweeps
* [x] streaming playback — `Player` push/pump/pop with end-to-end
      back-pressure (`InputFull`, queue soft limits and 2x hard stops),
      drift-free audio timestamps, exact tail trim from `tpt.io.sample_count`
      (Aura lossless round-trips bit-exactly), graceful end on a truncated
      stream. Tested natively and as wasm under Node. The *browser* playback
      scheduler is under the two items below
* [x] browser audio output — `web/tpt-web.js` schedules planar `f32` chunks
      ahead of the `AudioContext` clock (underrun re-anchoring, pause via
      `suspend`). Its control flow is executed against a **fake**
      `AudioContext` (`web/mock-browser.mjs`: 375 buffers back to back, 4.0000
      s total) **and against a real one** (2026-10-01,
      `web/browser-check.mjs` + `serve.mjs`; headless Chrome, unmodified
      player): a real `AudioContext` created and released by the player
      (`state` back to `closed` at end of stream, `baseLatency` 0.01 s),
      375 real `AudioBufferSourceNode`s handed 4.0000 s of mono 48 kHz PCM
      that the browser accepted, scheduled back to back against the real
      clock with no gap, every buffer non-silent. **What this does not
      establish: that a human has heard anything.** It proves the browser
      took the audio and the clock advanced (4.141 s of context time for
      4.0 s of media); it cannot prove the result is *audible*, only
      correct. On a machine with no audio output device the clock never
      moves, so the check reports these items as "not verifiable here"
      rather than passing.
* [x] browser video output — `putImageData` of RGBA8 pictures paced from
      `requestAnimationFrame` against the audio clock (stale pictures
      dropped). Exercised against a fake canvas, and against a **real** one
      (2026-10-01, `web/browser-check.mjs`): a real 160x96 canvas read back
      through the browser's own `getImageData` — 2834 distinct colours,
      every one of 15 360 pixels opaque, full 0-255 contrast, 60 pictures
      presented and 0 dropped over the 4 s demo, 0 decode errors. RGBA
      conversion itself (full-range BT.601, NV12, gray, RGB) is unit-tested
      natively. The check deliberately asserts *that a real rasterisation
      of a real decoded picture happened*, not that the picture is the
      right one.
* [x] JavaScript bindings — 18 `extern "C"` exports over integer handles
      (no pointer flows JS to Rust; `unsafe` confined to `abi.rs`) plus
      `web/tpt-decoder.js`. **Deliberately not `wasm-bindgen`**, justified in
      `docs/web-design.md` §3; `ARCHITECTURE.md` updated. ABI lifecycle,
      bad-handle and limit behaviour tested natively and through the built
      wasm under Node
* [x] memory limits — `WebLimits` on top of §19's `ResourceLimits`: input
      backlog, packet/metadata/track sizes, RGBA frame size, audio/video
      queues, tracks, decode-error budget, players per module; caller values
      clamped to a ceiling; worst case computed (223 MiB for the defaults,
      pinned by a test) and the wasm instance capped by `--max-memory`
      (`web/build.mjs`, default 512 MiB). `docs/security.md`,
      `docs/web-design.md` §7. `max_decode_micros`/cancellation remain
      unused (§19 open items)
* [x] browser demo — `web/index.html` + `tpt-web.js` + `tpt-decoder.js` +
      `demo.tpt`; static, no bundler, plays the demo, a local file or a URL.
      Logic-tested against fakes, **and loaded in a real browser**
      (2026-10-01, `web/browser-check.mjs`): the shipped `index.html` is
      loaded same-origin in an iframe and its own "Play demo.tpt" button
      clicked, with nothing in the page patched or stubbed — the
      assertions read the text the page writes into its own `#stats`
      and `#error` boxes. It reached end of stream in 4.2 s, its media
      clock advanced to 4 s, it presented 60 pictures with 0 decode
      errors, and its canvas ended up 160x96. Opening the same page by
      hand in Chrome is still worth doing: the check reads the page's
      counters, it cannot see the page.
* [x] performance benchmark — `cargo bench -p tpt-web` (native) and
      `web/bench.mjs` (the same byte-identical inputs as wasm under
      Node/V8). Real numbers in `docs/performance.md`: e.g. Cinder 30 s
      decodes in ~18 ms native / ~26 ms wasm (1100-1650x real time), Thrum
      640x352 at ~300 fps in wasm. Machine was shared, video numbers noisy
      (ranges given); **no in-browser numbers exist**
* [x] per-crate documentation — `apps/tpt-web/README.md` and
      `CHANGELOG.md`, plus its own `keywords`/`categories`, land **in the
      same change** that adds the crate as a workspace member
      (CONTRIBUTING §9.4; enforced by the CI `metadata` job). A WASM app
      is std/I-O-bound by nature, so it is exempt from the `no_std`-first
      rule in CONTRIBUTING §10 but not from §9.4.

---

# 46. FFI

Create `tpt-media-sys`.

The FFI exposes the **stable media abstraction**, not internal
implementation details. **v0.1 shipped 2026-09-29** with the codec
abstraction slice — packet, frame, encoder, decoder — for the three
audio codecs; the full `Expose` list below is the surface contract,
and `Context`/`Container`/`Demuxer`/`Muxer` handles are the v0.2
slice (the `Probe` layer already covers container introspection from
Rust). The abstract `tpt_media_config` (sample rate / channels /
quality 0–100) maps to per-codec parameters exactly as the CLI's
`--quality` does, so a C caller never touches per-codec structs.

Expose:

```text
Context          (v0.2)
Packet           (v0.1)
Frame            (v0.1 — audio; the video frame type lands with the video codecs' FFI slice)
Codec            (v0.1, as a packed 4-byte codec id + tpt_encoder_block_size)
Encoder          (v0.1)
Decoder          (v0.1)
Container        (v0.2)
Demuxer          (v0.2)
Muxer            (v0.2)
```

All ten mechanics items are 2026-09-29, in `ffi/tpt-media-sys`:

* [x] C ABI — `#[no_mangle] extern "C"` functions returning `i32`
      error codes, built as both `rlib` (Rust tests) and `staticlib`
      (`tpt_media_sys.lib`, what a C application links)
* [x] opaque handles — `TptEncoder`/`TptDecoder`/`TptFrame`/
      `TptPacket` are Rust structs behind `typedef struct X X;` in the
      header; C never sees a field
* [x] explicit ownership — documented per crate docs and per header
      comment: allocating calls return through `tpt_xxx**` out-params,
      the caller owns the handle, `const` handles are read-only for
      the call's duration
* [x] explicit allocation/free functions — every constructor has a
      matching `tpt_xxx_free`; freeing NULL is a documented no-op;
      `tpt_packet_new` copies the caller's bytes
* [x] error codes — `enum tpt_error` (`TPT_OK`…`TPT_ERR_PANIC`,
      `#[repr(i32)]`), mapped from the codec framework's `CodecError`;
      values and meanings are ABI-frozen per the stability rule
* [x] ABI version — `tpt_abi_version()` (`ABI_VERSION = 1`) with the
      stated rule: additions are compatible, signature/meaning/layout
      changes bump the version; `tpt_version_string()` for diagnostics
* [x] thread-safety documentation — crate docs define the model
      (exclusive `&mut`-style access per handle, distinct handles
      usable concurrently, no `Send` promise in v0.1); the header
      summarizes it
* [x] safety documentation — every `unsafe` export carries a `#
      Safety` section stating the caller's obligations, and every
      `unsafe` block carries a `// SAFETY:` comment tying it to one of
      those obligations; this crate is the one sanctioned `unsafe`
      home in CONTRIBUTING §4
* [x] C header generation — `include/tpt_media.h` is hand-written
      (deliberately no `cbindgen` dependency) and kept in sync with
      the ABI by `header_matches_the_abi`, which checks both
      directions (every export in the header, every header symbol in
      the library)
* [x] C integration tests — `tests/integration.c` is a real C
      translation unit that sees only the header (compiled by `cc`
      through the build script), drives a full encode→decode round
      trip plus NULL-argument rejection, and is asserted on by
      `tests/c_abi.rs`

Boundary behavior worth recording: the type-erased `DynFrame` carries
no timestamps, so the ABI owns PTS association — `tpt_encoder_encode`
stamps its packet with the frame's PTS, `tpt_decoder_decode` stamps
its frame with the packet's (a flushed frame carries PTS 0). Every
export also runs under `catch_unwind` — a Rust panic becomes
`TPT_ERR_PANIC` and never unwinds into C.

---

# 47. Security

* [x] threat model — `SECURITY.md` (hostile-input assumptions, attacker
      goals/mitigations table, supported versions) plus the
      implementation-level mitigations in `docs/security.md`
* [x] hostile media-input model — same documents: the input media file
      is the trust boundary, with the boundary crossings enumerated
      (demux `next_packet`, `BitReader` reads, codec `decode`)
* [x] integer-overflow audit — 2026-09-11: audited every length-prefixed
  field decoded from untrusted bytes in `tpt-container/src/format.rs`
  (`decode_tmta`/`decode_idx`/`decode_trak`/`decode_clus`) for
  wraparound-prone `usize` arithmetic on attacker-controlled `u32`/`u64`
  lengths. Found and fixed one real gap: `decode_tmta`'s `META_BYTES`
  branch used plain `p + len` (`len` a full attacker-controlled `u32`),
  which is safe on 64-bit `usize` but can wrap on a 32-bit target (this
  workspace explicitly builds/targets 32-bit — `thumbv7em-none-eabi` for
  Reed, and wasm32 for the planned §45 WASM target) and pass a bounds
  check it should have failed. Fixed with `checked_add`. Every other
  length field in that file is either itself narrow enough (`u8`/`u16`)
  to make wraparound unreachable on any realistic target, or already
  goes through `checked_add` (the chunk-size field in `read_chunk_header`
  / `TptDemuxer::scan`). Test:
  `decode_tmta_rejects_bytes_value_length_that_would_overrun_the_body`
* [x] bounds-check audit — 2026-09-11: covered together with §38's fuzz
  sweeps (Aura/Cinder OOB write fix) and the integer-overflow audit above;
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
* [x] recursion limits — Vault's quadtree decode is the one recursive codec
  path; it is bounded structurally to depth 3 below the root and checked
  against `limits.max_recursion_depth` (same evidence as §19's
  recursion-depth item)
* [x] decompression-bomb protection — the `decode_idx` fix above *is* a
  decompression-bomb fix (a few input bytes claiming a multi-gigabyte
  logical size); §19's metadata/allocation/packet/frame size limits
  cover the rest of this surface
* [x] malformed timestamp handling — audited 2026-09-14 (see Known
      Issues for the unsigned-envelope quirk found there): the demuxer
      does no timestamp arithmetic at all and any representable `i64`
      PTS/DTS round-trips exactly (`extreme_and_duplicate_pts...` in
      `tpt-container/tests/property.rs`); the probe — the first
      consumer that computes with timestamps — produces finite,
      non-negative reports or none on extreme/duplicated/descending/
      mixed-sign inputs (`tpt-media-probe/tests/timestamps.rs`);
      conclusion recorded in `docs/security.md` §Malformed Timestamps
* [x] fuzzing CI — `cargo test --workspace --locked` (already the
  existing CI gate, `.github/workflows/ci.yml`) runs every `#[test]`
  including all of §38's new fuzz-style sweeps; no separate workflow
  step needed since these are plain deterministic unit tests, not an
  external fuzz-harness job
* [x] security disclosure process — `SECURITY.md`: private reporting
      channel, acknowledgement/fix timelines, supported-version table

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

For each non-trivial algorithm (established 2026-09-29; the mechanism
is the per-algorithm record in `docs/provenance.md`, which now carries
a complete table for **all four implemented codecs** — Cinder, Aura,
Reed, Thrum — with the two that were missing added this session; DSP
primitives are covered by `tpt-dsp` §12's per-family contracts plus the
references in those same tables):

* [x] identify mathematical source — every row names its source paper /
      textbook / public standard; rows that are this project's own
      arrangement say so explicitly rather than borrowing a citation
* [x] record public references — same tables (Princen & Bradley,
      Markel & Gray, Schroeder, Terhardt, Traunmüller, Rice, Golomb,
      ISO/IEC 11172-3, ITU-T H.261/H.263/H.264, IEEE 802.3, …)
* [x] record implementation derivation — each table states where the
      implementation lives (`codecs/*/src/lib.rs` vs. `tpt-dsp-core`)
      and that no reference implementation was consulted or derived
      from; the "Deliberately *not* used" closing list per codec pins
      the derivation boundary
* [x] record contributor provenance — single-contributor project:
      authorship is the repository history; each record carries a
      dated, initialed patent review, and CONTRIBUTING §2 requires
      prior-project disclosure per algorithm touched
* [x] review patent concerns — every row is classified GREEN with a
      review date; CONTRIBUTING §3 lists the avoided technique
      families (CABAC, sub-pel interpolation, SBR, parametric stereo)
* [x] document uncertainty — the standing "no absolute patent-free
      claim — patent status is jurisdictional" disclaimer in every
      record and todo §1, the YELLOW-waiver policy, and the
      "When in Doubt" escalation path in `docs/provenance.md`

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

For native codec implementations (established 2026-09-29, same
mechanism as §48):

* [x] document algorithm sources — `docs/provenance.md` carries a
      complete per-codec table for all four implemented codecs
* [x] maintain implementation provenance — records are updated in the
      same change as the algorithm (e.g. the §42 buffer-reuse change
      landed with its provenance/§32 note); wire changes also bump the
      version table in `docs/codec-design.md`
* [x] avoid copying implementation structure from reference codecs —
      CONTRIBUTING §2's clean-room rules (no reading prohibited
      sources, no borrowed structure/naming)
* [x] do not derive code from proprietary source — same rules, plus
      the prior-project disclosure requirement per algorithm touched
* [x] document external references used — the `Reference` column of
      every provenance row
* [x] maintain codec-specific design notes — `docs/codec-design.md`'s
      per-codec sections, each closing with "known v0.1
      simplifications"/"sharp edges stated plainly"

The purpose is provenance and independent implementation, not an absolute legal guarantee.

---

# 50. Documentation

All twelve exist and were audited against the code 2026-09-28 (see §53's
documentation-baseline note for what the audit found and fixed):

* [x] README — crate table now distinguishes implemented vs *(planned)*
      (Vault, Mote, `tpt-web`) and lists the two research crates that
      existed but were missing from it; since 2026-09-30 every implemented
      crate's name links to its own README (CONTRIBUTING §9.4)
* [x] ARCHITECTURE — `ARCHITECTURE.md`; §6's codec-trait snippet now
      matches the real API (`CodecError`/`Vec<Frame>`,
      `DynEncoder`/`Registry`), §2's topology is complete, §7/§8/§11
      describe the actual container/pipeline/real-time behavior
* [x] codec design guide — `docs/codec-design.md`; format tables now
      carry honest per-codec status columns (the design matrix is
      aspirational, the `supported_formats` tables in code are binding),
      and the Real-Time/Determinism/Versioning sections describe
      implemented behavior instead of intent
* [x] container specification — `docs/container-design.md` (verified
      against `tpt-container`: header layout, TRAK/TMTA/CLUS/IDX /CRC1,
      seek behavior, streaming)
* [x] pipeline specification — `docs/pipeline-design.md` (written with
      the implementation; verified)
* [x] DSP dependency guide — the dependency boundary is documented in
      `ARCHITECTURE.md` §3/§14 and `docs/architecture.md` §1 (rationale
      for the bitstream split); no standalone guide on purpose — the
      boundary is two sections, not a subsystem
* [x] performance guide — `docs/performance.md` (measured tables,
      determinism/SIMD notes, reproducible bench configuration)
* [x] security model — `SECURITY.md` (threat model) +
      `docs/security.md` (implementation mitigations, limit tables)
* [x] provenance/IP policy — `docs/provenance.md` (per-algorithm
      records for all four implemented codecs — Cinder, Aura, Reed,
      Thrum; GREEN-only policy; per-crate records live in that one
      document, there are no per-crate `PROVENANCE.md` files)
* [x] contributor guide — `CONTRIBUTING.md` (states the contribution
      model up front — development is issues only, no external code
      contributions — then the clean-room rules, patent policy, and
      the §9–§11 engineering policies: versioning/stability, MSRV,
      feature flags, benchmarks)
* [x] API documentation — rustdoc on every public item;
      `cargo doc --workspace --no-deps` is now **zero-warning** (the
      audit fixed 13 broken intra-doc links across 7 crates)
* [x] examples — `crates/tpt-media-io/examples/io_tour.rs`: a runnable
      tour of the byte-exact WAV + PPM round trips (the `examples/`
      directory at the repo root is intentionally empty; examples live
      in the crate they demonstrate)
* [x] per-crate documentation — 2026-09-30: every one of the sixteen
      workspace members has its own `README.md` (user-facing: what it is,
      real usage, feature flags, API surface, testing, known limitations,
      linking `docs/` rather than duplicating it — the §8 "one canonical
      record" discipline applied to documentation) *and* its own
      `CHANGELOG.md` (a filtered view of the root one, per the new
      CONTRIBUTING §9.4 policy: lockstep versioning means a per-crate file
      cannot carry independent release history even if we wanted it to)
      *and* its own crate-specific `keywords`/`categories` instead of
      inheriting one workspace set — which had made all sixteen crates look
      identical in crates.io search and asserted `no-std` for the five
      std-only ones. Before this change **no crate had a README or a
      changelog**. `ci/check-metadata.py` (new CI `metadata` job) enforces
      all of it. The audit also fixed two real packaging blockers in
      `tpt-media-sys`: its inherited `readme` resolved to a file that did not
      exist, and its five path dependencies carried no `version` —
      `cargo package` failed on both.

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

* [x] establish versioning strategy — CONTRIBUTING §9.1: lockstep
      workspace versions from `[workspace.package]`, SemVer with `0.y`
      as the breaking-change counter pre-1.0, wire formats versioned
      independently of crate versions (16-bit bitstream version +
      fail-fast, §35), record-as-you-land changelog discipline
* [x] establish MSRV compatibility — 2026-09-29: the declared
      `rust-version` was **verified for the first time** and turned out
      to be false: the committed lockfile cannot even be parsed by
      1.75 (`clap_builder` 4.6 ships an edition-2024 manifest) and
      1.85 falls one short (`criterion` 0.8.2 requires 1.86); 1.86
      builds the whole workspace clean. `rust-version` is now 1.86, a
      CI `msrv` job runs `cargo check --workspace --locked
      --all-targets` on exactly that toolchain, and the bump policy
      (dependency-forced only, prefer pinning lower, CHANGELOG +
      job in the same change) is CONTRIBUTING §9.3
* [x] establish feature flags — CONTRIBUTING §10: `std` is the only
      default feature everywhere it exists (no_std + alloc beneath it),
      features are additive-only, capability features gate modes not
      correctness, new crates must be no_std-capable unless app/IO-bound
* [x] establish benchmark ownership — CONTRIBUTING §11: each crate
      owns its `benches/`, `docs/performance.md` is the canonical
      record (a bench without recorded numbers isn't the §41
      deliverable), benches deliberately stay out of CI, and a failed
      documented budget is a failed build, not a noisy benchmark
* [x] establish API stability expectations — CONTRIBUTING §9.2: no
      pre-1.0 guarantee (pin exact versions), two tiers of public
      surface (ecosystem crates vs. the explicitly experimental
      §56 research crates), deprecation-not-removal

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
* [x] CI operational — `.github/workflows/ci.yml` gates (`fmt --check`, clippy `--all-targets -- -D warnings`, `cargo build/test --workspace --locked`) all pass, and the no_std `tpt-audio-reed` job (`--target thumbv7em-none-eabi --no-default-features`) now builds: workspace `default-features = false` on `tpt-media-{core,bitstream,codec}` with members opting back into `std`, thiserror 1.x→2.x (1.x hard-requires `std`), `String`/no_std-gate fixes in `tpt-media-codec`, and `libm` for the required float math (2026-09-09)
* [x] basic media model
* [x] timestamp model — rational timebase, `rescale` overflow bug fixed 2026-09-07 (see Known Issues), signed-duration saturation + `checked_{add,sub}_duration`, disciplined `ClockDomain`/`SyncMap` (§16 all green)
* [x] bitstream primitives — the range coder finding was corrected 2026-09-07 (all bitstream tests pass); this gate item was blocked only on that
* [x] container prototype
* [x] one working codec — Cinder and Aura both have real, signal-preserving encode/decode round trips
* [x] fuzz infrastructure — 2026-09-11: §38's deterministic
  corruption/truncation sweeps now cover `tpt-media-bitstream` (new,
  found a real bug), `tpt-container` (pre-existing, §24),
  `tpt-audio-aura`/`tpt-audio-cinder` (new, found a real bug), and
  `tpt-media-probe` (new). No external fuzz-harness dependency; see §38's
  strategy note for why
* [x] documentation baseline — audited 2026-09-28, every top-level doc
      checked against the code it describes. Found and fixed: README's
      crate table listed unimplemented crates as real and omitted the
      two research crates; `ARCHITECTURE.md` §6 showed codec traits that
      don't exist (`DecodeError`, single-frame returns), §7-§8 predated
      the index/streaming container and the pipeline's actual
      backpressure model, §11 claimed "zero allocation" real-time (it is
      bounded allocation, pinned by tests) and a real-time thrum;
      `docs/architecture.md` claimed reed used "fixed-point i16/i32" (it
      is f64 + libm) and a `process_one()` pipeline API that never
      existed (it is `process()`/`Chain`/`Pipeline::pull`);
      `docs/codec-design.md`'s tables overstated implemented formats
      (96 kHz/8-channel Aura, stereo Cinder) and described a
      compatibility-mode version policy that the codecs deliberately
      don't have (§35: fail fast). Also: 13 broken rustdoc links fixed
      (`cargo doc --workspace` now zero-warning), a runnable `io_tour`
      example added, and stale "exit code 3 = not wired" CLI docs
      removed now that every listed codec is wired. The per-area design
      docs (container/pipeline/media-io/cli/security/provenance/
      performance/representation-research) were verified accurate —
      they were written with their implementations

* [x] crate metadata — 2026-09-30: every member declares its own
      `readme`, `keywords` and `categories`, ships a `README.md` and a
      `CHANGELOG.md`, and passes `ci/check-metadata.py` (the CI `metadata`
      job). This also cleared the last blocker to `cargo package`: the
      `tpt-media-sys` manifest was unpackageable (inherited `readme` path
      and versionless path dependencies), so a first crates.io publish was
      impossible before this.

Before 0.2:

* [x] MDCT — in `tpt-dsp` (§5.1: reference + `FastMdctPlan` + TDAC
      round-trip tests + benchmarks)
* [x] LPC — in `tpt-dsp` (§6: analyzer, Levinson-Durbin, stability
      tests, benchmarks)
* [x] VQ — in `tpt-dsp` (§8: search/encode/decode, accelerated search
      proven bit-identical, benchmarks)
* [ ] improved DSP APIs — open-ended; nothing driving a specific
      improvement right now
* [x] Cinder prototype — §29: encoder/decoder/PLC with round-trip,
      packet-loss, jitter tests; ≈3 dB structural ceiling recorded
      (Known Issues); real-time benchmarks recorded (§41)
* [x] Reed prototype — §31: LPC + LSP + pitch + stochastic-excitation VQ
      vocoder with PLC, property/fuzz/alloc tests, criterion bench, CLI
      wiring, provenance records (2026-09-28); honestly robotic by
      design at ~4.5 kbps, with envelope/pitch preservation pinned by
      tests
* [x] container streaming — §25 all green (fragmented writing,
      incremental parsing, live append, missing trailer, reconstruction)
* [x] basic pipeline — §26 all green (nodes, negotiation, backpressure,
      executors, realtime profile + integration test)

Before 0.3:

* [x] Aura prototype — §30: lossless (LPC + Exp-Golomb + CRC trailer)
      and lossy (MDCT + Bark masking + noise shaping) paths with
      bit-exact lossless round trips, benchmarks (§41), and the §36
      corpus property tests
* [x] Thrum prototype — §32: the I/P hybrid video codec with intra
      refresh, rate-aware skip, real Rice coding, and the deblocked
      reference loop; 45 unit + 6 property + §41/§42 tests, CLI codec
      table wiring, measured latency/rate/memory in
      `docs/performance.md`, design + provenance records (2026-09-28)
* [x] WASM decoder — `apps/tpt-web` (todo.md §45, 2026-09-30; browser-verified
      2026-10-01): builds for `wasm32-unknown-unknown` (328 KiB), 18
      `extern "C"` exports over integer handles, a bounded incremental `.tpt`
      parser, hard memory limits, and a WebAudio/canvas player. 49 native
      tests, 11 Node checks against the built wasm, and 29 checks in a real
      headless browser. Audited 2026-10-01: this line was last touched
      2026-09-07, before any of it existed
* [ ] stable container specification — **deliberately left unchecked, and the
      distinction is the point.** The specification *exists* and is verified
      against the implementation: `docs/container-design.md` is field-by-field
      for the header, `TRAK`/`TMTA`/`CLUS`/`IDX `/`CRC1`, seek and streaming
      behaviour (todo.md §49's item is checked), and the header already carries
      a major/minor version with unknown chunks skipped by design. What is
      *not* done is the commitment: CONTRIBUTING §9-§11 gives no pre-1.0
      stability guarantee, so the format may still break. Audited 2026-10-01 —
      this line and its neighbours are all from 2026-09-07 and were never
      revisited after the spec landed.
* [x] benchmark suite — every implemented codec has a criterion bench
      with its numbers recorded in `docs/performance.md`: the audio
      pair (2026-09-14), Reed and Thrum (2026-09-28), and the
      Vault/Mote-era benches (2026-09-30, own sections) that this gate
      was waiting on. CI-side recording stays deliberately out —
      CONTRIBUTING §11 keeps benches off CI (shared runners are too
      noisy) and names the release-time refresh as the ritual instead

Before 0.4:

* [x] Vault prototype — `codecs/vault` v0.1, 2026-09-30 (§33): 64×64
      superblocks partitioned to 8×8, half-pel motion, up to 16 references,
      2D DCT 4×4–32×32, static Rice, an H.263 Annex J loop filter, RDO on
      `J = D + λR`, versioned bitstream with a CRC-32 trailer. Driven by
      `tpt-cli` (`--codec vault`) and measured over the §36 corpus. Audited
      2026-10-01: this line was last touched 2026-09-07, before the crate
      existed
* [x] Mote prototype — `codecs/mote` v0.1, 2026-09-30 (§34)
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

* [x] no duplicated DSP implementations — Cinder/Aura/Reed consume
      `tpt-dsp` for MDCT/LPC/psychoacoustics/noise shaping (§3); the
      codecs' own code (entropy coding, packetization, band mapping)
      is codec logic, not DSP
* [x] no codec-specific DSP in `tpt-dsp` — verified §3.1: its public
      API is entirely generic, no Cinder/Aura/Reed-specific types
* [x] no media/container abstractions in `tpt-dsp` — §3.1: no
      `tpt-media` dependency anywhere in `tpt-dsp`'s manifests
* [x] no cyclic dependencies — cargo forbids workspace cycles by
      construction; the declared graph (core/bitstream/codec →
      container → pipeline → apps, probe → container, everything on
      tpt-dsp) builds clean
* [ ] real-time paths are allocation-free — Cinder's hot path is
      *bounded*-allocation (§42 test), not strictly zero; the strict
      reading stays open until caller-buffer APIs exist
* [x] malformed media cannot cause unbounded resource consumption —
      §19 limits + §47 audits + §38 sweeps + §42 hostile-input memory
      tests, with the three real overflow/OOB/bomb bugs they found
      fixed (2026-09-11/14)
* [x] native codecs have explicit versioned specifications — 2026-09-29
      for the first four, extended 2026-10-05 to all six implemented
      codecs: every wire layout is specified field-by-field with its
      version in `docs/codec-design.md` ("Cinder v1 — wire format",
      "Aura v1 — wire format", the Reed/Thrum design sections, and the
      Vault/Mote design sections with their wire tables and provable
      bounds), the Bitstream Versioning section carries the
      crate/`VERSION`/layout/integrity table for all six, and the
      change discipline (version bump + table + provenance in the same
      change) is stated there
* [ ] `.tpt` is independently implementable
* [x] public APIs have documented contracts — 2026-10-05, measured rather
      than assumed: a `-W missing_docs` sweep found 187 undocumented
      public items across the workspace (the newest codecs were at
      zero — recent sessions held the standard — while the older
      foundation crates predated it). Every one is now documented, and
      `#![warn(missing_docs)]` is on in all seventeen library crates,
      so the CI clippy gate (`-D warnings`) fails any future
      undocumented public item instead of trusting review. `cargo doc
      --workspace` is zero-warning (the sweep also surfaced four
      broken/private intra-doc links and two container workspace
      declarations whose `default-features` was silently ignored —
      both fixed), and the contracts themselves follow CONTRIBUTING
      §4: panics, errors, complexity, and real-time/allocation
      behavior on the codec surfaces
* [x] numerical algorithms have documented conventions — §12 in
      `tpt-dsp` (all 8 families carry `# Numerical contract` +
      `# Panics` sections, audited)
* [x] optimized implementations have reference implementations — every
      fast path in `tpt-dsp` (`FastMdctPlan`, `FastDctIvPlan`,
      accelerated VQ, SIMD) is tested against a direct scalar reference
      (§5.1, §5.2, §8, §13)
* [x] provenance is recorded — 2026-09-29: complete per-codec
      provenance tables for all four implemented codecs in
      `docs/provenance.md` (§48), maintained alongside algorithm
      changes (§49)
* [ ] security testing is continuous — CI runs the deterministic
      sweeps today; "continuous" in the §38/§47 sense (long-running
      coverage-guided fuzzing) is still pre-1.0 work
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
entropy coding within that paradigm. Thrum/Vault/Mote (§32-34) already
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

## Compatibility with Vault/Mote (§33-34)

* [x] no code changes to Vault/Mote needed yet — both now have implementations on disk (`codecs/vault`, `codecs/mote`), but neither has been expressed as a `Representation` yet. Recorded intent only: Vault's eventual prediction → partition → transform → quantisation → entropy pipeline and Mote's eventual patches → dictionary → VQ → temporal-prediction pipeline should each become expressible as one `Representation` impl, without forcing either codec's internals to look identical to the other

One thing I particularly like about this arrangement is that if you later decide to build something like RF codecs, scientific signal codecs, telemetry compression, or robotics signal processing, the DSP work we've done for media isn't trapped inside tpt-media; it becomes part of the reusable TPT foundation.

# 57. Codec Renames (2026-09-28)

Four of the six native codec names collided with well-known existing
products in the same domain and were renamed project-wide (crate
directories/packages, `CodecId` 4CCs, types, CLI strings, docs):

* [x] `Vox` (§31, audio, edge/IoT speech) → **Reed** — collided with the
  Dialogic/OKI ADPCM ".vox" telephony codec. 4CC `VOX ` → `REED`.
* [x] `Pulse` (§29, audio, real-time low-latency) → **Cinder** — collided
  with PulseAudio and the "pulse-code modulation" (PCM) term. 4CC `PULS` →
  `CIND`.
* [x] `Livelink` (§32, video, real-time communication) → **Thrum** —
  collided with Unreal Engine's Live Link real-time animation/mocap
  plugin. 4CC `LIVE` → `THRM`.
* [x] `Whisper` (§34, video, edge/IoT, not started) → **Mote** — collided
  with OpenAI Whisper, especially confusing since this codec is video, not
  audio. 4CC `WHSP` → `MOTE`.

`Aura` and `Vault` had no meaningful collision and were kept as-is. Earlier
sections in this file (§29, §31-34 and cross-references elsewhere) were
updated in place to use the new names; this entry is the historical record
of why and when the rename happened.