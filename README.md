# tpt-media

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](./LICENSE-MIT)
[![Rust](https://img.shields.io/badge/rust-1.75%2B-orange.svg)](https://www.rust-lang.org)
[![Build Status](https://img.shields.io/badge/build-WIP-yellow.svg)](./.github/workflows/ci.yml)

A 100% patent-free codec suite for audio and video, written in safe Rust.

## The 3-Niche Paradigm

tpt-media does **not** ship a single "Swiss Army Knife" codec. Instead, each
domain has three purpose-built tiers:

| Tier | Audio               | Video                | Target                                |
|------|---------------------|----------------------|---------------------------------------|
| 1    | `tpt-audio-pulse`   | `tpt-video-livelink` | Real-time (WebRTC, gaming, conf)       |
| 2    | `tpt-audio-aura`    | `tpt-video-vault`    | High-fidelity / archival              |
| 3    | `tpt-audio-vox`     | `tpt-video-whisper`  | Edge / IoT (microcontrollers, drones) |

## Repository Layout

```
tpt-media/
├── Cargo.toml                 # Workspace root
├── crates/
│   ├── tpt-core/             # Bitstream, entropy, math, container
│   ├── tpt-audio-pulse/
│   ├── tpt-audio-aura/
│   ├── tpt-audio-vox/
│   ├── tpt-video-livelink/
│   ├── tpt-video-vault/
│   ├── tpt-video-whisper/
│   └── tpt-media-sys/         # FFI bridge (cbindgen)
├── apps/
│   ├── tpt-cli/               # `tpt encode/decode/probe`
│   └── tpt-web/               # WASM browser player
└── tests/                     # Cross-crate integration tests
```

## License

Dual-licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE-2.0](./LICENSE-APACHE-2.0))
- MIT license ([LICENSE-MIT](./LICENSE-MIT))

at your option.

Copyright (c) 2026 TPT Solutions.

## Status

Early development — see [`todo.md`](./todo.md) for the phase-by-phase
implementation checklist and [`spec.txt`](./spec.txt) for the full technical
design specification.

## Contributing

Read [`CONTRIBUTING.md`](./CONTRIBUTING.md) before opening a pull request.
The patent-compliance rules are non-negotiable.