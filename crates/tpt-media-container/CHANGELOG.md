# Changelog — tpt-media-container

Scoped record of changes to `tpt-media-container`. The workspace-wide release
log — including cross-cutting changes and the lockstep version bump that ships
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
- `Chunk` and the `Container` trait surface documented against the real
  implementation: `stream`, `next_packet`, `write_packet`, `finalize`,
  `seek_to_keyframe`, `tracks`. The README now records the read/write/seek
  asymmetry (a read-only format returns `Unsupported` from the write side)
  rather than leaving it implied.

## [0.1.0] — 2026-09-04

### Added

- Initial crate: the generic container abstraction (`Container` trait, `Chunk`,
  `ContainerError`) that keeps codecs independent of the file format they are
  packaged in. The `.tpt` implementation lives in
  [`tpt-container`](../../containers/tpt-container).
