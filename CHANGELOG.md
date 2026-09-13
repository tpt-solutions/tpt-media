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
- `tpt-media-io` §27: media I/O crate (no_std + alloc; `std` default
  feature). New `IoError` model (UnexpectedEof / InvalidHeader /
  Unsupported / Overflow / Other). PCM: `PcmSpec`, `encode_pcm`/`decode_pcm`
  with chunked monotonic PTS, `normalize_to_f32`/`denormalize_from_f32`
  (f32 domain bit-exact for ≤24-bit integer formats). WAV: byte-exact
  `parse_wav`/`decode_wav`/`write_wav` for the codec pipeline plus a
  std-gated `f32` bridge `decode_wav_f32`/`write_wav_f32` that **reuses
  `tpt-dsp-io`** rather than duplicating it (§27 constraint). Raster frame
  I/O: dependency-free PPM P6 / PGM P5 readers/writers over `VideoFrame`,
  `checkerboard` fixtures, std path helpers. Streaming `FrameSource` +
  `MediaItem` (pull) and `FrameSink` (push) traits with concrete
  implementations over slices/vecs (no_std) and files/stdin/stdout (`std`);
  a stream that ends mid-sample-frame is reported as an error. Async
  integration: `AsyncFrameSource`/`AsyncFrameSink` (RPITIT + `Send`,
  dependency-free) and a sync `BlockingAdapter`. Design:
  `docs/media-io-design.md`. 32 unit tests (§27, 2026-09-08).
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
- `apps/tpt-cli` §43: `tpt` is now a working trans-Q codec CLI.
  `encode`/`decode` are live end-to-end (WAV → `pulse`/`aura` → `.tpt`
  and back): encode reads a 16-bit mono WAV (44.1/48 kHz), chunks and
  zero-pads to codec block sizes (Pulse 512, Aura 1024), muxes with full
  track metadata (timebase, codec version, sample rate, channels,
  `--set-meta` tags) and records the exact sample count under the
  reserved `tpt.io.sample_count` key; decode sniffs the container, picks
  the first audio track, decodes via the track's codec (explicit
  `--codec` must match), trims padding and writes a 16-bit WAV. Binary
  stdout flows through `Outcome.binary_stdout`; `Ctx` gained a
  `write_file` injector. `transcode`/`benchmark` still exit 3 (not wired);
  `vox` exits 3. 27 unit tests incl. a lossless Aura round trip
  (§43, 2026-09-08).
- `tpt-cli` §43: the `tpt` command-line interface (first `apps/`
  workspace member). Commands `probe` (with `--json` deterministic
  output), `inspect`, `list-codecs`, and `--help` are fully implemented;
  `encode`/`decode` were later wired end-to-end (see the `tpt-cli` §43
  entry above); `transcode`/`benchmark` fully validate their config
  (`--codec --container --quality --bitrate --set-meta [--runs]`,
  positionals, `-` for stdin/stdout) then exit 3. Hand-rolled arg parser
  and JSON writer (no clap, no serde). Injectable I/O (`Ctx`) so every
  command is tested in-process without a filesystem. Design:
  `docs/cli-design.md`. 16 unit tests (§43, 2026-09-08).
- Documentation: `README.md`, `CONTRIBUTING.md`, `ARCHITECTURE.md`,
  `SECURITY.md`, `docs/architecture.md`, `docs/codec-design.md`,
  `docs/container-design.md`, `docs/pipeline-design.md`,
  `docs/cli-design.md`, `docs/media-io-design.md`, `docs/performance.md`,
  `docs/security.md`, `docs/provenance.md`.
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
- `tpt-audio-pulse`: the per-packet header carried `frame_size` in an
  8-bit field, so the *default* `frame_size` of 256 overflowed to 0
  and every packet the default encoder produced was rejected as
  `MalformedPacket` by the decoder. The header now writes a 1-bit
  128/256 selector (both sides updated together; existing 128
  packets and the `plc` codec are unaffected). Regression test added.
