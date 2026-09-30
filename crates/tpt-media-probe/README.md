# tpt-media-probe

Container and codec introspection for the `tpt-media` stack.

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg)](../../LICENSE-MIT)
[![Rust](https://img.shields.io/badge/rust-1.86%2B-orange.svg)](https://www.rust-lang.org)
[![no_std](https://img.shields.io/badge/no__std-compatible-green.svg)](https://doc.rust-lang.org/1.86.0/core/)

Given raw bytes, produce a structured report: which container, which tracks,
which codecs and bitstream versions, sample rates, dimensions, timestamps,
durations, metadata, integrity status, and whether a decoder for each codec is
actually registered. This is the library behind `tpt probe` / `tpt inspect`.
Part of the [`tpt-media`](../../README.md) workspace —
[workspace changelog](../../CHANGELOG.md) · [changelog](CHANGELOG.md) ·
[architecture](../../ARCHITECTURE.md) · [contributing](../../CONTRIBUTING.md).

## At a glance

| | |
|---|---|
| Role | Container + codec introspection (todo.md §44) |
| Status | Implemented |
| Version | `0.1.0`, lockstep with the workspace |
| MSRV | 1.86 — verified, enforced by CI's `msrv` job |
| License | MIT OR Apache-2.0 |
| `no_std` | Yes — builds for `wasm32-unknown-unknown` (CI job `wasm`) |
| Unsafe | `#![forbid(unsafe_code)]` |

## Usage

```rust
use tpt_media_probe::{probe, probe_file, probe_with_registry};
use tpt_media_codec::Registry;

// From bytes in memory:
let report = probe(&bytes);
println!("{} track(s), {:?}", report.tracks.len(), report.duration);

// From disk (std):
let report = probe_file("input.tpt")?;

// Resolve codec capabilities against real implementations:
let mut registry = Registry::new();
registry.register_decoder(tpt_audio_cinder::CinderDynDecoder::default());
let report = probe_with_registry(&bytes, &registry);
```

`probe` always returns a `ProbeReport` — malformed input produces a report
with the failure recorded, never a panic or an `Err`. Hostile timestamp
streams are handled explicitly rather than assumed away (see *Testing*).

## What the report contains

- `ContainerKind` — `Tpt`, `Wav`, or `Unknown`.
- Per-track info: `AudioTrackInfo`, `VideoTrackInfo`, or generic `TrackInfo`
  for subtitle/metadata tracks — codec ID **and bitstream version**, sample
  rate, channel layout, dimensions, pixel format, first/last PTS, frame and
  sample counts, name, language, typed `TMTA` metadata.
- Duration, plus overall and per-track bitrate estimates.
- Integrity: container `CRC1` footer status and per-cluster checksum failures.
- `CodecCapabilityReport` per codec found: human name, whether the ID is
  TPT-native, and whether an encoder/decoder is registered in the `Registry`.

For `.tpt`, every field comes from the container's real `TRAK`/`TMTA`
descriptors and a full packet scan — it reflects what is actually in the file,
not a guess.

## Feature flags

| Feature | Default | Effect |
|---|---|---|
| `std` | yes | `probe_file` |

`no_std + alloc` with `default-features = false`.

## Design notes

Dependency direction: probe → container, never the reverse. `tpt-container`
had an unused dependency on probe that was inverted in 2026-09.

## Testing

```bash
cargo test -p tpt-media-probe
```

`tests/timestamps.rs` is the interesting one (§47): `probe()` survives
extreme, duplicated, descending and mixed-sign PTS containers, and reports
finite, non-negative durations. One unsigned-envelope quirk is documented
rather than hidden. A std-build bug where `format!`/`ToString` came from the
std prelude — invisible until the crate was actually built for
`wasm32-unknown-unknown` with `std` off — is fixed.

## Known limitations

- Only `.tpt` and WAV are recognised; other formats report
  `ContainerKind::Unknown` with whatever could be read.
- Bitrate and sample-count figures are estimates derived from duration and
  packet counts, labelled as such.
- `probe` without a registry reports codecs as unavailable even when an
  implementation exists — use `probe_with_registry`. The `tpt` CLI does, so
  its capability lines reflect the codecs that binary carries.

## License

MIT OR Apache-2.0, at your option. See [LICENSE-MIT](../../LICENSE-MIT) and
[LICENSE-APACHE-2.0](../../LICENSE-APACHE-2.0).
