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

Benchmark output records:

- throughput (items/sec)
- latency (µs / item)
- CPU time
- memory
- allocations (count + bytes)

## SIMD Equivalence

For every optimized DSP kernel:

- A scalar reference implementation exists in the same file.
- A property test compares scalar vs SIMD output for randomized input.
- Tolerance is documented per kernel (e.g., "1 ULP for DCT, 4 ULPs
  for MDCT").

## Reproducibility

Benchmarks are reproducible when run with:

- The same `rustc` version (CI records this).
- The same CPU (CI uses a pinned runner).
- The same `RUSTFLAGS` and `cargo` flags.
- The same input data (committed to `tests/corpus/`).

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