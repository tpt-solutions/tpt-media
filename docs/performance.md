# Performance Guide

Performance work in `tpt-media` follows three rules:

1. **Measure first.** No optimization without a benchmark.
2. **Scalar reference first.** Every optimized kernel ships with a scalar
   reference that the SIMD version must match within a documented
   tolerance.
3. **Real-time paths are sacred.** No allocation, no FMA ordering
   tricks that change results.

## Real-Time Profile

APIs marked `// SAFETY: real-time-safe` must:

- Not allocate (no `Vec::new`, no `Box::new`, no `String`).
- Not block on I/O or locks.
- Have bounded execution time.
- Use caller-owned buffers.

The following APIs are real-time-safe in `0.1.x`:

- `tpt-media-bitstream::BitReader` (pure, allocation-free)
- `tpt-media-bitstream::BitWriter` (allocation-free when given an
  external buffer)
- `tpt-media-codec::Decoder::decode` for codecs in real-time mode
  (pulse, livelink, vox)
- `tpt-audio-pulse::Decoder::decode`
- `tpt-audio-vox::Decoder::decode`

## Offline Profile

Offline APIs may allocate, parallelize, and perform expensive RDO. The
encoder side of `aura`, `vault`, and `whisper` is offline-only.

## Benchmark Suite

We use `criterion` for benchmarks. The benchmark suite covers:

- MDCT / IMDCT (sizes 128, 256, 512, 1024, 2048, 4096)
- FFT (powers of 2 from 64 to 16384)
- DCT (sizes 4, 8, 16, 32, 64)
- LPC (orders 8, 16, 32)
- VQ search (codebooks 64, 256, 1024, 4096)
- Psychoacoustics (Bark, masking)
- Entropy coding (Huffman, Rice, range)
- Each codec's encode and decode loops
- Container mux and demux
- Pipeline throughput

**Where they live (2026-09-14):** the first seven bullet groups are
benched in the `tpt-dsp` repo (`tpt-dsp-core/benches/dsp_bench.rs`,
results in `tpt-dsp/BENCHMARKS.md`) — transforms and DSP primitives are
`tpt-dsp`'s responsibility (todo.md §3). The last three are benched in
this repo:

| crate | bench | what it measures |
| --- | --- | --- |
| `tpt-audio-pulse` | `benches/codec_bench.rs` | encode/decode of 64×512-sample blocks @ 48 kHz |
| `tpt-audio-aura` | `benches/codec_bench.rs` | lossless encode/decode of 32×1024-sample blocks @ 48 kHz |
| `tpt-audio-vox` | `benches/codec_bench.rs` | encode/decode of 64×320-sample (20 ms) frames @ 16 kHz |
| `tpt-container` | `benches/container_bench.rs` | mux + demux of a 200-packet stream |
| `tpt-media-bitstream` | `benches/bitstream_bench.rs` | bit I/O, Rice, range, Huffman at codec batch sizes |
| `tpt-media-pipeline` | `benches/pipeline_bench.rs` | `Source→map→Sink` drive over 1 000 packets |

Additionally, `tpt benchmark <in.wav> --codec <name>` (todo.md §43)
measures end-to-end encode/decode wall time and §37 quality (SNR +
log-spectral distance) on a real WAV, and reports a real-time factor.
`cargo bench` (release profile) is the source for the tables below; the
CLI's numbers from a debug build are ~40× slower and are not comparable.

### Measured results (2026-09-14; Vox added 2026-09-28)

`cargo bench`, criterion (20 samples, 300 ms warm-up, 2 s measurement),
Windows x64 dev machine, release profile. Numbers are machine-relative —
compare only against other numbers from this table, and re-run on the
same machine when judging a change.

