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

codecs/    ← native TPT codecs, each implementing tpt-media-codec
  pulse    ← real-time audio
  aura     ← high-fidelity / archival audio
  vox      ← edge / IoT speech (no_std)
  livelink ← real-time video
  vault    ← archival video
  whisper  ← extreme-edge video

containers/
  tpt-container — the native .tpt format (implements tpt-media-container)

apps/
  tpt-cli — encode/decode/probe/inspect/benchmark CLI
  tpt-web — WASM browser player

ffi/
  tpt-media-sys — stable C ABI
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
- `MediaType` — `Audio` or `Video`.
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

`tpt-media-codec` defines:

```rust
pub trait Decoder {
    type Frame;
    fn decode(&mut self, packet: &Packet) -> Result<Self::Frame, DecodeError>;
    fn flush(&mut self) -> Result<(), DecodeError>;
}

pub trait Encoder {
    type Frame;
    fn encode(&mut self, frame: &Self::Frame) -> Result<Packet, EncodeError>;
    fn flush(&mut self) -> Result<Packet, EncodeError>;
}
```

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
FileHeader:    magic "TPTM", version, header_size, timebase
TRAK chunks:   track metadata + codec private data
CLUS chunks:   timestamped compressed data, with keyframe flag
```

Unknown chunk types are skipped. Truncated clusters are recovered where
possible. Seeking scans for `CLUS` chunks where the keyframe flag is set.

A second, generic `Container`/`Demuxer`/`Muxer` trait hierarchy in
`tpt-media-container` allows additional container formats to be plugged in
without modifying the codecs.

## 8. Pipeline

`tpt-media-pipeline` models media processing as a directed graph of nodes:

```text
Source → Demux → Decoder → Filter → Encoder → Mux → Sink
```

Each node has a single, documented input/output type. Backpressure is
implemented via bounded ring buffers. Cancellation is cooperative
(`cancel()` on each node). Timestamps flow through the pipeline
unchanged except for timebase conversions at boundary nodes.

## 9. I/O

`tpt-media-io` re-uses `tpt-dsp-io` for PCM and WAV I/O. Container- and
codec-level I/O lives in `apps/tpt-cli` (for the command line) and
`apps/tpt-web` (for the browser).

## 10. WASM and FFI

- `tpt-web` exposes decoders and the `.tpt` parser to JavaScript via
  `wasm-bindgen`.
- `tpt-media-sys` wraps the safe Rust API in `extern "C"` functions and
  uses `cbindgen` to generate `tpt_media.h`.

Both are documented as **stable** at the 1.0 release.

## 11. Real-Time and Determinism

Real-time paths are documented per-API. The default execution profile for
all codecs is **offline**: allocation, parallelism, and RDO are permitted.
The `pulse`, `livelink`, and `vox` codecs additionally support a
**real-time** profile with zero allocation and bounded work.

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
- **0.2** — MDCT/LPC/VQ in place, Pulse + Vox prototypes, container
  streaming, basic pipeline.
- **0.3** — Aura + Livelink prototypes, WASM decoder, stable `.tpt`
  specification, benchmark suite.
- **0.4** — Vault + Whisper prototypes, advanced quality testing,
  security hardening.
- **1.0** — stable public APIs, stable `.tpt` spec, comprehensive fuzzing,
  reproducible benchmarks, FFI/WASM stability.