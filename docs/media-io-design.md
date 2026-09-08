# Media I/O Design

**Status:** implemented — `crates/tpt-media-io` (todo.md §27, done 2026-09-08).

**Purpose:** the one place `tpt-media` reads and writes media data. Sources
produce [`MediaItem`]s for the pipeline; sinks and containers consume them.
The crate deliberately reuses `tpt-dsp-io` for DSP-adjacent formats ('reuse,
don't duplicate', §27) and does **not** depend on `tpt-media-pipeline` — the
pipeline consumes these sources/sinks, never the other way round.

## Dependency picture

| Dependency          | Purpose                                            | Gate         |
| ------------------- | -------------------------------------------------- | ------------ |
| `tpt-media-core`    | `AudioFrame` / `VideoFrame` / `Timestamp` / formats | always       |
| `tpt-dsp-io`        | WAV `f32` bridge (`read_wav_f32_*` / `write_wav_f32_*`) | `std` feature |

`tpt-dsp-io` is std-only, so the bridge lives behind the crate's `std`
feature. Core PCM / raster / source / sink / WAV code is `no_std` + `alloc`.

## Error model

`IoResult<T>` = `Result<T, IoError>` mirrors `std::io` but stays `no_std`:

- `UnexpectedEof` — input ended mid-structure.
- `InvalidHeader` — syntactically malformed header.
- `Unsupported(&'static str)` — well-formed but deliberately rejected
  (e.g. `maxval != 255`, non-PPM/PGM pixel formats).
- `Overflow` — lengths/sizes that would overflow `usize`/`u32`.
- `Other(String)` — everything else, with a message.

`IoError: Display` always; `std::error::Error` under `std`.

## Layers

### PCM integration (`pcm.rs`)

`PcmSpec { sample_format, channels, sample_rate }` describes interleaved raw
samples. `encode_pcm`/`decode_pcm` move between `&[AudioFrame]` and raw bytes;
`decode_pcm` chunks the input and stamps monotonically increasing PTS
(`chunk_index * frames_per_chunk` ticks over a `1/sample_rate` timebase, §43
design's convention). `normalize_to_f32`/`denormalize_from_f32` convert to
`[-1, 1]`, the DSP domain.

**Bit-exactness:** `f32` is exact for ≤24-bit integer formats; 32-bit
integer loses the low ~8 bits by design (documented in `pcm.rs`).

### WAV (`wav.rs`, two paths)

1. **Byte-exact path** (`parse_wav` / `decode_wav` / `write_wav`) — the
   codec-pipeline path. Parses PCM and IEEE-float WAV into `AudioFrame`s
   preserving the original sample format and bits; `write_wav` re-emits an
   identical byte stream for supported profiles.
2. **`f32` bridge** (`decode_wav_f32` / `write_wav_f32`, `std`) — reuses
   `tpt-dsp-io`'s reader/writer (`WavSpec`/`WavData`, 32-bit IEEE float on
   write). This is the DSP/interop path and is exactly where §27's "reuse,
   don't duplicate" constraint bites.

### Image / video frame I/O (`raster.rs`)

Dependency-free PPM `P6` (RGB) and PGM `P5` (grayscale) readers/writers over
`VideoFrame`, plus `checkerboard` fixtures — the video-codec-pipeline
boundary until a real image library lands (§54). `Rgb24`/`Rgba32` → P6
(alpha dropped), `Gray8` → P5; `maxval != 255` is rejected.

**Ambiguity caveat (inherited from every PPM/PGM scanner):** a raster whose
first pixel byte is an ASCII whitespace value (`\t` `\n` `\r` `' '`) is
byte-indistinguishable from header trailing whitespace. Readers skip the
whitespace run after `maxval`; writers must avoid such first pixel bytes.

### Streaming sources (`source.rs`)

```rust
pub trait FrameSource {
    fn poll(&mut self) -> IoResult<Option<MediaItem>>;
}
```

Pull contract: call `poll` until `Ok(None)` (end of stream) or `Err`. This
mirrors the pipeline's `NodeEvent::Eos` and composes with sinks and the async
adapters. `MediaItem { Audio(AudioFrame), Video(VideoFrame) }`.

- [`SlicePcmReader`] — chunked PCM over a borrowed byte slice
  (allocation-free apart from emitted frames, `no_std`).
- [`FilePcmReader`] / [`StdinPcmReader`] (`std`) — the same chunking over any
  `Read`; reports an error if the stream ends mid-sample-frame.

### Streaming sinks (`sink.rs`)

```rust
pub trait FrameSink {
    fn write(&mut self, item: &MediaItem) -> IoResult<()>;
    fn finish(&mut self) -> IoResult<()>;
}
```

Push contract: `write` one item, `finish` for final flush.

- [`VecMediaSink`] — in-memory accumulator (tests, in-memory pipelines).
- [`FilePcmSink`] / [`StdoutPcmSink`] (`std`) — re-encodes audio frames as
  interleaved PCM bytes and writes with a 64 KiB buffer. This is the
  `-` / stdin-stdout streaming path for the CLI.

### Async integration (`async_io.rs`, `std`)

```rust
pub trait AsyncFrameSource { fn poll_next(&mut self) -> impl Future<Output = IoResult<Option<MediaItem>>> + Send; }
pub trait AsyncFrameSink   { fn write_item(&mut self, item: &MediaItem) -> impl Future<Output = IoResult<()>> + Send;
                             fn finish(&mut self) -> impl Future<Output = IoResult<()>> + Send; }
```

`async fn`-in-trait desugared to RPITIT with an explicit `+ Send` bound
(arbitrary auto-trait bounds are otherwise invisible to callers). No executor,
tokio, or `async_trait` dependency.

[`BlockingAdapter`] lifts a synchronous in-memory/non-blocking `FrameSource`
into an `AsyncFrameSource`. **Documented, not stubbed:** blocking file/device
reads must run on a dedicated I/O thread — the pipeline's `ThreadedExecutor`
provides this — and non-blocking file/network transports are deferred to the
streaming milestone.

## Chunking and timestamps

All chunking readers stamp PTS as `chunk_index * frames_per_chunk` over the
spec's `1/sample_rate` timebase, so a 48 kHz stream chunked at 1024 frames
yields PTS 0, 1024, 2048, … — deterministic and clockable against the
pipeline's `Timestamp`/`Duration` arithmetic.

## Test coverage & determinism

32 unit tests across all layers, feature-independent ordering, no sleeps/times
(exact counts and byte-level assertions): PCM round trips (bit-exact ≤24-bit,
F32 passthrough), WAV parse/write byte-exactness and the f32-bridge vs
byte-exact agreement test, raster round trips + CRLF/maxval/magic/EOF error
cases, source chunking/PTS and the mid-frame-EOF error, sink accumulation and
stdout-style writes, and the async blocking adapter driven by a hand-rolled
no-op-waker executor.

## Build matrix

| Configuration                     | Why                                   |
| --------------------------------- | ------------------------------------- |
| default (`std`)                   | bridge + file/stdin/stdout + async    |
| `--no-default-features`           | `no_std` + `alloc`; `cargo build` must warn-free |

Run: `cargo build -p tpt-media-io`, `cargo test -p tpt-media-io`,
`cargo build -p tpt-media-io --no-default-features`, `cargo clippy -p
tpt-media-io --all-targets`, `cargo fmt --check`.