# Architecture Design Notes

This document captures design decisions and tradeoffs for the architectural
elements of `tpt-media`. The high-level overview is in [`ARCHITECTURE.md`](../ARCHITECTURE.md).

## 1. Why a separate bitstream layer?

`tpt-media-bitstream` exists because bitstream I/O and entropy coding are
fundamental to every codec but they are **not** signal processing — they
operate on bit patterns, not samples. Keeping them out of `tpt-dsp` enforces
the dependency direction and lets multiple codecs share a single
implementation.

## 2. Why `u64` accumulators?

The bitstream reader and writer use 64-bit accumulators for two reasons:

1. A single read of up to 57 bits (leaving 7 bits of headroom for the next
   byte) avoids cross-byte boundary handling in the hot loop.
2. On 64-bit platforms, a `u64` is a native register type and is faster
   than slicing the buffer byte-by-byte.

For 32-bit targets, the same code works at the cost of an extra shift; we
do not yet have a measured 32-bit target in CI but the API is
platform-agnostic.

## 3. Why no `f64` timestamps?

Floating-point timestamps drift under long durations and are notoriously
hard to round-trip across machines. Rational `num/den` timebases give
us exact arithmetic for any duration where the timebase values fit in
`i64`. Conversions to `f64` are only performed at the API boundary (e.g.,
when the user asks for seconds).

## 4. Why TLV chunks in `.tpt`?

EBML/Matroska uses variable-length integers to encode chunk lengths
inline, which makes parsing fuzz-hostile (a single truncated length
causes arbitrary look-ahead). A fixed-format TLV chunk header is easier
to bounds-check and easier to extend: unknown chunk IDs are skipped
without committing to a parse.

## 5. Pipeline threading

The default pipeline is single-threaded. Each node has an explicit
`process_one()` method that advances exactly one piece of work. A
multi-threaded executor that owns a set of nodes is a separate concern
(see `tpt-media-pipeline::executor`) and is **not** required for
correctness — only for throughput. Real-time codecs avoid the executor
entirely and call `process_one()` directly from the audio thread.

## 6. Why a separate FFI crate?

`ffi/tpt-media-sys` is the only crate allowed to use `unsafe`. By
isolating `unsafe` to one boundary we can audit the entire surface of
`tpt-media` as memory-safe Rust. The `cbindgen`-generated header is
considered a stable ABI artifact; the safe-Rust APIs above it can
evolve freely.

## 7. Why `no_std` for `vox` only?

Vox targets 8/16 kHz speech on microcontrollers. The other codecs are
either desktop-class (aura, vault, whisper) or have non-trivial
allocation patterns on real-time paths (pulse, livelink) that benefit
from `std`. `vox` uses fixed-point i16/i32 arithmetic and caller-owned
buffers, which makes it cleanly `no_std`-compatible.