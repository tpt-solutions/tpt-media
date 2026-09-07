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

Every decoder accepts a `ResourceLimits` struct on construction:

```rust
pub struct ResourceLimits {
    pub max_frame_bytes: usize,
    pub max_packet_bytes: usize,
    pub max_allocation_bytes: usize,
    pub max_reference_frames: usize,
    pub max_recursion_depth: u32,
    pub max_decode_micros: u32,
}
```

Defaults are conservative (e.g., `max_frame_bytes = 64 MiB`). When a
limit is exceeded, the decoder returns `DecodeError::ResourceLimit`.

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

The following fuzz targets are in CI:

- `fuzz/fuzz_bitstream` — random bytes into `BitReader`.
- `fuzz/fuzz_container` — random bytes into the `.tpt` parser.
- `fuzz/fuzz_codec_<name>` — random bytes into each decoder.
- `fuzz/fuzz_probe` — random bytes into `tpt-media-probe`.

Each target is run for 10 minutes per CI run; nightly runs are longer.

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
| Integer-overflow audit                  | pending  | TPT        |
| Bounds-check audit                      | pending  | TPT        |
| Allocation-exhaustion audit             | pending  | TPT        |
| Decompression-bomb protection test      | pending  | TPT        |
| Fuzzing CI operational                  | in-progress | TPT      |
| External security review (pre-1.0)      | planned  | TBD        |