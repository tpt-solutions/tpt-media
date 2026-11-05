# tpt-media-codec

Generic codec framework: `Encoder`, `Decoder`, `Codec` traits, capabilities, resource limits.

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg)](../../LICENSE-MIT)
[![Rust](https://img.shields.io/badge/rust-1.86%2B-orange.svg)](https://www.rust-lang.org)
[![no_std](https://img.shields.io/badge/no__std-compatible-green.svg)](https://doc.rust-lang.org/1.86.0/core/)

The seam every codec plugs into: it defines what an encoder/decoder *is*, what
it claims it can do, and what it refuses to be asked beyond. Part of the
[`tpt-media`](../../README.md) workspace —
[workspace changelog](../../CHANGELOG.md) · [changelog](CHANGELOG.md) ·
[architecture](../../ARCHITECTURE.md) · [contributing](../../CONTRIBUTING.md).

## At a glance

| | |
|---|---|
| Role | Codec abstraction + registry (todo.md §18, §19) |
| Status | Implemented; implemented by Aura, Cinder, Mote, Reed, Thrum, Vault |
| Version | `0.2.0`, lockstep with the workspace |
| MSRV | 1.86 — verified, enforced by CI's `msrv` job |
| License | MIT OR Apache-2.0 |
| `no_std` | Yes — builds for `wasm32-unknown-unknown` (CI job `wasm`) |
| Unsafe | `#![forbid(unsafe_code)]` |

## What it provides

- `Encoder` / `Decoder` traits — `info()`, `encode`/`decode`, `flush`, `drain`.
- `CodecInfo`, `CodecConfig`, `SupportedFormat`, `Capabilities`.
- `CodecError` / `CodecResult` — one error model for every codec
  (`MalformedPacket`, `NeedMoreInput`, `Cancelled`, `ResourceLimit`,
  `Unsupported`, `InvalidConfig`, `Internal`, `Bitstream`, `Downstream`).
- `ResourceLimits` with `permissive()` / `strict()` presets and per-check
  helpers (`check_packet`, `check_frame_bytes`, `check_reference_frames`,
  `check_allocation`, `check_recursion_depth`, `check_metadata`).
- Type-erased `DynEncoder` / `DynDecoder` + `Registry` for codec-agnostic
  callers, and `DynFrame` for payloads whose media type is only known at
  runtime.
- `RealtimeDecoder` / `RealtimeEncoder` — opt-in caller-buffer traits for
  real-time code that must not allocate per packet (see below).
- `CancelToken` and the `Cancellable*` adapters — cooperative cancellation at
  call granularity (see below).

## Usage

Implementing a codec means implementing two traits and stating your formats:

```rust
use tpt_media_codec::{Capabilities, CodecInfo, CodecResult, Decoder, Encoder, SupportedFormat};
use tpt_media_core::{AudioFrame, CodecConfig, Packet};

pub struct MyCodec {
    info: CodecInfo,
    // ...decoder state (reference frames, history)...
}

impl Decoder for MyCodec {
    type Frame = AudioFrame;

    fn info(&self) -> &CodecInfo {
        &self.info
    }

    fn decode(&mut self, packet: &Packet) -> CodecResult<Vec<AudioFrame>> {
        // Malformed input returns CodecError, it never panics.
        Ok(Vec::new())
    }
}
```

Registering implementations so codec-agnostic callers can find them:

```rust
use tpt_media_codec::Registry;

let mut registry = Registry::new();
registry.register_decoder(tpt_audio_cinder::CinderDynDecoder::default());
registry.register_encoder(tpt_audio_aura::AuraDynEncoder::default());
```

`Capabilities` is a claim, not a hint: `real_time_decoder` /
`real_time_encoder` are what `docs/performance.md` and the C ABI's
`tpt_media_config` mapping rely on. A codec whose encoder is not real-time
(Reed, Thrum) says so — `real_time_encoder: false`.

## Caller-buffer real-time traits

`Encoder::encode` and `Decoder::decode` return owned `Vec`s, which allocates on
every packet. A codec that claims real-time operation can also implement the
opt-in companion traits, which write into storage the caller owns:

- `RealtimeDecoder` — `max_output_bytes()` and `decode_into(packet, out)`. The
  caller allocates `out` once. A short buffer is refused before any decoder
  state changes.
- `RealtimeEncoder` — `push_frame`, then `pop_packet_into` until it returns
  `None`, then `drain_packet_into` for the tail. `max_packet_bytes()` gives the
  size of the output buffer.

Implemented by Cinder (both), and by Mote, Reed and Thrum on the decode side.
The owned traits are unchanged, and each implementation's output is
byte-identical to its owned path. Design: `docs/caller-buffer-design.md`.

## Cancellation

`CancelToken` is a shared one-way flag. Wrap a codec in `CancellableDecoder`
or `CancellableEncoder` (or the `Dyn` variants) and hold a clone of the token.
Each `decode`, `encode`, `flush` and `drain` call checks the flag first and
returns `CodecError::Cancelled` once it is set. A call in progress is never
interrupted, and there is no wall-clock deadline, so output never depends on the
machine's speed.

## `flush` vs `drain`

Both exist because end-of-stream and stream-reuse are different problems:

- `drain()` — graceful end-of-stream: emit whatever is still buffered, keep
  reference frames and history. Codecs with no look-ahead (every current
  TPT-native codec) inherit the default, which forwards to `flush`.
- `flush()` — hard reset: discard internal state so the codec can be reused
  on an unrelated stream.

## Resource limits

`ResourceLimits::strict()` is the `Default`. It exists because this crate is
the place untrusted bytes enter the stack: frame size, packet size, reference
frame count, allocation, recursion depth, CPU budget and metadata size are all
bounded *before* any work is done on them. `docs/security.md` records the
audits that produced and tightened these fields.

## Feature flags

| Feature | Default | Effect |
|---|---|---|
| `std` | yes | `std::error::Error` for `CodecError` |

`no_std + alloc` with `default-features = false`.

## Design notes

`ARCHITECTURE.md` §6 carries the trait sketches and the type-erasure story;
`docs/codec-design.md` carries the codec family design matrix (the
`supported_formats` tables in code are binding, not the prose matrix).

## Testing

```bash
cargo test -p tpt-media-codec
```

Covers the error model, both limit presets and every check helper, capability
flags, the type-erased wrappers, the registry's factory dispatch, and the
cancellation adapters. `tests/dyn_contract.rs` drives the type-erased path from
outside the crate.

## Known limitations

- Pre-1.0, so no API-stability guarantee (CONTRIBUTING §9.2): items go
  `#[deprecated]` with a migration note before they are ever removed.
- `DynFrame` carries no timestamps — the C ABI associates PTS at the
  boundary instead.

## License

MIT OR Apache-2.0, at your option. See [LICENSE-MIT](../../LICENSE-MIT) and
[LICENSE-APACHE-2.0](../../LICENSE-APACHE-2.0).
