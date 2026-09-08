# Changelog

All notable changes to `tpt-media` are recorded here. Dates are ISO-8601
(YYYY-MM-DD). Versions follow [Semantic Versioning](https://semver.org/).

The format is based on [Keep a Changelog](https://keepachangelog.com/).

## [Unreleased]

### Added

- Workspace scaffolding: `crates/`, `codecs/`, `containers/` per the v2
  architecture. (`apps/` and `ffi/` are listed in `todo.md` §2 as the
  target structure but have **not** been added yet — they will be
  enabled as their implementations land; a previous version of this
  entry listed them as added, which was inaccurate.)
- `tpt-media-core`: shared media data types (Timestamp, Duration,
  Timebase, Packet, Frame, AudioFrame, VideoFrame, Track, Stream,
  CodecId, CodecConfig, PixelFormat, SampleFormat, ChannelLayout,
  MediaType, Metadata).
- `tpt-media-bitstream`: zero-allocation `BitReader`/`BitWriter`,
  Huffman encoder/decoder, Rice coder, public-domain range coder.
- `tpt-media-codec`: generic `Encoder`/`Decoder`/`Codec` traits,
  resource-limit types, error model, capability discovery.
- `tpt-media-container`: container abstraction over the `.tpt` format.
- `tpt-container`: native `.tpt` container implementation (header,
  TRAK, CLUS, seeking, corruption tolerance, streaming).
- `tpt-media-pipeline` §26: full pipeline core — `Node` abstraction with
  `NodeEvent`/`NodeError` contract (one input consumed, at most one output
  emitted per call), lockstep typed `Chain<L,R>` composition with
  cascading flush (left-stage internal buffers drain into the right stage
  before the right stage flushes its own), `Pipeline` pull/drive with a
  stall-bound backpressure guard, built-in `Source`/`Map`/`Bounded`/
  `Cancellable`/`Sink` nodes, `Timestamped<T>` PTS/DTS/duration
  pass-through, `MediaFormat`/`StreamSpec`/`FormatAware` stream and
  format negotiation, `Executor` threading abstraction (`Inline`, plus
  std-only `ThreadedExecutor` offloading the graph to a worker thread),
  and realtime scheduling (`RealtimeProfile`, `Clock`-injected
  `drive_scheduled`, `LatencyObserver`/`LatencyLog`, `ScheduledRun`).
  Design documented in `docs/pipeline-design.md`. no_std + alloc; 21 unit
  + 1 container integration test (§26, 2026-09-08).
- `tpt-media-io`: PCM/WAV/raw audio I/O.
- `tpt-media-probe`: container and codec introspection.
- `tpt-media-probe` §44: full `.tpt` probing via `TptDemuxer` — container
  version and global timebase, per-track audio/video/subtitle/metadata
  inspection (codec ID + version, sample rate, channel layout,
  dimensions, pixel format, first/last PTS, frame/sample counts, name,
  language, typed `TMTA` metadata), duration (last PTS + duration),
  overall and per-track bitrate estimates, integrity reporting
  (container `CRC1` footer and per-cluster checksum failures), and codec
  capability reporting against the `tpt-media-codec` `Registry` with the
  native codec family table. `probe_file` reads from disk (`std`).
  Dependency edge inverted: `tpt-container` no longer depends on
  `tpt-media-probe` (was unused), so probe → container (§44, §52,
  2026-09-08).
- `tpt-audio-pulse`, `tpt-audio-aura`, `tpt-audio-vox`: native audio
  codec implementations. (`tpt-video-livelink`, `tpt-video-vault`,
  `tpt-video-whisper` are listed in `todo.md` §2 as the target
  structure but have **not** been added yet.)
- Documentation: `README.md`, `CONTRIBUTING.md`, `ARCHITECTURE.md`,
  `SECURITY.md`, `docs/architecture.md`, `docs/codec-design.md`,
  `docs/container-design.md`, `docs/pipeline-design.md`,
  `docs/performance.md`, `docs/security.md`, `docs/provenance.md`.
- `tpt-media-representation`: representation research contracts —
  `Representation`/`Quantizer`/`RateEstimator`/`DistortionMetric`,
  spatiotemporal `SignalView`/`SignalBuffer` (`V(x,y,t)`),
  MSE/distortion/sparsity diagnostics.
- `tpt-media-core`: `Track::media` gains optional `TrackMedia` parameters
  (sample rate, channel layout, width/height, pixel format); `ChannelMask`
  gains `from_bits` for deserializers.
- `tpt-container` §20–25 (container completeness, 2026-09-08):
  - Header: 20-byte layout with a reserved `u32`; real version checking
    (`UnsupportedVersion` for version 0 / future major / future minor);
    documented compatibility rules; 16-byte version 0.1 headers still parse.
  - `TRAK`: full track descriptor — media type, codec version, per-track
    timebase, sample rate, channel layout, dimensions, pixel format, name,
    language. New `TMTA` chunk for typed metadata pairs.
  - `CLUS`: distinct `PTS`/`DTS`/`duration`, dedicated `DISCONTINUITY` flag,
    optional per-cluster CRC-32 (flags bit 4).
  - Index: `IDX ` per-track chunk with absolute offsets and a version byte;
    unknown index versions are ignored (recovery); `seek_to_keyframe_on_track`
    and `seek_to_offset`; `rebuild_index()` and `reconstruct_trailer()`.
  - Robustness: incomplete-final-cluster handling, reserved/unknown chunk
    skipping, container `CRC1` footer, append mode, live-stream mode, and a
    deterministic fuzz-style corruption/truncation sweep that cannot panic.
  - Streaming: fragmented/clustered writing, incremental parsing via
    `new_streaming` + `feed()`, live append, missing-final-index playback,
    graceful termination via the `finalize()` trailer.
- Dependency-free no_std CRC-32 (`crc32` module) for container checksums.
- `tpt-media-research`: §56 representation research platform —
  five concrete representations (`baseline`, `block_dct`, `temporal_dct`,
  `patch_vq`, `motion_aligned`), deterministic entropy estimation and
  synthetic signal generators, the RD/sparsity measurement harness with
  primary/secondary/diagnostic metric hierarchy, `det_rng`, and trained
  VQ codebook construction (`kmeans.rs`).
- Representation experiments: four-condition oracle (moving edge) plus
  Phases 0/2/4/5, asserted for measurement validity (never hypothesis
  direction) and documented, results included, in
  `docs/representation-research.md`.

### Fixed

- `Timestamp::rescale`: the overflow-safe conversion was using the
  wrong factors (`self.timebase.num` twice instead of `target.num`),
  which both produced incorrect results for non-identity rescalings
  *and* masked cases that should overflow as `None`. Math re-derived
  from first principles, factors corrected, and the overflow test
  replaced with one whose inputs actually overflow `i64` (a previous
  `i64::MAX` µs → 90 kHz case fit in `i64` even with the bug, so it
  could not have been exercising the overflow branch).
- `tpt-audio-aura`: doc comment incorrectly described the lossless
  residual coder as Rice coding delegated to `tpt-media-bitstream`;
  the actual code (and v0.1 wire format) uses an in-crate
  `write_exp_golomb`/`read_exp_golomb` over zigzag-mapped residuals.
  Comments now match the code; switching the wire format to Rice
  would be a separate, breaking change and is tracked as the §974
  unchecked item.
- `Timestamp::add`/`sub` (Timestamp ± Duration): silently fell back
  to the unchanged left-hand side on overflow via `unwrap_or(self.ticks)`,
  which produces a result that's neither pre- nor post-overflow
  without any way to tell. Replaced with **signed-duration-aware
  saturating** arithmetic (`i64::MAX`/`i64::MIN` in the direction
  of the signed Duration), plus new fallible
  `Timestamp::checked_add_duration` / `checked_sub_duration` for
  callers that genuinely need to detect overflow.
- `PacketFlags::DISCONTINUITY` was present in `tpt-media-core` but
  undocumented and not round-tripped through the v1 `.tpt` wire
  format (`tpt-container` reserved only bits 0x01 KEYFRAME and
  0x02 END_OF_FRAME). Now: full semantic docs on all three flags,
  `Packet::is_discontinuity()` / `is_end_of_frame()` accessors, and
  bit 0x04 round-trips through both muxer and demuxer.

### Added

- `tpt-media-core::sync`: a `no_std`-friendly clock-domain /
  synchronization module per `todo.md` §20 — `ClockDomain`
  (named `(String, Timebase)`, equality by name only),
  `SyncPoint` (a cross-domain pin), `SyncMap` (linear-interpolation
  translator with explicit *no extrapolation* to prevent A/V
  drift), and `SyncError`. Includes a realistic
  audio-video sync end-to-end test (48 kHz audio × 90 kHz video
  with two pins at t=0 and t=60 s).

## [0.0.0] — 2026-09-04

### Removed

- v1 workspace scaffolding deleted in favour of the v2 architectural plan.

## [0.1.0] — 2026-09-04

### Added

- Initial workspace scaffolding for the v1 plan (`tpt-core`,
  `tpt-audio-pulse`, `tpt-audio-aura`, `tpt-audio-vox`,
  `tpt-video-livelink`, `tpt-video-vault`, `tpt-video-whisper`,
  `tpt-cli`, `tpt-web`, `tpt-media-sys`).
- CI workflow (`fmt + clippy + test`, plus `no_std` vox build).
- Release workflow (Linux x86_64, macOS x86_64, macOS aarch64,
  Windows x86_64).

[Unreleased]: https://github.com/TPT-Solutions/tpt-media/compare/v0.0.0...HEAD
[0.0.0]: https://github.com/TPT-Solutions/tpt-media/releases/tag/v0.0.0
[0.1.0]: https://github.com/TPT-Solutions/tpt-media/releases/tag/v0.1.0