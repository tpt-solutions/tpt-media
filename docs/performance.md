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
  (cinder, thrum, reed)
- `tpt-audio-cinder::Decoder::decode`
- `tpt-audio-reed::Decoder::decode`

## Offline Profile

Offline APIs may allocate, parallelize, and perform expensive RDO. The
encoder side of `aura`, `vault`, and `mote` is offline-only.

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
| `tpt-audio-cinder` | `benches/codec_bench.rs` | encode/decode of 64×512-sample blocks @ 48 kHz |
| `tpt-audio-aura` | `benches/codec_bench.rs` | lossless encode/decode of 32×1024-sample blocks @ 48 kHz |
| `tpt-audio-reed` | `benches/codec_bench.rs` | encode/decode of 64×320-sample (20 ms) frames @ 16 kHz |
| `tpt-video-thrum` | `benches/codec_bench.rs` | I-frame/P-frame encode + decode of 8-frame clips, 176×144 4:2:0 and 640×480 mono |
| `tpt-container` | `benches/container_bench.rs` | mux + demux of a 200-packet stream |
| `tpt-media-bitstream` | `benches/bitstream_bench.rs` | bit I/O, Rice, range, Huffman at codec batch sizes |
| `tpt-media-pipeline` | `benches/pipeline_bench.rs` | `Source→map→Sink` drive over 1 000 packets |

Additionally, `tpt benchmark <in.wav> --codec <name>` (todo.md §43)
measures end-to-end encode/decode wall time and §37 quality (SNR +
log-spectral distance) on a real WAV, and reports a real-time factor.
`cargo bench` (release profile) is the source for the tables below; the
CLI's numbers from a debug build are ~40× slower and are not comparable.

### Measured results (2026-09-14; Reed added 2026-09-28; Thrum added 2026-09-28)

`cargo bench`, criterion (20 samples, 300 ms warm-up, 2 s measurement),
Windows x64 dev machine, release profile. Numbers are machine-relative —
compare only against other numbers from this table, and re-run on the
same machine when judging a change.

| bench | median | per-item | realtime factor |
| --- | ---: | ---: | ---: |
| `cinder/encode` (64×512 samples = 0.68 s audio) | 604 µs | 9.4 µs/block | ≥1000× |
| `cinder/decode` (64 packets) | 676 µs | 10.6 µs/packet | ≥1000× |
| `aura_lossless/encode` (32×1024 samples = 0.68 s audio) | 1.80 ms | 56 µs/block | ~380× |
| `aura_lossless/decode` (32 packets) | 2.37 ms | 74 µs/packet | ~290× |
| `reed/encode` (64×320 samples = 1.28 s audio, alternating voiced/noise) | 17.0 ms | 266 µs/frame | ~75× |
| `reed/decode` (64 packets) | 375 µs | 5.9 µs/packet | ~3400× |
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
| `thrum/qcif/encode_i_frame` (8 frames, 176×144 4:2:0) | 46.8 ms | 5.8 ms/frame | ~57× |
| `thrum/qcif/encode_p_frame` (8 frames) | 92.1 ms | 11.5 ms/frame | ~29× |
| `thrum/qcif/decode` (8 packets) | 15.1 ms | 1.9 ms/packet | ~530× |
| `thrum/vga/encode_i_frame` (8 frames, 640×480 mono) | 377 ms | 47.1 ms/frame | ~6.8× |
| `thrum/vga/encode_p_frame` (8 frames) | 959 ms | 120 ms/frame | ~2.7× |
| `thrum/vga/decode` (8 packets) | 84.3 ms | 10.5 ms/packet | ~95× |

Readings worth recording honestly:

* **Cinder clears its real-time frame budget by ~3 orders of magnitude**
  (encode+decode ≈ 20 µs against a 5.3 ms frame budget at 256/48 kHz) —
  consistent with §5.2's ≤10% target and the FFT-backed MDCT switch.
