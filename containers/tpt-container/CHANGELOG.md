# Changelog — tpt-container

Scoped record of changes to `tpt-container`. The workspace-wide release log —
including cross-cutting changes and the lockstep version bump that ships every
crate together — is the root [`CHANGELOG.md`](../../CHANGELOG.md). This file
is a filtered view of it: a change touching only this crate is recorded here
*and* summarised in the root entry; a change touching several crates is
recorded in the root, which remains the record (CONTRIBUTING §9.1).

Format follows [Keep a Changelog](https://keepachangelog.com/). Versions are
lockstep with the workspace; no API-stability guarantee pre-1.0
(CONTRIBUTING §9.2).

> **Wire-format changes are breaking.** The `.tpt` header carries its own
> version, independent of the crate version (CONTRIBUTING §9.1).

## [Unreleased]

### Fixed

- **A `.tpt` with a zero timebase component aborted the process.**
  `FileHeader::from_bytes` handed the file's own numerator and denominator
  straight to `Timebase::new`, whose documented contract is "a zero component
  panics in debug; in release it is treated as `1/1`". So the same crafted
  file killed a debug build with
  `panicked at tpt-media-core/src/timebase.rs: timebase denominator must be
  non-zero`, and was silently re-interpreted as a *different* timebase in a
  release one. Found by patching four header bytes of a real `.tpt` and
  running `tpt probe` on it; the header is now rejected with `BadHeader` at
  every entry point (`TptDemuxer::new`/`new_streaming`, `parse_tpt`,
  `for_each_chunk`). `decode_trak` already validated its own timebase and
  fell back to the header's — the header had to become trustworthy for that
  fallback to mean anything. `tpt probe` on the same file now reports
  `WARNING: TPT parse error: bad header` and exits 0.
- **`read_chunk_header` overflowed on a hostile offset.** The bounds check was
  `pos + 8 > bytes.len()`, which is a debug panic at `pos = usize::MAX` and
  wraps in release into a slice index panic — either way a panic, from a
  `pub` primitive whose contract is to return `None`. Rewritten as a
  `checked_sub` on the length.

### Added

- **§38 unit-test sweep over the crate's public API** (2026-09-30): 13 new
  tests for the items that had no coverage — the zero-timebase rejection and
  its reach through every entry point, `decode_trak`'s timebase fallback,
  `read_chunk_header` at and past its 8-byte boundary and against hostile
  offsets, `encode_chunk` round-tripping through it (empty body included),
  `chunk_id_is_reserved` across the whole `0x00..=0xFF` range with the
  `0x1F` boundary pinned in both directions, `parse_tpt`'s round trip and
  its rejection of non-containers, `seek_to_offset` (including past the end),
  `verify_cluster_checksums` on a file written without checksums, and
  `with_limits`. Three of them fail against the previous code — verified by
  restoring the old bodies and re-running.
- **Per-crate README, changelog, keywords and categories** (2026-09-30):
  crate-specific `README.md` and manifest `keywords`/`categories`.
- Seeded-random property tests (`tests/property.rs`, 2026-09-11): random
  packet streams round-trip exactly (PTS/DTS/flags/payload), muxing is
  byte-identical across runs, and extreme or duplicate PTS values (up to
  `i64::MAX`) round-trip without panicking (todo.md §47).
- §25 streaming: fragmented writing, incremental parsing, live append, a
  missing trailer, and reconstruction.
- `IDX ` per-track index and the `CRC1` container-level CRC-32 footer, both
  written by `finalize`; `crc32` is exported for reuse by codec layers.
- `benches/container_bench.rs`: mux + demux of a 200-packet stream (2026-09-14).

### Fixed

- §47 security audit of the untrusted-byte decoders found two real issues:
  1. `decode_idx`'s `entry_count` is a fully attacker-controlled `u32` that
     went straight to `Vec::with_capacity` with no check against the actual
     input size — a malformed `IDX ` chunk of a few bytes could trigger a
     multi-gigabyte allocation attempt. Fixed by rejecting `entry_count`
     unless it could fit in the remaining body (17 bytes per entry).
  2. `decode_tmta`'s `META_BYTES` branch bounds-checked a length field with a
     plain `p + len` `usize` addition — correct on 64-bit, but able to wrap
     on a 32-bit target (this workspace explicitly targets
     `thumbv7em-none-eabi` and wasm32) and pass a check it should have
     failed. Fixed with `checked_add`.
  Regression tests: `decode_idx_rejects_entry_count_that_would_overrun_the_body`,
  `decode_tmta_rejects_bytes_value_length_that_would_overrun_the_body`.
- The `tpt-media-probe` dependency was unused and inverted (probe → container
  is the correct direction, todo.md §52).

## [0.1.0] — 2026-09-04

### Added

- Initial crate: the native `.tpt` container — TLV chunk layout, 20-byte file
  header, `TRAK` track descriptors, `TMTA` typed metadata, `CLUS` data
  clusters with keyframe/EOF/discontinuity/duration flags and optional
  per-cluster checksums, keyframe seeking, corruption tolerance
  (`corruption_never_panics_and_truncation_never_panics`), and the
  `build_tpt`/`parse_tpt`/`for_each_chunk` convenience API.
