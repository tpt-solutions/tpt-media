# Changelog — tpt-media-io

Scoped record of changes to `tpt-media-io`. The workspace-wide release log —
including cross-cutting changes and the lockstep version bump that ships every
crate together — is the root [`CHANGELOG.md`](../../CHANGELOG.md). This file
is a filtered view of it: a change touching only this crate is recorded here
*and* summarised in the root entry; a change touching several crates is
recorded in the root, which remains the record (CONTRIBUTING §9.1).

Format follows [Keep a Changelog](https://keepachangelog.com/). Versions are
lockstep with the workspace; no API-stability guarantee pre-1.0
(CONTRIBUTING §9.2).

## [Unreleased]

### Added

- **Per-crate README, changelog, keywords and categories** (2026-09-30):
  crate-specific `README.md` and manifest `keywords`/`categories`.
- `examples/io_tour.rs`: a runnable tour of the byte-exact WAV and PPM round
  trips (2026-09-28, with the documentation baseline audit). Examples live in
  the crate they demonstrate; the repo-root `examples/` directory stays empty
  on purpose.

### Fixed

- The tests' hand-rolled no-op waker was replaced with `Waker::noop()`, which
  has been stable since the declared MSRV (surfaced 2026-09-29 when a
  `wasm32` check cache invalidated and clippy re-ran).

## [0.1.0] — 2026-09-04

### Added

- Initial crate, todo.md §27 (2026-09-08): the media I/O layer, `no_std +
  alloc` with `std` as the default feature. The `IoError` model
  (`UnexpectedEof` / `InvalidHeader` / `Unsupported` / `Overflow` / `Other`);
  PCM integration (`PcmSpec`, `encode_pcm`/`decode_pcm` with chunked monotonic
  PTS, `normalize_to_f32`/`denormalize_from_f32`, `f32`-exact for ≤24-bit
  integer formats); WAV — a byte-exact `parse_wav`/`decode_wav`/`write_wav`
  path for the codec pipeline plus a std-gated `f32` bridge that **reuses
  `tpt-dsp-io`** rather than duplicating it (todo.md §27's constraint);
  dependency-free PPM `P6` / PGM `P5` raster I/O over `VideoFrame` with
  `checkerboard` fixtures; streaming `FrameSource`/`MediaItem` (pull) and
  `FrameSink` (push) with slice/vec implementations for `no_std` and
  file/stdin/stdout under `std`, where a stream ending mid-sample-frame is an
  error rather than a silent truncation; and async integration via
  `AsyncFrameSource`/`AsyncFrameSink` (RPITIT + `Send`, no runtime dependency)
  with a synchronous `BlockingAdapter`. 32 unit tests.