| bench | median | per-item | realtime factor |
| --- | ---: | ---: | ---: |
| `pulse/encode` (64×512 samples = 0.68 s audio) | 604 µs | 9.4 µs/block | ≥1000× |
| `pulse/decode` (64 packets) | 676 µs | 10.6 µs/packet | ≥1000× |
| `aura_lossless/encode` (32×1024 samples = 0.68 s audio) | 1.80 ms | 56 µs/block | ~380× |
| `aura_lossless/decode` (32 packets) | 2.37 ms | 74 µs/packet | ~290× |
| `vox/encode` (64×320 samples = 1.28 s audio, alternating voiced/noise) | 17.0 ms | 266 µs/frame | ~75× |
| `vox/decode` (64 packets) | 375 µs | 5.9 µs/packet | ~3400× |
| `tpt_container/mux` (200 packets) | 84 µs | 0.42 µs/packet | — |
| `tpt_container/demux` (200 packets) | 22 µs | 0.11 µs/packet | — |
| `bitstream/bit_write` (1024 ops) | 2.6 µs | 2.5 ns/op | — |
| `bitstream/bit_read` (1024 ops) | 3.9 µs | 3.9 ns/op | — |
| `bitstream/rice_encode` (256 values, k=2) | 232 µs | 0.9 µs/value | — |
| `bitstream/rice_decode` (256 values, k=2) | 587 µs | 2.3 µs/value | — |
| `bitstream/range_encode` (256 symbols, skewed) | 0.59 µs | 2.3 ns/symbol | — |
| `bitstream/range_decode` (256 symbols, peek+update) | 0.85 µs | 3.3 ns/symbol | — |
| `bitstream/huffman_encode` (512 symbols) | 2.4 µs | 4.7 ns/symbol | — |
| `bitstream/huffman_decode` (512 symbols) | 11.0 µs | 21 ns/symbol | — |
| `pipeline/drive` (1000 packets, source→map→sink) | 60 µs | 60 ns/packet | — |

Readings worth recording honestly:

* **Pulse clears its real-time frame budget by ~3 orders of magnitude**
  (encode+decode ≈ 20 µs against a 5.3 ms frame budget at 256/48 kHz) —
  consistent with §5.2's ≤10% target and the FFT-backed MDCT switch.
* **Aura lossless decode is ~4× more expensive than Pulse decode** —
  expected: LPC synthesis + Exp-Golomb per residual vs. band-quantized
  MDCT coefficients. Aura's §30 profile is archival, not real-time.
