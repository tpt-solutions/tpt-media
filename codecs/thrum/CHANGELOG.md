# Changelog — tpt-video-thrum

Scoped record of changes to `tpt-video-thrum`. The workspace-wide release log —
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

- **Per-crate README, changelog, keywords and categories** (2026-09-30):
  crate-specific `README.md` and manifest `keywords`/`categories`. The README
  records the measured latency, rate and memory figures and the profiling
  recipe, so the codec's central claim — latency over coding gain — is
  checkable rather than asserted.
- §36 video corpus, §37 video quality metrics and §42 measured memory budgets
  (2026-09-28, in `tpt-cli`): eight deterministic seeded scenes through real
  Thrum round trips in both coded formats, with per-scene rate/quality pins
  and a corpus-wide bitrate ordering that tracks inter-frame predictability.
- `ThrumConfig::max_packet_bits` derives the worst-case packet size from the
  level clamp and the per-code bit bound, so the encoder's single allocation
  provably cannot overflow.
- CLI codec-table wiring: `--codec thrum` / `--codec LIVE` resolve and
  `tpt list-codecs` advertises it. The `encode`/`decode`/`transcode`/
  `benchmark` family remains audio-only and rejects it with an explicit
  "video codec, audio-only" message rather than a misleading unknown-codec
  error.
- Provenance records in `docs/provenance.md` (2026-09-28).

### Fixed

- **Macroblock traversal order mismatch**: the decoder walked macroblocks
  column-major while the encoder wrote them row-major, desynchronising every
  macroblock after the first against both the bitstream and the intra
  prediction context — visible as ≈13 dB PSNR on a smooth gradient, now
  ≈45 dB.
- **Intra refresh did not sweep the whole picture**: the strided cursor was
  only exhaustive when the stride was coprime with the macroblock count; with
  48 macroblocks and 9 per frame the stride 6 shared a factor and silently
  covered 1/6 of the frame. `intra_refresh_sweeps_every_macroblock` now pins
  the property.
- **The encoder retained its worst-case packet capacity** (2.5 MB per
  176×144 packet) instead of the few kilobytes written — found by the §42
  allocation test. A `shrink_to_fit` fix was superseded the same day: the
  buffer is now allocated once at construction and reused, so the
  steady-state path does not allocate it at all, and per-frame churn is an
  exact-size payload copy.

## [0.1.0] — 2026-09-04

### Added

- Initial crate (todo.md §32): the I/P hybrid video codec — `Gray8` and planar
  `Yuv420`, macroblock-aligned dimensions (multiples of 16), four intra modes
  chosen by SAD, exhaustive full-pixel integer motion estimation with single
  reference frame and derived chroma vectors, rate-aware skip, 8×8 DCT-II/III
  from `tpt-dsp-core`, real Rice coding with a per-block parameter, an
  H.263-shaped 4×4-grid deblocking loop filter whose output becomes the next
  reference, per-packet CRC-32 trailer plus the 16-bit bitstream version
  field, and freeze-frame concealment. 45 unit tests, 6 property tests.