- `tpt-audio-pulse`: `bits_per_coef` was not part of the bitstream,
  so a decoder configured with default 8 bits mis-parsed packets
  encoded at a different coefficient depth. The CLI now carries it
  (and Aura's `block_size`, which is likewise not self-described) in
  the track's codec-private `data`; defaults still apply when the
  field is absent.

### Added

- `tpt-media-core::sync`: a `no_std`-friendly clock-domain /
  synchronization module per `todo.md` §20 — `ClockDomain`
  (named `(String, Timebase)`, equality by name only),
  `SyncPoint` (a cross-domain pin), `SyncMap` (linear-interpolation
  translator with explicit *no extrapolation* to prevent A/V
  drift), and `SyncError`. Includes a realistic
  audio-video sync end-to-end test (48 kHz audio × 90 kHz video
  with two pins at t=0 and t=60 s).
- `tpt-media-bitstream`: `entropy` module (`shannon_entropy_bits` /
  `shannon_entropy_nats`, `normalize`, `histogram`) wired into `lib.rs`
  (§17, 2026-09-09).
- `tpt-audio-pulse` §29 tests: `plc_conceals_mid_stream_packet_loss`
  (real drop-and-conceal stream scenario — encode a 4-frame tone, drop
  the middle packet, substitute `conceal()`, assert sample-count/PTS
  continuity and non-silent concealed output with clean decode before
  and after the gap) and `jittered_pts_are_preserved_not_reordered`
  (deliver valid packets out of PTS order without panic or
  self-reordering; output frames echo their own packet's PTS)
  (2026-09-09).
- `tpt-audio-aura` §30: per-packet CRC-32 integrity. Every packet now
  ends with a 4-byte CRC-32 (`crc32`, `CRC_TRAILER_BYTES`) over all
  preceding payload bytes; `AuraDecoder` verifies the checksum before
  parsing and rejects a mismatch (or a packet shorter than the trailer)
  as `CodecError::MalformedPacket`. Tests: trailer round-trip,
  body-bit-flip rejection, checksum-bit-flip rejection, undersized
  packet rejection. **Wire-format impact:** this is a v0.1 packet-format
  addition — packets produced before this change carry no trailer and
  are rejected by the new decoder (2026-09-09).
- `tpt-audio-aura` §30 (lossy mode): quantization is now psychoacoustically
  driven instead of a flat 16-bit-per-coefficient scheme. Each MDCT bin is
  mapped to a Bark critical band (`tpt-dsp-core::hz_to_bark`); bands are
  classified tonal/noise (`classify_tonal_bins`) and spread across the Bark
  axis to model simultaneous masking (`simultaneous_masking`,
  `combined_threshold_db`); the resulting per-band masking threshold sets
  that band's quantization step (`step = sqrt(12 * threshold_linear)`,
  clamped), and coefficients are quantized in bin order with first-order
  spectral noise shaping (`noise_shape_step`) so residual quantization
  error is fed forward rather than left independent per bin. The per-band
  step table (one 32-bit float per Bark band) is transmitted so decode
  needs no side information beyond the packet itself; the Bark-band layout
  itself is *not* transmitted — both encoder and decoder derive it
  deterministically from `(sample_rate, n_half)` alone. New tests:
  `mdct_bin_bark_bands_is_deterministic_and_nondecreasing`,
  `lossy_step_table_reflects_masking_not_flat`. **Wire-format impact:**
  this changes the v0.1 lossy packet layout (adds the per-band step table
  before the coefficient array) — lossy packets produced before this
  change are not decodable by the new decoder and vice versa; lossless
  mode is unaffected (2026-09-11).
- `tpt-audio-pulse` §29: optional perceptual weighting.
  `PulseEncoderConfig::perceptual_weighting` (default `false`, existing
  callers unaffected) scales each band's configured quantization step by
  that band's absolute threshold of hearing (`tpt-dsp-core::threshold_in_quiet`)
  relative to a 1kHz reference, computed once at construction time (not
  per frame). Deliberately does not use Aura's full masking model —
  Pulse is real-time/low-complexity by design. Scaling only ever coarsens
  a step, never sharpens below the configured value (the wire format's
  step field is an integer). No wire-format change. Tests:
  `perceptual_weighting_off_by_default_leaves_step_unchanged`,
  `perceptual_weighting_coarsens_less_sensitive_bands_never_below_configured`,
  `perceptual_weighting_round_trip_carries_signal` (2026-09-11).
- `tpt-media-codec`/codecs §19: `ResourceLimits` are now enforced by
  real decoders. `tpt-audio-pulse` checks `check_allocation` (decoded
  spectrum buffer) and `check_frame_bytes` (output) during decode;
  `tpt-audio-aura` checks `check_frame_bytes` and per-mode
  `check_allocation` (lossless LPC coefficient/sample buffers, lossy
  MDCT spec/synthesis buffers) before allocating. Both codecs have a
  test proving a structurally valid packet plus a tight budget yields
  `CodecError::ResourceLimit(Allocation)` (2026-09-09).
- `tpt-media-codec`/§19: new `ResourceLimits::max_metadata_bytes` +
  `ResourceLimitKind::MetadataSize` + `check_metadata`, following the
  existing `check_packet`/`check_allocation` pattern (`permissive()`
  16MiB, `strict()`/`Default` 64KiB). Wired into `tpt-container`:
  `TptDemuxer` gained a `limits: ResourceLimits` field and
  `with_limits`/`new_streaming_with_limits` constructors; `index_tmta`
  now checks a `TMTA` chunk's raw body length against the limit *before*
  parsing it, returning the previously-unused `TptError::ResourceExceeded`
  on violation. Test:
  `oversized_tmta_metadata_is_rejected_by_default_but_allowed_with_permissive_limits`
  (2026-09-11).
- `tpt-audio-aura`/`tpt-audio-pulse` §35: versioned bitstreams. Both
  codecs' packets now lead with a 16-bit wire-format version field (the
  pre-existing `VERSION` constant, actually written to the stream for
  the first time); a decoder that reads a version it doesn't recognize
  returns `CodecError::Unsupported` immediately, before interpreting any
  further bits. **Wire-format impact:** breaking for both codecs —
  packets produced before this change have no version field and are
  misread by the new decoder (and vice versa). This is the last
  un-gracefully-handled breaking change either codec should need: every
  future version bump can now be rejected cleanly instead of silently
  misparsed. Tests: `decode_rejects_unsupported_wire_version` (Aura and
  Pulse). Also strengthened Aura's `lossy_step_table_reflects_masking_not_flat`
  test, which had silently stopped checking real data once this change
  shifted every subsequent bit offset in the packets it hand-parses — it
  now also asserts every recovered step is finite and positive
  (2026-09-11).
