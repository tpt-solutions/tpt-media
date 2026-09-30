# Changelog — tpt-media-sys

Scoped record of changes to `tpt-media-sys`. The workspace-wide release log —
including cross-cutting changes and the lockstep version bump that ships every
crate together — is the root [`CHANGELOG.md`](../../CHANGELOG.md). This file
is a filtered view of it: a change touching only this crate is recorded here
*and* summarised in the root entry; a change touching several crates is
recorded in the root, which remains the record (CONTRIBUTING §9.1).

Format follows [Keep a Changelog](https://keepachangelog.com/). Versions are
lockstep with the workspace.

> **ABI rule.** Adding functions or error codes is compatible and holds
> `ABI_VERSION`; changing a signature, an error code's meaning, a struct
> layout, or removing anything bumps it (todo.md §46).

## [Unreleased]

### Added

- **Per-crate README, changelog, keywords and categories** (2026-09-30):
  crate-specific `README.md` and manifest `keywords`/`categories`. The README
  documents the ownership, thread-safety and panic rules in the form a C
  caller needs them, plus the build-and-link recipe.
- Manifest correctness (2026-09-30): the five path dependencies now carry
  `version = "0.1.0"`. `cargo package`/`publish` rejects a bare `path` —
  "all dependencies must have a version requirement specified when
  packaging" — so the crate was unpackageable until this change.

### Fixed

- `readme` inherited from `[workspace.package]` resolved to a path relative to
  the *member* directory, so it pointed at a `README.md` that does not exist
  and `cargo package` failed outright ("readme `README.md` does not appear to
  exist"). The crate now declares its own `readme`, and every workspace member
  does the same.

## [0.1.0] — 2026-09-04

### Added

- Initial release, todo.md §46's first slice (2026-09-29): the stable C ABI
  for the codec abstraction. Opaque, exclusively-owned handles for packets,
  frames, encoders and decoders; `extern "C"` functions returning frozen
  `enum tpt_error` codes; `tpt_abi_version()` with a stated stability rule;
  explicit per-handle free functions (freeing NULL is a no-op, packet
  payloads are copied in); an abstract `tpt_media_config` (sample rate,
  channels, quality 0–100) mapping to per-codec parameters exactly as the
  CLI's `--quality` does, so a C caller never sees per-codec structs;
  per-function `# Safety` / `// SAFETY:` documentation and a thread-safety
  statement; every export panic-guarded so a caught panic becomes
  `TPT_ERR_PANIC` instead of unwinding into C; a hand-written
  `include/tpt_media.h` kept in sync by the two-directional
  `header_matches_the_abi` test; and a real C integration test
  (`tests/integration.c` sees only the header) driving a bit-exact Aura
  lossless round trip through `cargo test`. Built as both `rlib` and
  `staticlib`.
- `Context`, `Container`, `Demuxer` and `Muxer` handles are the recorded v0.2
  surface.
