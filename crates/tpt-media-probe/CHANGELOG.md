# Changelog — tpt-media-probe

Scoped record of changes to `tpt-media-probe`. The workspace-wide release log
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
- `tests/timestamps.rs` (2026-09-11, todo.md §47): `probe()` survives
  extreme, duplicated, descending and mixed-sign PTS containers and reports
  finite, non-negative durations. One unsigned-envelope quirk is documented
  rather than hidden.

### Fixed

- **The crate could not actually be built without `std`** (2026-10-01, found
  by auditing every crate's `no_std` claim). The `std` feature, the
  `#![cfg_attr(not(feature = "std"), no_std)]` attribute, the README's
  `no_std: Yes` row and CI's `wasm` job all said the crate builds without std
  — but every dependency was pinned in `[dependencies]` with
  `features = ["std"]`, which turns `std` on for the whole graph whatever a
  consumer asks for. `--no-default-features` therefore could not switch it off,
  and the CI check could not have failed on a real `std` leak. Now
  `default-features = false`, with `std` still forwarded through this crate's
  own `std` feature. Verified with the exact CI command.
- The crate did not compile for `wasm32-unknown-unknown` with `std` off
- The crate did not compile for `wasm32-unknown-unknown` with `std` off
  despite carrying the `no_std` switch — `format!`/`ToString` came from the
  std prelude and the `alloc` imports were missing. Every std build masked it;
  it was found on 2026-09-29 when the foundation crates were actually built
  for `wasm32` for the first time. Two missing imports, plus a `map_or` →
  `is_none_or` clippy fix that surfaced when check caches invalidated.
- The human-readable audio track line printed the first→last PTS envelope in
  raw ticks under a `"{:.3} s"` label with no timebase conversion, so a
  1-second 16 kHz file reported `15680.000 s` — found 2026-09-28 while
  smoke-testing the new Reed CLI path. Any `.tpt` file with a track timebase
  other than 1/1 was affected, including every file `tpt encode` produces.
  `probe --json` was already correct (it emits ticks and timebase explicitly).

## [0.1.0] — 2026-09-04

### Added

- Initial crate: container and codec introspection.
- Full `.tpt` probing via `TptDemuxer`, todo.md §44 (2026-09-08): container
  version and global timebase; per-track audio/video/subtitle/metadata
  inspection (codec ID and version, sample rate, channel layout, dimensions,
  pixel format, first/last PTS, frame and sample counts, name, language, typed
  `TMTA` metadata); duration; overall and per-track bitrate estimates;
  integrity reporting (the `CRC1` footer and per-cluster checksum failures);
  and codec capability reporting against the `tpt-media-codec` `Registry`
  with the native codec family table. `probe_file` reads from disk (`std`).
- The dependency edge `tpt-container` → `tpt-media-probe` was **inverted**
  (2026-09-08): it was unused, so the correct direction is probe → container
  (todo.md §52).
