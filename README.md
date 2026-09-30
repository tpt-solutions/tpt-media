# tpt-media

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE-MIT)
[![Rust](https://img.shields.io/badge/rust-1.86%2B-orange.svg)](https://www.rust-lang.org)
[![no_std](https://img.shields.io/badge/no__std-compatible-green.svg)](#)

The native TPT media framework and codec ecosystem.

`tpt-media` is the home of the TPT-native media stack: shared media data models,
timestamps, packet/frame abstractions, codec interfaces, the `.tpt` container,
bitstream and entropy primitives, media pipelines, native TPT codecs, a probing
library, the `tpt` command-line tool, and a stable C ABI (`tpt-media-sys`,
§46). A WASM browser player (`tpt-web`) is the remaining milestone (see
[`todo.md`](todo.md) §45).

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
| [`tpt-media-core`](crates/tpt-media-core)       | Media data models (Timestamp, Frame, Packet, Track)     |
| [`tpt-media-bitstream`](crates/tpt-media-bitstream)  | Bit reader/writer, Huffman, Rice, range coder            |
| [`tpt-media-codec`](crates/tpt-media-codec)      | Generic codec interfaces (`Encoder`, `Decoder`)         |
| [`tpt-media-container`](crates/tpt-media-container)  | Generic container abstraction                           |
| [`tpt-media-pipeline`](crates/tpt-media-pipeline)   | Node-based pipeline (Source → Demux → Decode → Sink)    |
| [`tpt-media-io`](crates/tpt-media-io)         | PCM/WAV/raw I/O integration                             |
| [`tpt-media-probe`](crates/tpt-media-probe)      | Container + codec introspection                         |
| [`tpt-container`](containers/tpt-container)        | The native `.tpt` container format                      |
| [`tpt-media-representation`](crates/tpt-media-representation) | Swappable signal-representation trait contracts (research) |
| [`tpt-media-research`](crates/tpt-media-research)   | Representation research harness (`docs/representation-research.md`) |
| [`tpt-audio-cinder`](codecs/cinder)      | Real-time low-latency audio codec                       |
| [`tpt-audio-aura`](codecs/aura)       | High-fidelity / archival audio (lossless + lossy)       |
| [`tpt-audio-reed`](codecs/reed)        | Extremely low-bandwidth speech codec (`no_std` LPC vocoder) |
| [`tpt-video-thrum`](codecs/thrum)   | Real-time video communication (I/P, intra refresh, 8x8 DCT)  |
| [`tpt-cli`](apps/tpt-cli)              | Command-line encoder/decoder/probe                      |
| [`tpt-video-vault`](../tpt-media/codecs/vault)      | *(planned)* High-compression archival video             |
| [`tpt-video-mote`](../tpt-media/codecs/mote)    | *(planned)* Extreme-edge dictionary-coded video         |
| [`tpt-web`](../tpt-media/apps/tpt-web)              | *(planned)* WASM browser player                         |
| [`tpt-media-sys`](ffi/tpt-media-sys)       | Stable C ABI (§46: opaque handles, error codes, C header)  |

Each crate link goes to its own `README.md`, which carries the usage examples,
feature flags, testing entry points and known limitations for that crate, and
links onward to the shared design documents. Each crate also carries its own
`CHANGELOG.md`, scoped to that crate — see
[CONTRIBUTING §9.4](CONTRIBUTING.md#94-per-crate-documentation) for how those
relate to the workspace-level [CHANGELOG.md](CHANGELOG.md).

## Building

```bash
# Format, lint, build, and test the entire workspace
cargo fmt --all
cargo clippy --all-targets --locked -- -D warnings
cargo build --workspace --locked
cargo test  --workspace --locked
```

The `tpt-audio-reed` crate is `no_std`-compatible and can additionally be built
for bare-metal targets:

```bash
cargo build -p tpt-audio-reed --target thumbv7em-none-eabi --no-default-features
```

## License

Dual-licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE-2.0](LICENSE-APACHE-2.0))
- MIT License ([LICENSE-MIT](LICENSE-MIT))

at your option.

Copyright 2026 TPT Solutions.

## Contributing

Development is **issues only** — we do not accept external code
contributions. See [CONTRIBUTING.md](CONTRIBUTING.md) for how to report
bugs or propose work, and for the clean-room development rules,
patent-conscious design policy, and provenance requirements the code
is held to.

## Architecture

See [ARCHITECTURE.md](ARCHITECTURE.md) for the long-form design document and
[`docs/`](docs/) for per-area design notes.