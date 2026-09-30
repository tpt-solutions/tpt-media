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

- **`tests/dyn_contract.rs` — the crate's first integration test.** Every
  test until now was inline, and inline tests can reach private items, so
  nothing proved the framework is usable from *outside* the crate — which is
  exactly how `tpt-media-sys` (the C ABI) and `tpt-cli` consume it. Five
  tests drive the type-erased path end to end: `Registry` → `build()` →
  `DynDecoder`/`DynEncoder`, including that `drain()` and `flush()` are
  distinguishable (a decoder that made them identical would make
  `Capabilities::supports_drain` a lie), that an empty packet is
  `MalformedPacket` rather than a fabricated frame, and that a built codec
  reports the same `CodecInfo` its factory advertised.
- **§38 unit-test sweep over the crate's public API** (2026-09-30): 24 new
  tests (6 → 30) for the items that had no coverage — every
  `ResourceLimits::check_*` at its exact boundary (`max` passes, `max + 1`
  does not) and reporting its own `ResourceLimitKind`, `Default` resolving
  to `strict()`, `strict()` being stricter on all seven fields rather than
  the one the old test compared, `usize::MAX` rejected by every byte limit,
  `SupportedFormat::matches` within and across variants, `supports_format`
  (including that an empty list supports *nothing*, not everything),
  `CodecError`'s `Display` for all nine variants and its `std::error::Error`
  impl, `Capabilities::default`, `CodecConfig::default`, and the
  `Registry`'s encoder side, ID iteration, empty case, duplicate-ID
  shadowing and hand-written `Debug`.
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
