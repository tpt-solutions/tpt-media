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

- **Caller-buffer real-time decode (2026-11-04).** `ReedDecoder` implements
  `RealtimeDecoder`. `decode_into` makes 0 allocation calls per packet at steady
  state (`tests/alloc_count.rs`). The owned `decode` fell from 11 to 2 calls per
  packet; the last four were in `tpt-dsp-core`'s `lsp_to_lpc`, which now has a
  scratch variant (`lsp_to_lpc_with_scratch`, in `tpt-dsp` master, unreleased).
  Output is byte-identical to `decode`. No wire change. The encoder is not in
  scope, because its capabilities do not claim real-time operation.

### Changed

- **Reed wire version 2 (breaking, 2026-10-19).** The gain index is 7 bits, not
  6, and spans −126 to 0 dB in 1 dB steps, not −62 to 0. Version 1 clamped the
  gain a quiet voiced frame needs: a high-Q resonance makes the synthesis
  filter's impulse-response energy large, so the gain is far below −62 dB. A
  −52 dBFS 200 Hz tone came back about 16 dB loud; it now comes back within 2 dB
  across a 40 dB range of input levels. Packets are 91 bits of parameters, which
  still round to 12 bytes, so the container-level rate is unchanged. Version 1
  packets are rejected.

### Fixed

- **`encode` refuses frames that are not exactly one frame (2026-11-05).** A
  longer frame was truncated to its first frame with no error, silently dropping
  the rest of the input. A zero-length frame now returns no packets. Any other
  length returns `InvalidConfig`. The CLI always sends whole frames.

- **Voiced output level (2026-10-19).** A 200 Hz tone came back up to 8 dB loud,
  and speech came back about 8 dB loud at default quality. The encoder took the
  excitation gain from the residual of the unquantised predictor, but the decoder
  synthesises through the quantised one, and for a high-Q resonance those differ
  a lot. The gain is now the value that gives the input's power through the
  quantised synthesis filter. Speech at quality 80 is now within 1.5 dB, noise
  within 0.1 dB. This change alone left the wire format and the decoder
  unchanged; the quiet-tone case below needed version 2.

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
