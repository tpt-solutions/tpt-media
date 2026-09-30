# Changelog — tpt-video-mote

Scoped record of changes to `tpt-video-mote`. The workspace-wide release log —
including cross-cutting changes and the lockstep version bump that ships every
crate together — is the root [`CHANGELOG.md`](../../CHANGELOG.md). This file
is a filtered view of it: a change touching only this crate is recorded here
*and* summarised in the root entry; a change touching several crates is
recorded in the root, which remains the record (CONTRIBUTING §9.1).

Format follows [Keep a Changelog](https://keepachangelog.com/). Versions are
lockstep with the workspace; no API-stability guarantee pre-1.0
(CONTRIBUTING §9.2).

> **Wire-format changes are breaking regardless of the crate version.** This
> codec is fail-fast: an unsupported 16-bit bitstream version is an error, not
> a guess (todo.md §35).

## [Unreleased]

### Added

- **The crate itself** (2026-09-30, todo.md §34): Mote, the extreme edge/IoT
  video codec. 8×8 patch dictionary VQ with up to 4096 entries (12-bit index),
  `Gray8`/`Yuv420` up to 480p (multiples of 16), key and delta frames with
  Rice-coded skip runs (conditional replenishment against one reference), an
  out-of-band or in-band dictionary identified by a 16-bit id, versioned
  bitstream (`VERSION = 1`) with a CRC-32 trailer, `CODEC_ID = *b"MOTE"`,
  `DynEncoder`/`DynDecoder` factories, hostile-input-safe decoder honouring
  `ResourceLimits`, and a provable packet-size bound
  (`MoteConfig::max_packet_bits`).
- VQ search reuses `tpt_dsp_core::nearest_vector_accelerated` (todo.md §8);
  training (seeded, integer-centroid Lloyd) is Mote's own and deterministic.
- Quality metric (`compare`, `QualityReport`): luma PSNR primary, all-plane
  PSNR and mean absolute error secondary.
- Tests, a criterion bench (bandwidth, quality, CPU) and a counting-allocator
  memory test; measured tables in `docs/performance.md`, design and the
  temporal-prediction investigation in `docs/codec-design.md`, algorithm
  sources in `docs/provenance.md`.
- Registered in `tpt-cli`'s `probe` registry. Not yet driven by `encode` /
  `decode` (no y4m bridge).
