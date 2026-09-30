# tpt-media-core

Shared media data types for the `tpt-media` stack (timestamps, packets, frames, tracks, metadata).

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg)](../../LICENSE-MIT)
[![Rust](https://img.shields.io/badge/rust-1.86%2B-orange.svg)](https://www.rust-lang.org)
[![no_std](https://img.shields.io/badge/no__std-compatible-green.svg)](https://doc.rust-lang.org/1.86.0/core/)

The vocabulary every other crate in the workspace speaks: if a type is not
here, two crates cannot agree on it. Part of the
[`tpt-media`](../../README.md) workspace — [workspace changelog](../../CHANGELOG.md) ·
[changelog](CHANGELOG.md) · [architecture](../../ARCHITECTURE.md) ·
[contributing](../../CONTRIBUTING.md).

## At a glance

| | |
|---|---|
| Role | Shared media data model (todo.md §15) |
| Status | Implemented; depended on by every other crate |
| Version | `0.1.0`, lockstep with the workspace (CONTRIBUTING §9.1) |
| MSRV | 1.86 — verified, enforced by CI's `msrv` job |
| License | MIT OR Apache-2.0 |
| `no_std` | Yes — builds for `wasm32-unknown-unknown` and `thumbv7em-none-eabi` (CI jobs `wasm`, `no_std`) |
| Unsafe | `#![forbid(unsafe_code)]` |

## What it provides

| Module | Types |
|---|---|
| `timebase` | `Timebase`, `Timestamp`, `Duration` — rational time, **never `f64`** |
| `packet` | `Packet`, `PacketFlags` — one encoded access unit |
| `frame` | `Frame`, `AudioFrame`, `VideoFrame`, `SideData` — one decoded access unit |
| `track` | `Track`, `TrackMedia`, `Stream`, `MediaType` |
| `codec_id` | `CodecId` (4-byte ASCII 4CC), `CodecConfig` |
| `sample_format` / `pixel_format` | `SampleFormat`, `PixelFormat` |
| `channel_layout` | `ChannelLayout`, `ChannelMask` |
| `metadata` | `Metadata`, `MetadataKey`, `MetadataValue` |
| `sync` | `ClockDomain`, `SyncPoint`, `SyncMap`, `SyncError` — A/V synchronisation (§16) |

`prelude::*` re-exports the common items in one `use`.

## Usage

Time is a rational `(ticks, timebase)` pair — conversions are explicit and
overflow-checked, and every duration operation saturates rather than wrapping:

```rust
use tpt_media_core::{CodecId, Packet, Timebase, Timestamp, Track, MediaType};

// 48 kHz audio clock.
let tb = Timebase { num: 1, den: 48_000 };
let pts = Timestamp { ticks: 96_000, timebase: tb };
assert_eq!(pts.rescale(tb).unwrap().ticks, 96_000);

let mut packet = Packet::empty(0, pts);
packet.data.extend_from_slice(&[0xAA, 0xBB]);
assert_eq!(packet.len(), 2);
assert!(!packet.is_keyframe());

let track = Track::new(0, MediaType::Audio, CodecId::new(*b"CIND"), tb);
```

Frames own their payload; the library never borrows caller data across calls:

```rust
use tpt_media_core::{AudioFrame, ChannelLayout, SampleFormat};

let mut frame = AudioFrame::alloc(256, SampleFormat::S16, ChannelLayout::Mono, 48_000);
frame.s16(0, 0)  // Option<i16>, None when out of bounds or not S16
```

## Feature flags

| Feature | Default | Effect |
|---|---|---|
| `std` | yes | `std::error::Error` impls for this crate's error types |

With `default-features = false` the crate is `no_std + alloc`. Features are
additive only (CONTRIBUTING §10).

## Design notes

- **Rational time, no floats.** A `f64` timestamp cannot represent 1 s at
  90 kHz as exactly 90 000 ticks; `Timebase`/`Timestamp` can, and
  `rescale` reports overflow instead of silently wrapping.
- **Checked or saturating, never wrapping.** `Duration` arithmetic saturates;
  `checked_{add,sub}_duration` and timestamp `rescale` return `Result`.
  Pinned by seeded-random property tests in `tests/property.rs`.
- **Caller-owned buffers.** Every container of samples/pixels is a `Vec` the
  caller can take back.

Read `ARCHITECTURE.md` §4 for the data-model design and todo.md §15/§16 for
the timestamp and synchronisation requirements.

## Testing

```bash
cargo test -p tpt-media-core
```

Covers timestamp overflow/saturation, rescale monotonicity, format and
channel-layout arithmetic, metadata limits, sync-map behaviour, and seeded-random
property tests over rescale/duration agreement.

## Known limitations

- No `f32`/`f64` timestamp escape hatch by design; conversions are explicit.
- `ChannelLayout` and `PixelFormat` cover the coded formats the native codecs
  support today (mono/stereo S16 audio; `Gray8`, `Yuv420` video), not the
  full format zoo.

## License

MIT OR Apache-2.0, at your option. See [LICENSE-MIT](../../LICENSE-MIT) and
[LICENSE-APACHE-2.0](../../LICENSE-APACHE-2.0).