- §38 fuzz infrastructure: deterministic, dependency-free
  corruption/truncation sweeps (fixed-seed LCG, no `cargo-fuzz`/libFuzzer,
  same approach as `tpt-container`'s pre-existing §24 sweep) added to
  every remaining crate that parses untrusted input:
  `tpt-media-bitstream::fuzz_like` (`BitReader`, Huffman, Rice, range
  coder), `tpt-audio-aura`/`tpt-audio-pulse`
  (`decode_never_panics_on_corrupted_or_arbitrary_packets`), and
  `tpt-media-probe` (`probe_never_panics_on_corrupted_or_arbitrary_bytes`).
  Found two real bugs on first run — see `Fixed` below (2026-09-11).

### Fixed

- `tpt-media-bitstream`: `BitReader::skip` corrupted every read following
  a multi-byte, non-byte-aligned skip. It correctly computed how many
  whole bytes to advance past, but then reloaded the *wrong* byte (one
  past the skip region) to recover the last skipped byte's unconsumed
  trailing bits, and also spuriously reloaded a byte when the skip landed
  exactly on a byte boundary. Unnoticed until now because the only
  production caller (`tpt-audio-aura`'s `read_exp_golomb`) only ever
  calls `skip(1)` (never the multi-byte path), and the crate's own prior
  `skip_across_bytes` test happened to skip exactly the bits already
  buffered in the accumulator. Surfaced by a new `tpt-audio-pulse` test
  that skips over a real packet's per-band coefficients
  (`skip(n_coeffs * bits_per_coef)`) to inspect the transmitted step
  table. Fixed by rewriting `skip` to advance past whole bytes directly
  and only reload one byte for a genuine partial-byte remainder. New
  regression tests:
  `skip_past_a_fresh_reader_leaves_trailing_bits_of_the_last_byte_readable`,
  `skip_an_exact_multiple_of_8_bits_consumes_no_extra_byte`,
  `skip_beyond_available_bits_is_end_of_input` (2026-09-11).
- `tpt-media-bitstream::range::RangeDecoder::decode` divided by zero on a
  *second* call after an earlier call on the same decoder had already
  returned `EndOfInput`: running out of input partway through
  renormalization leaves `range` below `TOP` (and potentially below
  `total`) without ever restoring it, so `range/total` on a later call
  could be `0`. Found by the new `tpt-media-bitstream::fuzz_like`
  corruption sweep. Fixed by rejecting `total > self.range` up front
  (which also covers a caller genuinely passing a `total` larger than
  `range`, not just this poisoned-state case). New regression test:
  `decode_keeps_returning_err_after_running_out_of_input_instead_of_panicking`
  (2026-09-11).
- `tpt-audio-pulse`'s decoder read `n_bands` and each band's `n_coeffs`
  straight from the packet with no bound relative to `frame_size`, so a
  malformed/adversarial packet claiming more bands/coefficients than
  actually fit in the decoded spectrum buffer indexed it out of bounds
  and panicked instead of returning `CodecError::MalformedPacket`. Found
  by the new codec fuzz sweep (see `Added` above). Fixed by
  bounds-checking `start + n_coeffs <= spec.len()` (and rejecting
  `n_bands == 0`) before writing. New regression tests:
  `decode_rejects_n_coeffs_that_would_overrun_the_spectrum`,
  `decode_rejects_zero_bands` (2026-09-11).
- §47 security audit (integer overflow / bounds checks / allocation
  exhaustion / decompression bombs) over `tpt-container`'s untrusted-byte
  decoders found and fixed two more real issues: (1) `decode_idx`'s
  `entry_count` is a fully attacker-controlled `u32` (up to ~4.29
  billion) that was passed straight to `Vec::with_capacity` with no
  check against the actual input size — a malformed `IDX ` chunk of a
  few bytes could trigger a multi-gigabyte allocation attempt. Fixed by
  rejecting `entry_count` unless it could actually fit in the remaining
  body (each entry is a fixed 17 bytes). (2) `decode_tmta`'s `META_BYTES`
  branch bounds-checked a length field (`len`, also an attacker-controlled
  `u32`) with plain `p + len` `usize` addition — correct on 64-bit, but
  able to wrap around on a 32-bit target (this workspace explicitly
  targets 32-bit: `thumbv7em-none-eabi` for Vox, and wasm32 for the
  planned WASM target) and pass a check it should have failed. Fixed with
  `checked_add`. Tests:
  `decode_idx_rejects_entry_count_that_would_overrun_the_body`,
  `decode_tmta_rejects_bytes_value_length_that_would_overrun_the_body`
  (2026-09-11).
- CI's `no_std` Vox build (`cargo build -p tpt-audio-vox --target
  thumbv7em-none-eabi --no-default-features`) now compiles. The
  failure was hidden twice over: (1) member-level `default-features =
  false` on the workspace-shared `tpt-media-{core,bitstream,codec}`
  dependencies was silently *ignored* because Cargo only honours it
  when the `workspace.dependencies` entry declares it — so the `std`
  feature stayed on for the bare-metal target; and (2) `thiserror
  1.0.69` has no Cargo features and unconditionally requires `std`, so
  the `default-features = false` on it was a no-op. Fixed: workspace
  deps now declare `default-features = false` (std-consuming members
  opt back in via `features = ["std"]`, forwarding the feature
  transitively), `thiserror` bumped to 2.x (std optional), and
  `tpt-media-codec` gained the missing `#![cfg_attr(not(feature =
  "std"), no_std)]` gate plus an `alloc::string::String` import.
- `tpt-media-bitstream` / `tpt-media-core`: `f64` methods (`ln`,
  `log2`, `round`) are not available in `core` for `no_std` targets;
  the float math now routes through `libm` instead (2.13.1-era
  `bitflags` compiles clean for the bare-metal target once its `std`
  feature is not unified in).

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