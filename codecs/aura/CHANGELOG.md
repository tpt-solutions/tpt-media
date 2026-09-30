# Changelog — tpt-audio-aura

Scoped record of changes to `tpt-audio-aura`. The workspace-wide release log —
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
  now lists the shipped breaking wire changes explicitly, so "my old files no
  longer decode" has a documented cause.
- `lossless` / `lossy` feature flags gating which encode paths are compiled
  (CONTRIBUTING §10: capability features gate *modes*, and neither mode is a
  silently-degraded version of the other).

### Fixed

- An out-of-bounds write in the lossy path, found by the §38 codec fuzz sweep
  (2026-09-11).

### Changed

- **Lossy quantisation reworked** to be psychoacoustically driven
  (2026-09-07): each MDCT bin is mapped to a Bark critical band, bands are
  classified tonal/noise, simultaneous masking is computed across the Bark
  axis, and the resulting per-band threshold sets that band's step;
  first-order spectral noise shaping pushes quantisation error toward
  already-noisy neighbours. The per-band step table is transmitted (32-bit
  float per band) so the decoder needs no side information. This **replaced
  a flat 16-bit-per-coefficient scheme and is a breaking change to the v0.1
  lossy wire format** — packets encoded before it are not decodable.
- Residual coding is Exp-Golomb, not Rice. The `tpt-media-bitstream` Rice
  coder remains the correct tool for this distribution (todo.md §30's
  unchecked item), but switching would break every existing packet, so the
  doc comment was corrected to match the code rather than the reverse.

## [0.1.0] — 2026-09-04

### Added

- Initial crate (todo.md §30): **lossless** mode — autocorrelation LPC up to
  order 32 (Levinson-Durbin) with Exp-Golomb-coded zigzag residuals,
  bit-exact round trips; and **lossy** mode — large-window MDCT (2048/4096)
  in `f64`. Per-packet CRC-32 trailer (`CRC_TRAILER_BYTES`) verified before
  parsing, and the 16-bit bitstream version field — both breaking wire
  additions.
