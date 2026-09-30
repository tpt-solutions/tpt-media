# Changelog — tpt-media-bitstream

Scoped record of changes to `tpt-media-bitstream`. The workspace-wide release
log — including cross-cutting changes and the lockstep version bump that ships
every crate together — is the root
[`CHANGELOG.md`](../../CHANGELOG.md). This file is a filtered view of it: a
change touching only this crate is recorded here *and* summarised in the root
entry; a change touching several crates is recorded in the root, which remains
the record (CONTRIBUTING §9.1).

Format follows [Keep a Changelog](https://keepachangelog.com/). Versions are
lockstep with the workspace; no API-stability guarantee pre-1.0
(CONTRIBUTING §9.2).

## [Unreleased]

### Added

- **Per-crate README, changelog, keywords and categories** (2026-09-30):
  crate-specific `README.md` and manifest `keywords`/`categories`, replacing
  one inherited workspace set shared by all sixteen crates.
- Seeded-random property tests (`tests/property.rs`, 2026-09-14): randomized
  write/read round trips through `BitWriter`/`BitReader`, Rice, Huffman and
  the range coder, plus byte-identical re-encode determinism.
- Deterministic, dependency-free corruption sweeps (`fuzz_like`, 2026-09-11)
  over every decode entry point — a fixed-seed LCG feeding arbitrary and
  corrupted buffers, asserting every call returns a `Result` and never
  panics.

### Fixed

- `BitReader::skip` corrupted all subsequent reads for multi-byte skips —
  silent data corruption rather than an error (2026-09-07).
- `range::RangeDecoder::decode` desynchronised from the encoder for any
  symbol with `freq > 1`: the decoder narrowed its range to `range/total`
  without multiplying by the decoded symbol's frequency, while the encoder
  narrows to `(range/total)·freq`, so every non-uniform model diverged after
  the first symbol. Found 2026-09-14 by the new property tests, not by
  review — every pre-existing test used `freq = 1`. The API is now the
  standard two-phase scheme: `decode_peek(total)` returns the
  cumulative-frequency bin without touching state and `update(cum, freq,
  total)` narrows by the chosen symbol's interval; `decode(total)` remains
  for the uniform case it implemented correctly.
- `decode` could divide by zero after running out of input (renormalization
  left `range` below `total` without restoring it), panicking on a *second*
  call after a prior `EndOfInput`. Now `total > range` is rejected up front;
  regression test `decode_keeps_returning_err_after_running_out_of_input_instead_of_panicking`.
- `f64` methods are not in `core` for `no_std` targets; the float math routes
  through `libm` (2026-09-09).

## [0.1.0] — 2026-09-04

### Added

- Initial crate: zero-allocation `BitReader`/`BitWriter` (arbitrary-width
  reads/writes, alignment, peek, skip), canonical Huffman encoder/decoder,
  Rice/Golomb coder, public-domain range coder, entropy helpers. All types
  operate on caller-owned buffers.
