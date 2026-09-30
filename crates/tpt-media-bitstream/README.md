# tpt-media-bitstream

Zero-allocation bit reader/writer and entropy coding for `tpt-media`.

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg)](../../LICENSE-MIT)
[![Rust](https://img.shields.io/badge/rust-1.86%2B-orange.svg)](https://www.rust-lang.org)
[![no_std](https://img.shields.io/badge/no__std-compatible-green.svg)](https://doc.rust-lang.org/1.86.0/core/)

Every TPT-native codec writes its wire format through this crate, so its
behaviour under malformed input *is* the codec-level security boundary. Part
of the [`tpt-media`](../../README.md) workspace —
[workspace changelog](../../CHANGELOG.md) · [changelog](CHANGELOG.md) ·
[architecture](../../ARCHITECTURE.md) · [contributing](../../CONTRIBUTING.md).

## At a glance

| | |
|---|---|
| Role | Bit-level I/O + entropy coding (todo.md §17) |
| Status | Implemented; depended on by every codec and by `tpt-container` |
| Version | `0.1.0`, lockstep with the workspace |
| MSRV | 1.86 — verified, enforced by CI's `msrv` job |
| License | MIT OR Apache-2.0 |
| `no_std` | Yes — builds for `wasm32-unknown-unknown` (CI job `wasm`) |
| Unsafe | `#![forbid(unsafe_code)]` |

## What it provides

| Item | What it is |
|---|---|
| [`BitReader`] | `&[u8]` + internal `u64` accumulator; arbitrary-width reads (1–56 bits), `peek`, `skip`, alignment, EOF |
| [`BitWriter`] | `&mut [u8]` + internal accumulator; arbitrary-width writes, alignment, bounded `finish()` |
| `huffman` | Canonical Huffman table build + encode/decode |
| `rice` | Rice/Golomb coder (`encode_riced`, `decode_riced`) |
| `range` | Static range coder (public-domain formulation) |
| `entropy` | Shannon entropy, histograms, distribution helpers |

**All types are allocation-free at runtime** — they operate on caller-owned
buffers, which is what makes the real-time paths possible.

## Usage

```rust
use tpt_media_bitstream::{BitReader, BitWriter};

// Write arbitrary-width fields into a caller-owned buffer.
let mut buf = [0u8; 8];
let mut w = BitWriter::new(&mut buf);
w.write(0b101, 3)?;
w.write(0x2A, 8)?;
w.finish()?;

let mut r = BitReader::new(&buf);
assert_eq!(r.read(3)?, 0b101);
assert_eq!(r.read(8)?, 0x2A);
# Ok::<(), tpt_media_bitstream::BitstreamError>(())
```

Real Rice coding, as Thrum uses it:

```rust
use tpt_media_bitstream::rice::{decode_riced, encode_riced};

let mut buf = [0u8; 16];
let mut w = BitWriter::new(&mut buf);
w.write(encode_riced(42, 1)?, 7)?;
w.finish()?;
```

## Error handling

Every fallible operation returns `Result`; the decode side is written so that
**arbitrary bytes can never panic**, only return `Err`. `BitstreamError` and
`WriteError` are the only failure types; there is no "successful" parse of
truncated input.

Two real defects were found by the sweeps below and are worth knowing about,
because they are the reason the API looks the way it does:

- `BitReader::skip` originally corrupted all subsequent reads for multi-byte
  skips (silent data corruption, fixed 2026-09-07).
- `range::RangeDecoder::decode` desynchronised from the encoder for any
  symbol with `freq > 1` — it narrowed the range without multiplying by the
  decoded symbol's frequency. The API is now the standard two-phase
  `decode_peek` / `update` split; `decode(total)` remains only for the
  uniform case it implemented correctly. Found 2026-09-14 by property tests,
  not by review.

## Feature flags

| Feature | Default | Effect |
|---|---|---|
| `std` | yes | `std::error::Error` impls |

`no_std + alloc` with `default-features = false`. Float math that is not in
`core` routes through `libm`.

## Design notes

The crate exists as a separate boundary on purpose: `tpt-dsp` owns transforms
and DSP primitives, `tpt-media-bitstream` owns *bit* packing and entropy
coding, and neither leaks into the other (`docs/architecture.md` §2).

## Benchmarks & profiling

```bash
cargo bench -p tpt-media-bitstream
```

`benches/bitstream_bench.rs` measures bit I/O, Rice, range and Huffman at
codec batch sizes. Recorded numbers live in
[`docs/performance.md`](../../docs/performance.md).

## Testing

```bash
cargo test -p tpt-media-bitstream
```

- `tests/property.rs` — seeded-random round trips through `BitWriter`/`BitReader`,
  Rice, Huffman and the range coder, plus byte-identical re-encode determinism.
- In-crate `fuzz_like` sweeps — fixed-seed LCG corruption/truncation over every
  decode entry point, asserting "returns `Result`, never panics". Deliberately
  dependency-free rather than `cargo-fuzz`/libFuzzer, matching the project's
  self-contained sweep strategy (todo.md §38).

## Known limitations

- A `BitWriter` whose buffer fills returns `WriteError::BufferFull`; there is
  no growable writer. That is deliberate (real-time paths need a bound), and
  callers who want growth size the buffer from `CodecConfig`.
- Entropy coders here are the classic Huffman/Rice/range formulations; no
  adaptive or arithmetic-coding context modelling beyond what `range` provides.

## License

MIT OR Apache-2.0, at your option. See [LICENSE-MIT](../../LICENSE-MIT) and
[LICENSE-APACHE-2.0](../../LICENSE-APACHE-2.0).
