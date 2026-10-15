# tpt-audio-aura

Aura: TPT high-fidelity / archival audio codec (lossless LPC + lossy MDCT).

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg)](../../LICENSE-MIT)
[![Rust](https://img.shields.io/badge/rust-1.86%2B-orange.svg)](https://www.rust-lang.org)

The archival end of the native audio family: a **bit-exact lossless** mode
and a psychoacoustically driven lossy mode. Part of the
[`tpt-media`](../../README.md) workspace —
[workspace changelog](../../CHANGELOG.md) · [changelog](CHANGELOG.md) ·
[codec design](../../docs/codec-design.md) · [provenance](../../docs/provenance.md) ·
[contributing](../../CONTRIBUTING.md).

## At a glance

| | |
|---|---|
| Role | Native archival audio codec (todo.md §30) |
| Codec ID | `AURA` · bitstream `VERSION = 1` |
| Status | Implemented |
| Version | `0.1.0`, lockstep with the workspace |
| MSRV | 1.86 — verified, enforced by CI's `msrv` job |
| License | MIT OR Apache-2.0 |
| Modes | `lossless` (LPC + adaptive Rice), `lossy` (MDCT + Bark masking + noise shaping) |
| Real-time | **Neither** encoder nor decoder claimed real-time — archival by design |
| Measured peak memory | 25 330 B against a documented < 256 KiB budget |

## Modes

**Lossless** — autocorrelation LPC analysis (Levinson-Durbin) up to order 32,
residuals adaptive-Rice-coded after a zigzag map, with one Rice parameter per
256-sample partition. The LPC analysis is Hann-windowed; prediction uses the
unwindowed samples. Round trips are **bit-exact**,
pinned by tests and used as the workspace's reference lossless path (including
the C ABI's integration test).

**Lossy** — large-window MDCT (2048/4096) in `f64`, with each bin mapped to a
Bark critical band, bands classified tonal/noise, simultaneous masking
computed across the Bark axis, and the resulting per-band threshold setting
that band's quantisation step. First-order spectral noise shaping pushes
quantisation error toward already-noisy neighbours. The per-band step table
is transmitted (32-bit float per band), so the decoder needs no side
information beyond what is in the packet.

## Usage

```rust
use tpt_audio_aura::{AuraConfig, AuraDecoder, AuraEncoder, AuraMode};
use tpt_media_codec::{Decoder, Encoder};
use tpt_media_core::CodecConfig;

let cfg = CodecConfig::new(*b"AURA", 1);

let mut enc = AuraEncoder::new(cfg, AuraConfig { mode: AuraMode::Lossless, ..Default::default() })?;
let packets = enc.encode(&audio_frame)?;

let mut dec = AuraDecoder::new(cfg, enc_cfg)?;
let frames = dec.decode(&packets[0])?;
```

Enable the mode you need explicitly — the `lossless` and `lossy` features
gate which encode paths are *compiled*:

```toml
[dependencies]
tpt-audio-aura = { version = "0.1", default-features = false, features = ["std", "lossless"] }
```

Neither mode is a silently-degraded version of the other (CONTRIBUTING §10).

## Wire format and versioning

Every packet leads with a 16-bit bitstream version, rejected up front
(todo.md §35, fail-fast). Each packet ends with a 4-byte CRC-32 trailer
(`CRC_TRAILER_BYTES`) over all preceding payload bytes, verified before
anything is parsed.

**Breaking wire changes that have shipped** — recorded because valid bytes
changed, not just the API:

- Lossy quantisation was reworked from flat 16-bit-per-coefficient steps to
  the per-band masking-driven table above. Pre-change lossy packets are
  **not decodable** by the current decoder.
- The CRC-32 trailer was added; packets without it are rejected.
- The 16-bit bitstream version field was added.

## Benchmarks & profiling

```bash
cargo bench -p tpt-audio-aura
```

`benches/codec_bench.rs` — lossless encode/decode of 32 × 1024-sample blocks
at 48 kHz (2026-09-14, release profile, criterion):

| bench | median | per-item | real-time factor |
|---|---:|---:|---:|
| `aura_lossless/encode` (32 × 1024 samples = 0.68 s) | 1.80 ms | 56 µs/block | ~380× |
| `aura_lossless/decode` (32 packets) | 2.37 ms | 74 µs/packet | ~290× |

Lossless decode is ~4× Cinder's decode: LPC synthesis plus Rice decoding per
residual, versus band-quantised MDCT coefficients. Aura's profile is
archival, not real-time. Tables:
[`docs/performance.md`](../../docs/performance.md).

Profiling recipe: `cargo bench -p tpt-audio-aura`, then `cargo flamegraph
--bench codec_bench` (Linux) / `Instruments` (macOS) / `VTune` (Windows).

## Testing

```bash
cargo test -p tpt-audio-aura
```

- Bit-exact lossless round trips over the §36 corpus.
- Seeded-random property tests, encoder/decoder determinism at the packet
  level, silence stays silent.
- Corruption/truncation/bit-flip sweeps; these caught a real out-of-bounds
  write in the lossy path.
- Codec-private (`block_size`) round trip, CRC rejection tests.

## Known limitations

- **Lossless is mono or stereo; lossy is mono only.** A stereo lossless packet picks
  L/R, L/S, R/S or M/S per block (FLAC's four modes), by exact cost. The lossy MDCT
  path has no stereo model yet.
- The lossy path uses `f64` and large windows — it is an archival encoder, not
  a real-time one, and `Capabilities::real_time_encoder` says so.
- Packets are independent: each block carries its own LPC coefficients and
  starts with zero history. Carrying history across packets would save about
  5% but would make a packet undecodable on its own, so it is not done.

## License

MIT OR Apache-2.0, at your option. See [LICENSE-MIT](../../LICENSE-MIT) and
[LICENSE-APACHE-2.0](../../LICENSE-APACHE-2.0).
