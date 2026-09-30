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

## Browser Player Memory Limits (§45, `tpt-web`)

`apps/tpt-web` runs the decoders inside WebAssembly, where the memory
model changes the threat: linear memory only grows, and a failed growth
aborts the whole module, so a hostile file must be refused *before* it
can ask for memory rather than after. `tpt_web::WebLimits` wraps
`ResourceLimits` with the player-level bounds; the design and full table
are in [`web-design.md`](web-design.md) section 7. In summary:

- **Every bound is checked before allocating**, from the 8-byte chunk
  header alone: a `CLUS` claiming more than `max_packet_bytes`, a `TMTA`
  above `max_metadata_bytes`, or a `TRAK` above 256 KiB fails the stream
  with `WebError::Limit` without buffering a body byte. Unknown chunks are
  skipped without buffering at all, whatever size they claim.
- **The unparsed backlog is capped** (`max_input_buffer_bytes`, default
  16 MiB): `push` returns the retryable `InputFull` and consumes nothing,
  so a fast or malicious network source cannot grow memory without bound.
  Buffer growth uses `try_reserve`, so an allocation failure is an error,
  not an abort.
- **Decoded output is capped**: audio queue (default 2 s), picture queue
  (default 4) and per-frame size (default 8 MiB, checked against the
  *RGBA* size at track declaration and again per frame) each have a soft
  limit that stalls decoding and a hard stop at 2x that drops output.
  A track that claims a 60 000 x 60 000 picture is refused before any
  decoder is built.
- **Caller-supplied limits are clamped** to `WebLimits::ceiling()`, including
  through the C ABI, so page script cannot ask for an unbounded budget;
  at most 4 players and one 4 MiB staging slice exist per module.
- **Undecodable packets are tolerated but bounded** (`max_decode_errors`,
  default 256): a corrupt packet is skipped and counted, and enough of them
  fail the stream, so hostile input always terminates.
- **The worst case is a computed number**, not a hope:
  `WebLimits::memory_ceiling_bytes()` is 223 MiB for the defaults, pinned by
  a test, and the linker `--max-memory` cap (`web/build.mjs`, default 512
  MiB) bounds the instance regardless.
- **`unsafe` is confined to the export module** (`abi.rs`); no pointer ever
  flows from JavaScript into Rust (input is staged in module-owned memory),
  so the parser and player are safe code. The same deterministic
  mutation/truncation/noise sweeps as §38 run against the parser and player
  natively and against the built wasm under Node (`web/check.mjs`: 300 files,
  zero traps).

Not yet bounded: `max_decode_micros` and cancellation (no codec checks them;
§19), and the browser main thread itself (decoding is not in a Worker), so a
pathological-but-legal stream can still make a page unresponsive for the
duration of one packet's decode.

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

Library code must never panic on malformed input — every parse of
untrusted bytes returns a `Result` (`CodecError::MalformedPacket`,
`TptError::Truncated`, …), and the §38 sweeps assert Result-or-no-panic
over hostile inputs. There is no FFI boundary yet (§46); when
`tpt-media-sys` exists, `catch_unwind` at that boundary will surface
internal panics as a C-level error code rather than crossing ABI.

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
- `tpt-audio-{aura,cinder}` — arbitrary packets + truncation + bit-flip
  sweeps of each decoder. Found and fixed Cinder's out-of-bounds
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
  bounded recursion depth. Vault's is structurally three levels (64 → 8)
  and is additionally checked against `limits.max_recursion_depth` on every
  packet; its corruption sweep repairs the CRC after corrupting the body so
  that the parser, not just the checksum, is exercised
  (`codecs/vault/tests/property.rs`).

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
| Bounds-check audit                      | done (2026-09-11, Cinder OOB write fixed) | TPT |
| Allocation-exhaustion audit             | done (2026-09-11, `decode_idx` entry-count bomb fixed) | TPT |
| Decompression-bomb protection test      | done (2026-09-11, same fix + §19 limits) | TPT |
| Fuzzing CI operational                  | done (deterministic sweeps run in CI; see Fuzzing) | TPT |
| Malformed-timestamp audit               | done (2026-09-14, see above) | TPT |
| External security review (pre-1.0)      | planned  | TBD        |