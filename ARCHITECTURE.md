# tpt-media Architecture (v2)

This document is the long-form companion to `README.md` and `todo.md`. For
the master task list, see [`TODO.md`](todo.md); for per-area design notes,
see [`docs/`](docs/).

## 1. Goals and Non-Goals

### Goals

- Provide a reusable native media stack: containers, codecs, bitstreams,
  pipelines, I/O integration, CLI, WASM, FFI.
- Keep the surface small, generic, and documentable.
- Make every part of the stack independently usable — applications can
  depend on the subset they need.
- Be deterministic, bounded-resource, and real-time-safe on documented paths.
- Stay within the clean-room, patent-conscious design envelope.

### Non-Goals

- **Not** a general-purpose DSP library. Transforms, filters, signal
  analysis and SIMD live in `tpt-dsp`.
- **Not** a re-implementation of standards codecs. Those live in
  `tpt-kinetix`.
- **Not** a media player. We ship a player to demonstrate the codecs,
  not to compete with established playback tools.
- **Not** an "all features" framework. We refuse to expose APIs that we
  cannot keep stable, safe, and well-documented.

## 2. Crate Topology

```text
crates/    ← generic, codec-independent primitives
  tpt-media-core       — shared data types
  tpt-media-bitstream  — bit-level I/O + entropy coding
  tpt-media-codec      — generic Encoder/Decoder/Codec traits
  tpt-media-container  — generic container abstraction
  tpt-media-pipeline   — node-based media pipeline
  tpt-media-io         — PCM/WAV/raw I/O
  tpt-media-probe      — introspection
  tpt-media-representation — swappable representation contracts (research, §56)
  tpt-media-research   — representation research harness (§56)

codecs/    ← native TPT codecs, each implementing tpt-media-codec
  cinder    ← real-time audio
  aura     ← high-fidelity / archival audio
  reed      ← edge / IoT speech (no_std)
  thrum ← real-time video            (implemented, todo.md §32)
  vault    ← high-compression video    (implemented, todo.md §33)
  mote  ← extreme-edge video         (implemented, todo.md §34)

containers/
  tpt-container — the native .tpt format (implements tpt-media-container)

apps/
  tpt-cli — encode/decode/transcode/probe/inspect/benchmark CLI
  tpt-web — WASM browser player        (todo.md §45; docs/web-design.md)

ffi/
  tpt-media-sys — stable C ABI         (todo.md §46)
```

## 3. Dependency Direction

```text
tpt-media ─────────► tpt-dsp        (never reversed)
codecs  ──────────► tpt-media-codec ─► tpt-media-core
tpt-cli, tpt-web ─► codecs, tpt-container, tpt-media-*
tpt-media-sys ────► all public APIs
```

Cycles are forbidden. The dependency graph is enforced by `cargo`'s
build system and reviewed in CI.

## 4. Data Model

The shared data model in `tpt-media-core` is deliberately tiny:

- `Timestamp` / `Duration` / `Timebase` — rational time arithmetic
  (`num/den`), no `f64`.
- `Packet` — a chunk of compressed bitstream with PTS/DTS/flags/track id.
- `Frame` / `AudioFrame` / `VideoFrame` — decoded samples/pixels.
- `Track` — codec configuration + timebase + metadata.
- `CodecId` — a stable 4-byte identifier.
- `CodecConfig` — codec-specific serialised configuration.
- `MediaType` — `Audio`, `Video`, `Subtitle`, or `Metadata`.
- `Metadata` — key/value pairs of side data.

All decoders allocate one `Frame` per decoded unit and return it via
`Decoder::decode`; the caller owns the memory. Buffers are caller-owned
on real-time paths.

## 5. Bitstream and Entropy

`tpt-media-bitstream` provides:

- `BitReader` — wraps `&[u8]`, internal `u64` accumulator, reads
  arbitrary bit widths, supports bounded reads, alignment, peek, skip.
- `BitWriter` — wraps `&mut [u8]`, deterministic, bounded.
- `huffman` — canonical Huffman encoder/decoder.
- `rice` — Rice/Golomb coder for Laplacian-distributed residuals.
- `range` — public-domain static range coder.

Entropy coding lives here (not in `tpt-dsp`) because it is part of the
bitstream, not a signal transform.

## 6. Codec Framework

`tpt-media-codec` defines (see the crate docs for the full surface):

```rust
pub type CodecResult<T> = Result<T, CodecError>;

pub trait Decoder {
    type Frame;
    fn info(&self) -> &CodecInfo;
    fn decode(&mut self, packet: &Packet) -> CodecResult<Vec<Self::Frame>>;
    // `flush`/`drain` have defaults; most codecs are stateless enough
    // not to override them.
}

pub trait Encoder {
    type Frame;
    fn info(&self) -> &CodecInfo;
    fn encode(&mut self, frame: &Self::Frame) -> CodecResult<Vec<Packet>>;
}

pub enum CodecError { MalformedPacket, ResourceLimit(ResourceLimitKind),
    Unsupported(&'static str), InvalidConfig(&'static str), /* ... */ }
```

A `CodecInfo` carries the codec name/`CodecId`/bitstream version,
`Capabilities` (lossless/lossy, real-time flags), and a
`SupportedFormat` table validated up front. Type-erased use goes through
`DynEncoder`/`DynDecoder` and the `Registry` (factories keyed by
`CodecId`).

All codecs must support bounded resources (see `todo.md` §19):

- Maximum frame dimensions.
- Maximum packet size.
- Maximum reference frames.
- Recursion depth limit.
- Cancellation.

## 7. Container

The native `.tpt` format is a TLV (Type-Length-Value) chunk-based format
designed for forward compatibility and fuzz resistance:

