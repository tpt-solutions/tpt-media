# Security Model

The full threat model is in [`SECURITY.md`](../SECURITY.md). This
document covers the implementation-level mitigations.

## Trust Boundary

`tpt-media`'s trust boundary is the input media file. Every API that
accepts bytes from the network, the file system, or the user must
defend against malicious input.

The boundary is crossed at:

- `tpt-media-container::Demuxer::next_packet` — reads `.tpt` chunks.
- `tpt-media-bitstream::BitReader::read_*` — reads bit patterns.
- `<codec>::Decoder::decode` — consumes compressed bitstreams.

## Resource Limits

Every decoder accepts a `ResourceLimits` struct on construction
(`tpt-media-codec`):

```rust
pub struct ResourceLimits {
    pub max_frame_bytes: u32,
    pub max_packet_bytes: u32,
    pub max_reference_frames: u8,
    pub max_allocation_bytes: u32,
    pub max_recursion_depth: u8,
    pub max_decode_micros: u32,   // 0 = disabled
    pub max_metadata_bytes: u32,  // added 2026-09-11
}
```

`permissive()` and `strict()` presets exist (`Default` = `strict()`).
When a limit is exceeded, the decoder returns
`CodecError::ResourceLimit`; the container layer returns
`TptError::ResourceExceeded` for chunk-level violations
(`max_metadata_bytes` against a raw `TMTA` body, for example).
`TptDemuxer` accepts limits via `with_limits`/`new_streaming_with_limits`.

## Bounds Checks

All reads from `&[u8]` use `checked indexing` (returning a `Result`)
rather than `[]` indexing. The bitstream reader returns
`BitstreamError::EndOfInput` when the buffer is exhausted.

The container parser never reads past a chunk's declared length. When
the declared length exceeds the remaining file, the chunk is treated as
truncated and parsing stops with `ContainerError::Truncated`.

## Integer Overflow

All size calculations use `checked_add`, `checked_mul`, and `checked_shl`
when the result can overflow. Where performance permits, this is
enforced by clippy's `arithmetic_overflow` lint.

## Allocations

Library code never allocates on the documented real-time path. In
offline code, allocations are bounded by `max_allocation_bytes`.

## Panic Safety

Library code must never panic on malformed input. Panics from internal
bugs are caught at the FFI boundary (via `catch_unwind`) and surfaced
as `DecodeError::Internal`.

## Fuzzing

The project's established fuzzing approach (§38) is **deterministic,
dependency-free corruption/truncation sweeps** — a fixed-seed LCG
driving bit-flips and truncation over real fixtures, asserting
`Result`-or-no-panic — rather than an external `cargo-fuzz`/libFuzzer
harness, so every "fuzz target" is an ordinary reproducible `#[test]`
that CI already runs:

- `tpt-media-bitstream::fuzz_like` — sweeps `BitReader`
  read/peek/skip, Huffman, Rice, and the range decoder over arbitrary
  and corrupted bytes. Found and fixed a divide-by-zero on a
  post-`EndOfInput` `RangeDecoder` (2026-09-11).
- `tpt-container` — `corruption_never_panics_and_truncation_never_panics`
  (every truncation point + 512 seeded bit-flips of a full fixture).
- `tpt-audio-{aura,pulse}` — arbitrary packets + truncation + bit-flip
  sweeps of each decoder. Found and fixed Pulse's out-of-bounds
  spectrum write on malformed band counts (2026-09-11).
- `tpt-media-probe` — truncation/bit-flip/arbitrary sweeps over `probe()`.
- Randomized *round-trip* property tests (`tests/property.rs` in
  bitstream, core, container, and both codecs, added 2026-09-14) — the
  container extreme-timestamp sweep and probe's
  `tests/timestamps.rs` extend this to hostile PTS/DTS values (§47).

Long-running coverage-guided fuzzing remains a pre-1.0 goal; the
deterministic sweeps are the current, CI-enforced baseline.

## Malformed Timestamps (§47, audited 2026-09-14)

Timestamps (`PTS`/`DTS`) cross the trust boundary as opaque tick
counts. The demuxer performs **no arithmetic** on them — any
representable `i64` tick value round-trips exactly (pinned by
`extreme_and_duplicate_pts_round_trip_without_panicking` in
`tpt-container/tests/property.rs`). The first consumer that does
arithmetic is the probe, audited by `tpt-media-probe/tests/timestamps.rs`:
extreme (`i64::MAX`), duplicated, descending, and mixed-sign PTS sets
produce finite, non-negative reports or no report at all — never a
panic, and never unbounded work. One honest quirk is recorded there:
the probe's first/last PTS envelope is computed on *unsigned* ticks, so
a stream mixing negative and positive timestamps reports its envelope
in unsigned order. Correct-looking timestamps in the report for a
nonsense input (e.g. a bitrate of 0 bit/s for a file claiming
`i64::MAX` ticks) are considered acceptable output, not a defect.

## Decompression Bombs

A malicious file cannot expand into unbounded memory because:

- Frame dimensions are validated against `ResourceLimits` before any
  buffer is allocated.
- The bitstream reader refuses to read past the declared packet size.
- Recursive algorithms (e.g., superblock partitioning in Vault) have a
  bounded recursion depth.

## FFI Safety

`ffi/tpt-media-sys` is the only crate that uses `unsafe`. Its
invariants are:

- Opaque handles are non-null.
- The caller owns all buffers passed to FFI functions.
- Allocation and deallocation are paired (every `tpt_*_new` has a
  matching `tpt_*_free`).
- No panics cross the boundary (caught and converted to error codes).

## Security Audit Schedule

| Audit                                   | Status   | Owner      |
| --------------------------------------- | -------- | ---------- |
| Integer-overflow audit                  | done (2026-09-11, `decode_tmta` 32-bit wrap fixed) | TPT |
| Bounds-check audit                      | done (2026-09-11, Pulse OOB write fixed) | TPT |
| Allocation-exhaustion audit             | done (2026-09-11, `decode_idx` entry-count bomb fixed) | TPT |
| Decompression-bomb protection test      | done (2026-09-11, same fix + §19 limits) | TPT |
| Fuzzing CI operational                  | done (deterministic sweeps run in CI; see Fuzzing) | TPT |
| Malformed-timestamp audit               | done (2026-09-14, see above) | TPT |
| External security review (pre-1.0)      | planned  | TBD        |