* **Vox encode is dominated by the unvoiced shape-gain codebook search**
  (1024 candidate excitations regenerated and correlated per noise
  frame; the 266 µs/frame median is over a half-voiced/half-noise bench
  input — pure voiced frames skip the search entirely). Decode is one
  codebook-entry regeneration + LPC synthesis per frame: 5.9 µs against
  a 20 ms frame budget. The encoder deliberately does *not* claim
  real-time safety (`Capabilities.real_time_encoder` is false); the
  decoder does, and `codecs/vox/tests/realtime_alloc.rs` pins its
  bounded allocation. `--quality` (Vox's LSF bit depth) trades envelope
  fidelity against packet size only — search cost is
  voicing-dependent, not quality-dependent.
* **Rice decode is ~5× slower than range decode here** — Rice's unary
  inner loop dominates for k=2-magnitude residuals; the range coder's
  peek+update is branch-free per symbol. If Rice ever becomes a hot path
  (Aura §30's unchecked "Rice coding" item), budget for a prefix-summed
  decode table.
* Huffman/Rice/Range relative standings match `tpt-dsp`'s entropy
  notes; nothing here contradicts them.

### Allocations

The per-frame allocation *counts* for Pulse's hot path are asserted
bounded by `codecs/pulse/tests/realtime_alloc.rs` (a counting global
allocator over 31 steady-state encode+decode passes, budget 512 KiB —
~25× the measured working set), and Vox's by
`codecs/vox/tests/realtime_alloc.rs` (same harness; 32 steady-state
passes over alternating voiced/noise frames, same 512 KiB bound — the
codebook search regenerates entries into one fixed scratch frame rather
than allocating per candidate). Peak-memory measurement for Aura and the
container remains `criterion`-profiler work, not yet scripted.

## SIMD Equivalence (todo.md §40)

All optimized kernels in this ecosystem live in `tpt-dsp` (§3), so the
equivalence machinery lives there too — this section records where it
stands rather than duplicating it:

- A scalar reference (`simd_scalar.rs`) backs every `core::simd` kernel.
- Equivalence tests (`simd_mul_matches_scalar`, `simd_add_matches_scalar`,
  `simd_magnitude_matches_scalar`, `simd_fft_matches_scalar_fft`)
  compare the paths on identical inputs.
- Tolerances are documented per kernel family in each module's
  `# Numerical contract` section (`tpt-dsp` §12), e.g. ~1e-3/1e-4 in
  `f32` for the FFT-backed transforms.
- Randomized testing beyond the fixed equivalence vectors is the main
  remaining gap (the §40 item stays honest); `tpt-media`'s own
  seeded-random property tests (§38) exercise the *composed* scalar
  paths end-to-end.
- Architecture-specific CI (x86_64 + AArch64 matrix) is likewise still
  open; today's CI runs x86_64 Linux/Windows only.

## Determinism (todo.md §39)

* **Decoding is deterministic.** Both native codecs are pure functions
  of (packet bytes, config): no RNG, no wall-clock, no thread state.
  Pinned by `encoding_and_decoding_are_deterministic` in
  `codecs/{pulse,aura}/tests/property.rs` (byte-identical packets and
  decodes from independent encoder/decoder instances).
* **Container generation is deterministic.** The muxer writes no
  timestamps, counters, or padding entropy; the same packet stream
  produces byte-identical files (`muxing_is_deterministic`,
  `containers/tpt-container/tests/property.rs`).
* **Scalar DSP is deterministic** by construction in `tpt-dsp` (pure
  functions over fixed iteration orders); the codec-level determinism
  tests above pin that end-to-end at the media layer.
* **SIMD differences** are build-time, not runtime: `core::simd`
  kernels dispatch by `target-feature` at compile time (see
  `tpt-dsp/ARCHITECTURE.md` §10 for the support matrix and why runtime
  detection is deliberately absent). Same-binary results are
  deterministic; results *across* different `target-cpu` builds may
  differ within the documented per-kernel tolerances.
* **The §36 corpus is deterministic.** `apps/tpt-cli/src/signals.rs`
  generates every signal from closed-form math plus SplitMix64 — same
  seed, same bytes, no committed binary fixtures needed.

## Reproducibility

Benchmarks are reproducible when run with:

- The same `rustc` version (CI records this).
- The same CPU (CI uses a pinned runner).
- The same `RUSTFLAGS` and `cargo` flags.
- The same input data (committed to `tests/corpus/`).
- The same criterion invocation: the committed benches fix
  `sample_size(20)`, `warm_up_time(300ms)`, `measurement_time(2s)` in
  code, so plain `cargo bench` reproduces the configuration — no flags
  to remember.

We do not claim absolute cross-CPU reproducibility for SIMD paths;
results are tagged with the target architecture.

## Memory Budgets

Default memory budgets per codec:

| Codec  | Per-frame scratch | Reference frames | Total working set |
| ------ | ----------------- | ---------------- | ----------------- |
| Pulse  | 4 KiB             | 0                | < 32 KiB          |
| Aura   | 64 KiB            | 0                | < 256 KiB         |
| Vox    | 1 KiB             | 0                | < 8 KiB           |
| Livelink | 64 KiB          | 1                | < 1 MiB           |
| Vault  | 4 MiB             | 16               | < 128 MiB         |
| Whisper | 16 KiB           | 0                | < 64 KiB          |

These are starting points; actual numbers are measured and recorded
in each codec's `PROVENANCE.md`.

## Profiling

- `cargo flamegraph` for coarse profiling.
- `perf stat` for hardware counters on Linux.
- `Instruments` on macOS.
- `VTune` on Windows / Linux.

A profiling recipe for each codec is recorded in its README.