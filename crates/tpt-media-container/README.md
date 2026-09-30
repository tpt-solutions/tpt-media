# tpt-media-container

Generic container abstraction (`Demuxer`/`Muxer` traits) for the `tpt-media` stack.

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg)](../../LICENSE-MIT)
[![Rust](https://img.shields.io/badge/rust-1.86%2B-orange.svg)](https://www.rust-lang.org)
[![no_std](https://img.shields.io/badge/no__std-compatible-green.svg)](https://doc.rust-lang.org/1.86.0/core/)

Keeps codecs ignorant of the file format they are packaged in: a codec emits
and consumes `Packet`s, and something that implements `Container` decides
whether those packets live in `.tpt`, something else, or nothing at all. Part
of the [`tpt-media`](../../README.md) workspace —
[workspace changelog](../../CHANGELOG.md) · [changelog](CHANGELOG.md) ·
[architecture](../../ARCHITECTURE.md) · [contributing](../../CONTRIBUTING.md).

## At a glance

| | |
|---|---|
| Role | Container-format abstraction (todo.md §18/§20 boundary) |
| Status | Implemented; the `.tpt` implementation lives in [`tpt-container`](../../containers/tpt-container) |
| Version | `0.1.0`, lockstep with the workspace |
| MSRV | 1.86 — verified, enforced by CI's `msrv` job |
| License | MIT OR Apache-2.0 |
| `no_std` | Yes — builds for `wasm32-unknown-unknown` (CI job `wasm`) |
| Unsafe | `#![forbid(unsafe_code)]` |

## What it provides

- `Container` — the trait every container format implements:
  `stream()`, `next_packet()`, `write_packet()`, `finalize()`,
  `seek_to_keyframe()`, `tracks()`.
- `Chunk` — a four-byte ASCII chunk ID plus its body.
- `ContainerError` / `ContainerResult` — `Truncated`, `InvalidHeader`,
  `MalformedChunk`, `Unsupported`, `Io`.

Deliberately small. The format-specific work — box layout, indexes, CRC,
streaming, seeking — belongs in the format crate, not here.

## Usage

```rust
use tpt_media_container::{Container, ContainerResult};
use tpt_media_core::{Packet, Stream};
use tpt_media_codec::CodecResult;

struct MyContainer {
    stream: Stream,
    // ...format state...
}

impl Container for MyContainer {
    fn stream(&self) -> &Stream {
        &self.stream
    }
    fn next_packet(&mut self) -> ContainerResult<Packet> {
        // Truncated input is `Err(ContainerError::Truncated)`, never a panic.
        unimplemented!()
    }
    fn write_packet(&mut self, packet: &Packet) -> ContainerResult<()> {
        unimplemented!()
    }
    fn finalize(&mut self) -> ContainerResult<()> {
        unimplemented!()
    }
    fn seek_to_keyframe(&mut self, target_pts: i64) -> ContainerResult<()> {
        unimplemented!()
    }
    fn tracks(&self) -> &[tpt_media_core::Track] {
        &self.stream.tracks
    }
}
```

## Feature flags

| Feature | Default | Effect |
|---|---|---|
| `std` | yes | `std` conveniences |

`no_std + alloc` with `default-features = false`.

## Design notes

`ARCHITECTURE.md` §7 documents the real container behaviour (index/`IDX `,
`CRC1`, streaming) and `docs/container-design.md` is the full `.tpt`
specification. The dependency direction matters: probe → container, never the
reverse — that edge was inverted in 2026-09 because `tpt-container` had an
unused dependency on probe.

## Testing

```bash
cargo test -p tpt-media-container
```

The interesting tests live in the format crate, which runs corruption and
truncation sweeps over every byte-level parser (todo.md §24, §38). Two real
allocation/overflow bugs were found that way (`IDX ` entry count passed
straight to `Vec::with_capacity`; a `TMTA` length checked with wrapping
`usize` addition on 32-bit targets).

## Known limitations

- One method set covers read, write and seek; a read-only format
  implementation returns `Unsupported` from the write side.
- `ContainerError::Io(&'static str)` carries no `io::Error` payload — by
  design for `no_std`, at the cost of detail on I/O failures.

## License

MIT OR Apache-2.0, at your option. See [LICENSE-MIT](../../LICENSE-MIT) and
[LICENSE-APACHE-2.0](../../LICENSE-APACHE-2.0).