```text
FileHeader:    magic "TPTM", version, header_size, timebase, reserved
TRAK chunks:   track metadata + codec private data (+ TMTA metadata)
CLUS chunks:   timestamped compressed data, with keyframe/end-of-frame/
               discontinuity flags and an optional per-cluster CRC-32
IDX  chunk:    optional versioned keyframe/offset index (per track)
CRC1 chunk:    container checksum footer written on finalize()
```

Unknown chunk types are skipped. A missing or newer `IDX ` index is
ignored (seeking falls back to a linear keyframe scan, and
`rebuild_index()`/`reconstruct_trailer()` can regenerate it). Truncated
input either errors strictly or, in streaming mode (`new_streaming` +
`feed()`), pauses until more bytes arrive.

A second, generic `Container`/`Demuxer`/`Muxer` trait hierarchy in
`tpt-media-container` allows additional container formats to be plugged in
without modifying the codecs.

## 8. Pipeline

`tpt-media-pipeline` models media processing as a linear, typed chain of
nodes:

```text
Source → Demux → Decoder → Filter → Encoder → Mux → Sink
```

Each node consumes at most one input per `process()` call and emits a
`NodeEvent`. Backpressure is a soft signal (`NodeEvent::NeedMore`) plus a
stall bound in `Pipeline::pull` that reports
`NodeError::Backpressure`; `Bounded` adds batching with a hard cap.
Cancellation is cooperative (`Node::cancel()`, surfaced as
`NodeError::Cancelled`). Timestamps ride along via `Timestamped` and
container `Packet`s flow through unchanged. Execution is `Inline` by
default (deterministic); a std-only `ThreadedExecutor` exists, and the
real-time scheduling profile (`RealtimeProfile`, latency observation) is
documented in `docs/pipeline-design.md`.

## 9. I/O

`tpt-media-io` re-uses `tpt-dsp-io` for its std-gated f32 WAV bridge (the
byte-exact codec-path WAV reader/writer is local). Container- and
codec-level I/O lives in `apps/tpt-cli` and `apps/tpt-web` (browser, fed by
`fetch` chunks through a bounded push parser).

## 10. WASM and FFI

`tpt-web` shipped 2026-09-30 (todo.md §45; design in `docs/web-design.md`).
`tpt-media-sys` shipped its
first slice 2026-09-29 (todo.md §46): the codec abstraction — packet,
frame, encoder, decoder — as opaque, panic-guarded C handles with error
codes, an ABI version, a hand-written kept-in-sync header, and a C
integration test; container/demuxer/muxer handles are the v0.2 surface.

- `tpt-web` exposes a streaming player (incremental `.tpt` parser, the
  Cinder/Aura/Reed/Thrum decoders, planar-`f32` audio and RGBA video out) to
  JavaScript through a plain `extern "C"` surface over integer handles, with
  hand-written JS glue. It does **not** use `wasm-bindgen`: the surface is
  integers and pointers into linear memory, so the dependency, its generated
  glue and its build tool buy nothing (rationale in `docs/web-design.md`).
- `tpt-media-sys` wraps the codec abstraction in `extern "C"` functions;
  the header is hand-written and kept in sync with the ABI by a test
  (deliberately no `cbindgen` dependency).

Both are documented as **stable** at the 1.0 release.

## 11. Real-Time and Determinism

Real-time paths are documented per-API. The default execution profile for
all codecs is **offline**: allocation, parallelism, and RDO are permitted.
Cinder claims real-time encode *and* decode; Reed and Thrum claim
real-time decode only (Reed runs a documented 1024-entry codebook search,
Thrum an exhaustive `(2r+1)^2` motion search, both in their
encoders). "Real-time" here means bounded allocation and bounded work per
frame — not literally zero allocation — and the bounded-allocation
claims are pinned by counting-allocator tests
(`codecs/{cinder,reed,thrum}/tests/realtime_alloc.rs`).

Determinism is required for:

- Decoding (any conformant bitstream must decode identically).
- Scalar DSP (no FMA ordering tricks).
- Container generation (timestamps and offsets are stable).

SIMD differences are documented in `docs/performance.md` and verified by
scalar/SIMD equivalence tests.

## 12. Security

See [`SECURITY.md`](SECURITY.md) and [`docs/security.md`](docs/security.md).
The container parsers, bitstream readers, and codec entry points are the
trust boundary. All of them are fuzz-tested in CI.

## 13. Provenance

See [`docs/provenance.md`](docs/provenance.md) and the per-crate
provenance tables. Every non-trivial algorithm carries a record of its
mathematical source, classification (`GREEN` / `YELLOW` / `RED`), and
last patent review.

## 14. Cross-Repository Integration

`tpt-media` consumes from `tpt-dsp`:

- FFT, DCT, MDCT, IMDCT
- LPC analysis
- Pitch estimation
- VQ (generic, codec-specific codebooks live in the codec crate)
- Psychoacoustic models (Bark, masking)
- Filters, resampling
- SIMD

`tpt-media` does **not** depend on `tpt-kinetix`; they are siblings under
`tpt-dsp`.

## 15. Release Gates

See [`todo.md` §53](todo.md) for the full list. The high-level gates are:

- **0.1** — workspace builds, CI runs, basic media model, one working
  codec.
- **0.2** — MDCT/LPC/VQ in place, Cinder + Reed prototypes, container
  streaming, basic pipeline.
- **0.3** — Aura + Thrum prototypes, WASM decoder, stable `.tpt`
  specification, benchmark suite.
- **0.4** — Vault + Mote prototypes, advanced quality testing,
  security hardening.
- **1.0** — stable public APIs, stable `.tpt` spec, comprehensive fuzzing,
  reproducible benchmarks, FFI/WASM stability.