* **Aura lossless decode is ~4× more expensive than Cinder decode** —
  expected: LPC synthesis + Exp-Golomb per residual vs. band-quantized
  MDCT coefficients. Aura's §30 profile is archival, not real-time.
* **Reed encode is dominated by the unvoiced shape-gain codebook search**
  (1024 candidate excitations regenerated and correlated per noise
  frame; the 266 µs/frame median is over a half-voiced/half-noise bench
  input — pure voiced frames skip the search entirely). Decode is one
  codebook-entry regeneration + LPC synthesis per frame: 5.9 µs against
  a 20 ms frame budget. The encoder deliberately does *not* claim
  real-time safety (`Capabilities.real_time_encoder` is false); the
  decoder does, and `codecs/reed/tests/realtime_alloc.rs` pins its
  bounded allocation. `--quality` (Reed's LSF bit depth) trades envelope
  fidelity against packet size only — search cost is
  voicing-dependent, not quality-dependent.
* **Rice decode is ~5× slower than range decode here** — Rice's unary
  inner loop dominates for k=2-magnitude residuals; the range coder's
  peek+update is branch-free per symbol. If Rice ever becomes a hot path
  (Aura §30's unchecked "Rice coding" item), budget for a prefix-summed
  decode table.
* Huffman/Rice/Range relative standings match `tpt-dsp`'s entropy
  notes; nothing here contradicts them.
* **Thrum P-frame encoding is ~2× the cost of an I-frame**, and the
  exhaustive motion search is why: every macroblock costs
  `(2·search_range + 1)²` full-pixel SAD evaluations (289 at the default
  `search_range = 8`) on top of the intra path's own mode search. That
  is the deliberate trade in §32 — bounded work and no sub-pel
  interpolation, bought with encoder CPU. The **decoder is ~6× cheaper
  than the encoder** (1.9 ms vs 11.5 ms/frame at QCIF), which is the
  asymmetry that matters: only the decoder is claimed real-time
  (`Capabilities.real_time_encoder` is false, exactly as for Reed).
* **Thrum's latency figure is the P-frame encode + decode sum**, not
  any buffer or reorder delay: the encoder never holds a frame back and
  there is no reordering anywhere in the codec, so at QCIF one-way
  latency is ~13.4 ms against a 33.3 ms frame budget at 30 fps. Decode
  alone is ~1.9 ms. At 640×480 the P-frame encode alone (~120 ms) blows
  a 33.3 ms budget, so this configuration is offline/smaller-frame only
  until the search window shrinks — `search_range` is the knob, and the
  cost is quadratic in it.
* **Mean packet sizes** at the default `qp = 26`, from the bench's own
  synthetic clip (a panning diagonal ramp plus noise, 8 frames so ~1
  I-frame + 7 P-frames): 1 074 B/packet at 176×144 4:2:0 and 4 357 B at
  640×480 mono. These are *bench-input* rates, not a claimed bitrate
  target — a real scene, and especially a real talking head, codes
  considerably smaller, because the skip path and intra refresh both
  key on predicted residual being small.

### Allocations

The per-frame allocation *counts* for Cinder's hot path are asserted
bounded by `codecs/cinder/tests/realtime_alloc.rs` (a counting global
allocator over 31 steady-state encode+decode passes, budget 512 KiB —
~25× the measured working set), and Reed's by
`codecs/reed/tests/realtime_alloc.rs` (same harness; 32 steady-state
passes over alternating voiced/noise frames, same 512 KiB bound — the
codebook search regenerates entries into one fixed scratch frame rather
than allocating per candidate). Thrum's is
`codecs/thrum/tests/realtime_alloc.rs` (same harness, 24 steady-state
encode+decode passes at 176×144 4:2:0, 4 MiB bound — deliberately looser
than the audio budgets because a *video* decoder must return a whole
frame buffer per call, which is inherently ~1.5 bytes per pixel; the
property guarded is still order-of-magnitude stability, not a tight
ceiling). That test is what caught the encoder retaining its
worst-case packet capacity (2.5 MB per packet for a 176×144 frame,
59 MB over 24 packets) instead of the few kilobytes actually written;
the initial `shrink_to_fit` fix was itself superseded — the encoder now
allocates that worst-case buffer once at construction and reuses it for
every frame, emitting an exact-size copy per packet, so the steady-state
path does not allocate it at all.

*Peak* working sets — the high-water mark of live heap bytes, which is
the number a memory budget is stated in — are measured by
`apps/tpt-cli/tests/memory_budget.rs` through `tpt-cli`'s `memprobe`
harness (a `TrackingAlloc` global allocator whose `MemoryProbe` windows
report live-heap peaks and cumulative churn). It measures Cinder, Aura,
Reed, and Thrum in a steady-state streaming pass where nothing is
retained, asserts each against the budget table below, and asserts
Thrum's working set is independent of clip length (8× the input does
not cost 8× the memory). Results: see the "Measured" column below.
The container, pipeline, and probe layers are not yet measured by that
harness; that remains `criterion`-profiler follow-up work.

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
  `codecs/{cinder,aura}/tests/property.rs` (byte-identical packets and
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

Default memory budgets per codec, with the measured steady-state
streaming working set next to each (see "Allocations" above for how the
measurement is taken):

| Codec | Per-frame scratch | Reference frames | Total working set | Measured peak |
| ----- | ----------------- | ---------------- | ----------------- | ------------- |
| Cinder | 4 KiB            | 0                | < 32 KiB          | 6 645 B       |
| Aura   | 64 KiB           | 0                | < 256 KiB         | 25 330 B      |
| Reed   | 1 KiB            | 0                | < 16 KiB          | 8 140 B       |
| Thrum  | 64 KiB           | 1                | < 1 MiB           | 152 503 B (length-independent: 152 200 B over 6 frames vs 152 503 B over 48) |
| Vault  | 4 MiB            | 16               | < 128 MiB         | — (no implementation) |
| Mote   | 16 KiB           | 0                | < 64 KiB          | — (no implementation) |

The budget column is the ceiling each codec's streaming working set must
stay under; the measured column is what the current implementation
actually peaks at, over a steady-state encode+decode pass in which
nothing is retained (`apps/tpt-cli/tests/memory_budget.rs`, x86_64
Windows, `cargo test`'s unoptimized test profile — the numbers are
allocation-structure-dependent, not timing-dependent, so they are
stable across runs; platform struct-size differences can move them
slightly). The same test pins every budget row above to its documented
value, so the table cannot silently drift away from the code.

Two numbers moved because of the measurement, both worth recording:

* Reed's budget was originally "< 8 KiB"; the first measurement came in
  at 8 140 B — 99% of the ceiling, passing by 52 bytes. A budget the
  implementation fills is a flake in waiting, so it was raised to the
  next sane power of two rather than kept as a false precision point.
* Thrum's encoder initially allocated its worst-case packet buffer
  (~2.4 MB for a 176×144 frame) *per frame*; the buffer is now allocated
  once and reused, which is why the measured peak (~149 KiB, one frame
  plus one reference plus the reused scratch) is two orders of
  magnitude below what the per-frame allocation would have produced.

Cumulative per-frame *churn* is a separate figure from the peak and is
deliberately not budgeted to zero: Thrum's measured churn is
~263 KB/frame (exact-size payload copy + the decoded frame the decoder
must hand back), bounded by `thrum_per_frame_churn_is_bounded` to
proportionality with a frame rather than to nothing.

## Profiling

- `cargo flamegraph` for coarse profiling.
- `perf stat` for hardware counters on Linux.
- `Instruments` on macOS.
- `VTune` on Windows / Linux.

A profiling recipe for each crate with benchmarks is recorded in its own
README (`benches/` names the harness, and the recipe says which tool to point
at it); the canonical measured numbers are the tables above.