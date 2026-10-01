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

- **§38 unit-test sweep over the crate's public API** (2026-10-01): the crate
  had **one** test, and it was a tautology — it constructed a `Chunk` and read
  back the two fields it had just assigned, which cannot fail. 16 tests now
  (1 → 16), covering the categories that had no coverage at all:
  - a `Fake` that actually **implements `Container`**, so the trait's contract
    is exercised rather than merely declared — write/read round trip through
    the trait, `next_packet` advancing without repeating, `finalize`,
    `seek_to_keyframe` leaving the reader usable, and `stream`/`tracks`
    reachable on a shared borrow;
  - **empty input** — a zero-length `Chunk` body, an empty container
    (`next_packet` → `Truncated`, never a fabricated packet), and a
    zero-length packet payload, which is legal at the codec level;
  - **end of stream** — reading past the last packet is `Truncated`, an error
    rather than a panic or a silent `Ok`;
  - **maximum values / overflow** — `seek_to_keyframe` at `i64::MIN` and
    `i64::MAX`, and `Debug` over a 4 KiB body;
  - **the derives are the public surface** — `Clone` is a *deep* copy (a
    caller that mutates a clone cannot reach the original's body), `Eq`
    compares both fields, and every `ContainerError` variant is nameable with
    its `Unsupported`/`Io` payload intact.
- **Per-crate README, changelog, keywords and categories** (2026-09-30):
  crate-specific `README.md` and manifest `keywords`/`categories`.
- `Chunk` and the `Container` trait surface documented against the real
  implementation: `stream`, `next_packet`, `write_packet`, `finalize`,
  `seek_to_keyframe`, `tracks`. The README now records the read/write/seek
  asymmetry (a read-only format returns `Unsupported` from the write side)
  rather than leaving it implied.

### Fixed

- **The crate claimed `no_std` in three places and was not `no_std`** (found
  2026-10-01, while sweeping for the §38 gap above). The README's "At a
  glance" said `no_std: Yes — builds for wasm32-unknown-unknown (CI job
  wasm)`, the manifest listed `no-std` in both `keywords` and `categories`,
  and a `std` feature existed — but `src/lib.rs` had **no `#![no_std]`
  attribute at all**, and the manifest pinned its dependencies with
  `features = ["std"]` rather than `default-features = false`, so `--no-default-features`
  could not turn `std` off and the build "passed" while still using it. A
  sibling (`tpt-media-codec`) shows the correct pattern; this crate now
  matches it: `#![cfg_attr(not(feature = "std"), no_std)]` + `#![forbid(unsafe_code)]`
  + `extern crate alloc` + `use alloc::vec::Vec`, and both dependencies moved
  to `default-features = false`. Verified with
  `cargo check -p tpt-media-container --no-default-features --target wasm32-unknown-unknown`.
  The crate was also **absent from CI's `wasm` job**, so nothing had ever
  built it for `wasm32` at all — it is now in the `no_std build` list. The
  claim was never tested, only asserted; it is now enforced.

## [0.1.0] — 2026-09-04

### Added

- Initial crate: the generic container abstraction (`Container` trait, `Chunk`,
  `ContainerError`) that keeps codecs independent of the file format they are
  packaged in. The `.tpt` implementation lives in
  [`tpt-container`](../../containers/tpt-container).
