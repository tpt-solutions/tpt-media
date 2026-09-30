# Changelog — tpt-media-codec

Scoped record of changes to `tpt-media-codec`. The workspace-wide release log
— including cross-cutting changes and the lockstep version bump that ships
every crate together — is the root [`CHANGELOG.md`](../../CHANGELOG.md). This
file is a filtered view of it: a change touching only this crate is recorded
here *and* summarised in the root entry; a change touching several crates is
recorded in the root, which remains the record (CONTRIBUTING §9.1).

Format follows [Keep a Changelog](https://keepachangelog.com/). Versions are
lockstep with the workspace; no API-stability guarantee pre-1.0
(CONTRIBUTING §9.2).

## [Unreleased]

### Added

- **Per-crate README, changelog, keywords and categories** (2026-09-30):
  crate-specific `README.md` and manifest `keywords`/`categories`.
- `drain()` on both `Encoder` and `Decoder` (2026-09-29, with the C ABI
  slice): graceful end-of-stream that emits still-buffered frames *without*
  discarding reference frames or rate-control history, where `flush()` is a
  hard reset for stream reuse. The default forwards to `flush`, which is
  correct for codecs with no look-ahead; codecs that hold back frames
  override it and advertise `supports_drain`.
- `ResourceLimits` gained `max_metadata_bytes` and `check_metadata`, so a
  container's per-track `TMTA` block is bounded *before* it is parsed.

### Changed

- `DynEncoder`/`DynDecoder` gained public `new()` constructors (2026-09-29):
  the FFI needs direct boxed construction, while the factories stay for
  config-from-codec-private construction.

### Fixed

- The crate lacked `#![cfg_attr(not(feature = "std"), no_std)]` despite
  carrying a `std` feature, so the `no_std` bare-metal build failed once the
  workspace `default-features = false` fix landed (2026-09-09); a missing
  `alloc::string::String` import came with it.

## [0.1.0] — 2026-09-04

### Added

- Initial crate: generic `Encoder`/`Decoder` traits, `CodecInfo`,
  `CodecConfig`, `SupportedFormat`, `Capabilities`, the `CodecError` model,
  `ResourceLimits`, and type-erased `DynEncoder`/`DynDecoder` with a
  `Registry` for codec discovery.
