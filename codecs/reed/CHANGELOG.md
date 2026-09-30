# Changelog — tpt-audio-reed

Scoped record of changes to `tpt-audio-reed`. The workspace-wide release log —
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
  crate-specific `README.md` and manifest `keywords`/`categories`.
- Provenance records in `docs/provenance.md` (2026-09-28) and the
  measured benchmark/memory figures in `docs/performance.md`.
- `ReedConfig::validate` is called from both constructors, so a rejected
  configuration is an error at construction rather than at first packet.
- CLI wiring: `--codec reed` runs through the same encode/decode path as the
  other audio codecs (8/16 kHz inputs).

### Changed

- Reed's documented memory budget rose from `< 8 KiB` to `< 16 KiB`
  (2026-09-28): the first measurement came in at 8 140 B — 99% of the old
  ceiling, passing by 52 bytes. A budget the implementation fills is a flake
  in waiting, so it was raised to the next sane bound rather than kept as
  false precision.

## [0.1.0] — 2026-09-04

### Added

- Initial crate (todo.md §31): a source-filter LPC vocoder in the spirit of
  the classic 2.4 kbps LPC-10 codecs. Per 20 ms frame: quantised LSP spectral
  envelope, voicing + pitch from normalised autocorrelation, 1 dB-step
  excitation gain with index 0 as exact mute, and a shape-gain VQ over a
  fixed 1024-entry stochastic codebook that is *derived* from its index via
  SplitMix64 on both sides rather than transmitted — 10 bits per unvoiced
  frame and no training corpus. PLC decays the excitation gain 6 dB per
  concealed frame. Builds `no_std + alloc`; CI compiles it for
  `thumbv7em-none-eabi` with `libm`-backed float math.
