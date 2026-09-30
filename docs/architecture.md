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

The workspace builds for a 32-bit bare-metal target in CI
(`thumbv7em-none-eabi`, via `tpt-audio-reed`), but performance
*measurement* is 64-bit only; the API itself is platform-agnostic.

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

The default pipeline is single-threaded and deterministic: nodes
implement `process()` (one input consumed, at most one output per call)
and are driven by `Chain`/`Pipeline::pull`. A std-only
`ThreadedExecutor` exists for throughput, but it is not required for
correctness. Real-time consumers avoid the executor entirely and call
node methods directly from their own threads (see
[`pipeline-design.md`](pipeline-design.md) for the full model).

## 6. Why a separate FFI crate?

`ffi/tpt-media-sys` (todo.md §46) is the only crate that needs `unsafe` in
this workspace. Isolating `unsafe` to one boundary keeps the entire surface of
`tpt-media` auditable as
memory-safe Rust, which is already true today: the workspace is safe
Rust throughout. The `cbindgen`-generated header would be considered a
stable ABI artifact; the safe-Rust APIs above it can evolve freely.

## 7. Why `no_std` for `reed` only?

Reed targets 8/16 kHz speech on microcontrollers. The other codecs are
either desktop-class (aura, vault, mote) or have non-trivial
allocation patterns on real-time paths (cinder, thrum) that benefit
from `std`. Reed builds `no_std` + `alloc` with `f64` math via `libm`
(it is *not* a fixed-point implementation — a Q-format port is future
work, todo.md §31), which is what makes it cleanly
`no_std`-compatible.