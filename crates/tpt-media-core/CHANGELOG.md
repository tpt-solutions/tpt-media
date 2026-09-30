# Changelog — tpt-media-core

Scoped record of changes to `tpt-media-core`. The workspace-wide release log —
including cross-cutting changes and the lockstep version bump that ships every
crate together — is the root [`CHANGELOG.md`](../../CHANGELOG.md). This file
is a filtered view of it: a change touching only this crate is recorded here
*and* summarised in the root entry; a change touching several crates is
recorded in the root, which remains the record (CONTRIBUTING §9.1).

Format follows [Keep a Changelog](https://keepachangelog.com/). Versions are
lockstep with the workspace; the leading `0` component means no API-stability
guarantee (CONTRIBUTING §9.2).

## [Unreleased]

### Added

- **Per-crate README, changelog, keywords and categories** (2026-09-30):
  this crate now has its own `README.md` (user-facing: what it provides, real
  usage, feature flags, testing, limitations) and crate-specific
  `keywords`/`categories` in its manifest instead of inheriting one workspace
  set that made every crate look identical in crates.io search.

### Fixed

- Timestamp `rescale` overflow bug (2026-09-07): rescaling between
  timebases could overflow silently. `rescale` now reports overflow instead
  of wrapping, and signed-duration arithmetic saturates rather than wrapping;
  `checked_{add,sub}_duration` are the explicit forms. The bitstream-adjacent
  `Timestamp` fixes are recorded in the root changelog.
- `f64` methods (`ln`, `log2`, `round`) were used directly, which are not in
  `core` for `no_std` targets (2026-09-09); the float math now routes through
  `libm`.

## [0.1.0] — 2026-09-04

### Added

- Initial crate: shared media data types — `Timestamp`, `Duration`,
  `Timebase`, `Packet`, `PacketFlags`, `Frame`, `AudioFrame`, `VideoFrame`,
  `Track`, `Stream`, `MediaType`, `CodecId`, `CodecConfig`, `PixelFormat`,
  `SampleFormat`, `ChannelLayout`, `Metadata`.
- Rational-time design with **no `f64` timestamps**, and caller-owned frame
  buffers (ARCHITECTURE.md §4).
