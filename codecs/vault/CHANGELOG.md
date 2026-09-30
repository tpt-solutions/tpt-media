# Changelog — tpt-video-vault

Scoped record of changes to `tpt-video-vault`. The workspace-wide release log —
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

- **Initial implementation** (2026-09-30, todo.md §33): `Gray8` and planar
  `Yuv420` at any size that is a multiple of 8; 64×64 superblocks
  recursively partitioned to 8×8 with implicit splits at the picture edge;
  half-pel motion (full-pel pattern search plus 8-neighbour half-pel
  refinement, 2-tap bilinear interpolation, neighbour-predicted vector
  differences); up to 16 reference frames; four intra modes at every block
  size; 4×4–32×32 DCT from `tpt-dsp-core` in orthonormal quantiser units;
  RDO over partition, mode, reference and vector with the explicit objective
  `J = D + λR` (greedy over a pruned candidate set, not a global optimum);
  static Rice coefficient coding; boundary-strength-driven H.263 Annex J loop
  filter; per-packet CRC-32 trailer plus the 16-bit bitstream version;
  `max_packet_bits` derived bound; freeze-frame concealment; `DynEncoder` /
  `DynDecoder` factories with codec id `VAUL`.
- Hostile-input hardening and resource limits (§19/§38/§47): header,
  reference-index and motion-vector range checks, bounded coefficient walks,
  atomic failed decodes, packet/frame/allocation/reference/recursion limits.
- `VaultDecoder::last_stats()` (`FrameStats`): a per-frame census of the coding
  tools actually used.
- Tests: 15 unit, 18 property, 4 coding-tool census, 1 memory (38 total), and
  a criterion bench at QCIF and CIF.
- Provenance records in `docs/provenance.md` and design notes in
  `docs/codec-design.md`.
