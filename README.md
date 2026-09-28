# tpt-media

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE-MIT)
[![Rust](https://img.shields.io/badge/rust-1.75%2B-orange.svg)](https://www.rust-lang.org)
[![no_std](https://img.shields.io/badge/no__std-compatible-green.svg)](#)

The native TPT media framework and codec ecosystem.

`tpt-media` is the home of the TPT-native media stack: shared media data models,
timestamps, packet/frame abstractions, codec interfaces, the `.tpt` container,
bitstream and entropy primitives, media pipelines, native TPT codecs, a probing
library, and the `tpt` command-line tool. A WASM browser player (`tpt-web`) and
a stable C ABI (`tpt-media-sys`) are the next milestones (see
[`todo.md`](todo.md) §45/§46).

`tpt-media` is **not** a general-purpose DSP library. All transforms,
filters, signal analysis and SIMD live in the companion [`tpt-dsp`](https://github.com/TPT-Solutions/tpt-dsp)
crate, which `tpt-media` depends on. Standards-oriented media implementations
live in the companion [`tpt-kinetix`](https://github.com/TPT-Solutions/tpt-kinetix)
crate. The intended stack is:

```text
                     TPT APPLICATIONS
                            │
              ┌─────────────┼─────────────┐
              │             │             │
       tpt-government    tpt-kinetix    tpt-media
                                         │
                                  Native TPT Media
                                         │
                                         ▼
                                      tpt-dsp
```

## Ecosystem

Only the crates implemented so far are workspace members; the video codecs,
the WASM player, and the FFI bridge land in their `todo.md` order and are
marked *(planned)* below.

| Crate                  | Purpose                                                 |
| ---------------------- | ------------------------------------------------------- |
| `tpt-media-core`       | Media data models (Timestamp, Frame, Packet, Track)     |
| `tpt-media-bitstream`  | Bit reader/writer, Huffman, Rice, range coder            |
| `tpt-media-codec`      | Generic codec interfaces (`Encoder`, `Decoder`)         |
| `tpt-media-container`  | Generic container abstraction                           |
| `tpt-media-pipeline`   | Node-based pipeline (Source → Demux → Decode → Sink)    |
| `tpt-media-io`         | PCM/WAV/raw I/O integration                             |
| `tpt-media-probe`      | Container + codec introspection                         |
| `tpt-container`        | The native `.tpt` container format                      |
| `tpt-media-representation` | Swappable signal-representation trait contracts (research) |
| `tpt-media-research`   | Representation research harness (`docs/representation-research.md`) |
| `tpt-audio-pulse`      | Real-time low-latency audio codec                       |
| `tpt-audio-aura`       | High-fidelity / archival audio (lossless + lossy)       |
| `tpt-audio-vox`        | Extremely low-bandwidth speech codec (`no_std` LPC vocoder) |
| `tpt-cli`              | Command-line encoder/decoder/probe                      |
| `tpt-video-livelink`   | *(planned)* Real-time video communication               |
| `tpt-video-vault`      | *(planned)* High-compression archival video             |
| `tpt-video-whisper`    | *(planned)* Extreme-edge dictionary-coded video         |
| `tpt-web`              | *(planned)* WASM browser player                         |
| `tpt-media-sys`        | *(planned)* Stable C ABI                                |

## Building

```bash
# Format, lint, build, and test the entire workspace
cargo fmt --all
cargo clippy --all-targets --locked -- -D warnings
cargo build --workspace --locked
cargo test  --workspace --locked
```

The `tpt-audio-vox` crate is `no_std`-compatible and can additionally be built
for bare-metal targets:

```bash
cargo build -p tpt-audio-vox --target thumbv7em-none-eabi --no-default-features
```

## License

Dual-licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE-2.0](LICENSE-APACHE-2.0))
- MIT License ([LICENSE-MIT](LICENSE-MIT))

at your option.

Copyright 2026 TPT Solutions.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for the clean-room development rules,
patent-conscious design policy, and provenance requirements.

## Architecture

See [ARCHITECTURE.md](ARCHITECTURE.md) for the long-form design document and
[`docs/`](docs/) for per-area design notes.