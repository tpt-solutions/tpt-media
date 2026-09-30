# tpt-media-io

PCM/WAV/raw audio and image frame I/O plus streaming sources/sinks for the `tpt-media` stack.

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg)](../../LICENSE-MIT)
[![Rust](https://img.shields.io/badge/rust-1.86%2B-orange.svg)](https://www.rust-lang.org)
[![no_std](https://img.shields.io/badge/no__std-compatible-green.svg)](https://doc.rust-lang.org/1.86.0/core/)

The one place `tpt-media` reads and writes media data. It reuses `tpt-dsp-io`
for the WAV/`f32` bridge rather than duplicating it, and deliberately does
**not** depend on `tpt-media-pipeline` — the pipeline consumes these
sources/sinks, never the other way round. Part of the
[`tpt-media`](../../README.md) workspace —
[workspace changelog](../../CHANGELOG.md) · [changelog](CHANGELOG.md) ·
[architecture](../../ARCHITECTURE.md) · [contributing](../../CONTRIBUTING.md).

## At a glance

| | |
|---|---|
| Role | Media I/O (todo.md §27) |
| Status | Implemented |
| Version | `0.1.0`, lockstep with the workspace |
| MSRV | 1.86 — verified, enforced by CI's `msrv` job |
| License | MIT OR Apache-2.0 |
| `no_std` | Yes — `no_std + alloc` core, `std` adds file/stdin/stdout and the `f32` bridge |
| Unsafe | `#![forbid(unsafe_code)]` |
| Extra dependency | `tpt-dsp-io`, behind the default `std` feature |

## Layers

| Module | Contents |
|---|---|
| `pcm` | `PcmSpec`, `encode_pcm`/`decode_pcm` (chunked, monotonic PTS), `normalize_to_f32`/`denormalize_from_f32` |
| `wav` | Byte-exact `parse_wav`/`decode_wav`/`write_wav` for the codec pipeline; std-gated `decode_wav_f32`/`write_wav_f32` bridge reusing `tpt-dsp-io` |
| `raster` | Dependency-free PPM `P6` / PGM `P5` readers/writers over `VideoFrame`, plus `checkerboard` fixtures |
| `source` | `FrameSource` pull trait, `MediaItem`, `SlicePcmReader`, and `FilePcmReader`/`StdinPcmReader` (std) |
| `sink` | `FrameSink` push trait, `VecMediaSink`, and `FilePcmSink`/`StdoutPcmSink` (std) |
| `async_io` | `AsyncFrameSource`/`AsyncFrameSink` (RPITIT + `Send`, dependency-free) and a synchronous `BlockingAdapter` (std) |

## Usage

```rust
use tpt_media_io::{decode_wav, encode_pcm, PcmSpec};
use tpt_media_core::{ChannelLayout, SampleFormat};

let (frames, spec) = decode_wav(&wav_bytes)?;
let bytes = encode_pcm(&frames, spec)?;
```

Streaming, end to end:

```rust
use tpt_media_io::{FrameSource, MediaItem, SlicePcmReader};
use tpt_media_core::{ChannelLayout, SampleFormat, Timebase};

let spec = PcmSpec { sample_format: SampleFormat::S16, channels: 1, sample_rate: 48_000 };
let mut reader = SlicePcmReader::new(&pcm_bytes, spec, 1024)?;
while let Some(MediaItem::Audio(frame)) = reader.poll()? {
    // chunked, monotonically timestamped
}
```

A runnable tour of the byte-exact WAV and PPM round trips is in
`examples/io_tour.rs`:

```bash
cargo run -p tpt-media-io --example io_tour
```

## Error model

`IoResult<T> = Result<T, IoError>` mirrors `std::io` but stays `no_std`:
`UnexpectedEof`, `InvalidHeader`, `Unsupported(&'static str)`, `Overflow`,
`Other(String)`. `Display` always; `std::error::Error` under `std`.

## Feature flags

| Feature | Default | Effect |
|---|---|---|
| `std` | yes | `tpt-dsp-io` bridge, file/stdin/stdout sources and sinks, `async_io`, `probe`-style path helpers |

`no_std + alloc` with `default-features = false` keeps `pcm`, `raster`,
`source`, `sink` and the byte-exact `wav` path.

## Design notes

- **Reuse, don't duplicate.** The `f32` WAV bridge calls `tpt-dsp-io`'s
  reader/writer; the only thing implemented here is the mapping between
  `AudioFrame` and that crate's types.
- **Bit-exactness.** `f32` normalisation is exact for integer formats up to
  24 bits; 32-bit integer input loses its low ~8 bits by design, which is
  documented in `pcm.rs` rather than hidden.
- **PPM/PGM caveat** (inherited from every such scanner): a raster whose
  first pixel byte is ASCII whitespace is byte-indistinguishable from header
  trailing whitespace. Readers skip the whitespace run after `maxval`;
  writers must avoid such first pixel bytes.

Full design: [`docs/media-io-design.md`](../../docs/media-io-design.md).

## Testing

```bash
cargo test -p tpt-media-io
```

32 unit tests: PCM round trips and chunked PTS, WAV byte-exactness and
header rejection, raster round trips and the whitespace-ambiguity case,
source/sink contracts including a stream that ends mid-sample-frame, and the
blocking adapter.

## Known limitations

- Image I/O is PPM/PGM only — a deliberate dependency-free stopgap until a
  real image library lands; there is no JPEG/PNG support.
- No async runtime dependency: `async_io` is trait-level (RPITIT) plus a
  blocking adapter, so wiring it to a specific executor is the caller's job.

## License

MIT OR Apache-2.0, at your option. See [LICENSE-MIT](../../LICENSE-MIT) and
[LICENSE-APACHE-2.0](../../LICENSE-APACHE-2.0).
