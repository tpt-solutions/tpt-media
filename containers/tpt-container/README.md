# tpt-container

The native `.tpt` container format: TLV chunks, header, TRAK, CLUS, seeking, streaming.

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg)](../../LICENSE-MIT)
[![Rust](https://img.shields.io/badge/rust-1.86%2B-orange.svg)](https://www.rust-lang.org)

The reference implementation of the TPT-native container, implementing
[`tpt-media-container`](../../crates/tpt-media-container)'s `Container` trait.
Part of the [`tpt-media`](../../README.md) workspace —
[workspace changelog](../../CHANGELOG.md) · [changelog](CHANGELOG.md) ·
[container spec](../../docs/container-design.md) ·
[architecture](../../ARCHITECTURE.md) · [contributing](../../CONTRIBUTING.md).

## At a glance

| | |
|---|---|
| Role | Native container format (todo.md §20–§25) |
| Magic | `TPTM` · header version `0x0001` |
| Status | Implemented (streaming, index, integrity all live) |
| Version | `0.1.0`, lockstep with the workspace |
| MSRV | 1.86 — verified, enforced by CI's `msrv` job |
| License | MIT OR Apache-2.0 |
| `no_std` | No — `std` scaffolding exists but the crate is std-only today |
| Unsafe | `#![forbid(unsafe_code)]` |

## Binary layout (little-endian)

```text
FileHeader (20 bytes)
  magic        u8[4]    = "TPTM"
  version      u16      = 0x0001 (major = >> 8, minor = & 0xFF)
  header_size  u16      = 20
  timebase_num u32
  timebase_den u32
  reserved     u32      = 0 (ignored on read)

Chunk (TLV)
  chunk_id     u8[4]
  chunk_size   u32
  body         u8[chunk_size]
```

Chunk types: `TRAK` (full track descriptor), `TMTA` (typed per-track
metadata), `CLUS` (data cluster: `pts`, `dts`, `flags`, `track_id`, optional
`duration`, payload, optional CRC-32), `IDX ` (optional per-track index,
written by `finalize`, versioned), `CRC1` (optional container-level CRC-32
footer, written by `finalize`). Field-by-field:
[`docs/container-design.md`](../../docs/container-design.md).

## Usage

```rust
use tpt_container::{TptMuxer, TptDemuxer};
use tpt_media_core::{Timebase, Packet, Timestamp};

let tb = Timebase { num: 1, den: 48_000 };

let mut m = TptMuxer::new(tb);
m.register_track(&track);            // writes TRAK
m.write_packet(&packet)?;            // writes CLUS
let bytes = m.finish();              // writes IDX + CRC1 trailer

let mut d = TptDemuxer::new(&bytes)?;
while let Some(packet) = d.next_packet()? {
    // ...
}
```

One-shot helpers for the common case:

```rust
use tpt_container::{build_tpt, parse_tpt};

let bytes  = build_tpt(tb, &[(0, b"AURA", &[])], &packets);
let (header, tracks, packets) = parse_tpt(&bytes)?;
```

`for_each_chunk` walks raw chunks — known, unknown and reserved alike — which
is how a forward-compatible reader inspects a file whose chunk IDs it does not
yet understand.

## Design properties

- **Forward compatible.** Unknown chunk IDs are skipped; reserved header bytes
  are ignored.
- **Fuzz resistant.** Every chunk length is bounds-checked; corrupt or
  truncated input yields `TptError`, never a panic.
- **Streaming.** Chunks parse as they arrive; a partial trailing chunk merely
  pauses the parser, and a file still being appended to demuxes correctly.
- **Seekable.** Keyframes are `CLUS` flags; `IDX ` (optional, versioned) makes
  seeking O(1) per track.
- **Deterministic.** The muxer writes no timestamps, counters or padding
  entropy — the same packet stream produces byte-identical files.

## Feature flags

| Feature | Default | Effect |
|---|---|---|
| `std` | yes | std conveniences |

`std`-only in practice: the `std` feature scaffolding exists, but the crate
does not yet carry `#![cfg_attr(not(feature = "std"), no_std)]`. Converting it
is follow-up work, not a done fact (CONTRIBUTING §10).

## Benchmarks & profiling

```bash
cargo bench -p tpt-container
```

`benches/container_bench.rs` — mux + demux of a 200-packet stream
(2026-09-14, release profile, criterion): mux 84 µs (0.42 µs/packet), demux
22 µs (0.11 µs/packet). Tables:
[`docs/performance.md`](../../docs/performance.md).

Profiling recipe: `cargo bench -p tpt-container`, then `cargo flamegraph
--bench container_bench` (Linux) / `Instruments` (macOS) / `VTune` (Windows).

## Testing

```bash
cargo test -p tpt-container
```

- `corruption_never_panics_and_truncation_never_panics` — deterministic
  corruption/truncation sweeps over every byte-level parser (todo.md §24).
- `tests/property.rs` — random packet streams round-trip exactly
  (PTS/DTS/flags/payload), muxing is byte-identical across runs, and extreme
  or duplicate PTS values (up to `i64::MAX`) round-trip without panicking.

That audit found two real defects: `IDX `'s attacker-controlled
`entry_count` went straight into `Vec::with_capacity` (a few malformed bytes
could trigger a multi-gigabyte allocation), and `TMTA`'s length check used a
wrapping `usize` addition that passed on 64-bit but could wrap on the 32-bit
targets this workspace explicitly targets.

## Known limitations

- Not yet `no_std`.
- `IDX ` and `CRC1` are written on `finalize`; a live-appended stream may have
  neither until it is closed (the demuxer handles that).
- The header version rule is major/minor with fail-fast on anything newer;
  there is no compatibility mode (todo.md §24, §35).

## License

MIT OR Apache-2.0, at your option. See [LICENSE-MIT](../../LICENSE-MIT) and
[LICENSE-APACHE-2.0](../../LICENSE-APACHE-2.0).
