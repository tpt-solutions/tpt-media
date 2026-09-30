# tpt-video-thrum

Thrum: TPT low-latency video communication codec (I/P frames, intra refresh, 8x8 DCT).

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg)](../../LICENSE-MIT)
[![Rust](https://img.shields.io/badge/rust-1.86%2B-orange.svg)](https://www.rust-lang.org)
[![no_std](https://img.shields.io/badge/no__std-compatible-green.svg)](https://doc.rust-lang.org/1.86.0/core/)

The video codec in the native family, and the one built for a *conversation*
rather than for compression efficiency: every design choice trades coding gain
for latency and bounded work. Part of the
[`tpt-media`](../../README.md) workspace —
[workspace changelog](../../CHANGELOG.md) · [changelog](CHANGELOG.md) ·
[codec design](../../docs/codec-design.md) · [provenance](../../docs/provenance.md) ·
[contributing](../../CONTRIBUTING.md).

## At a glance

| | |
|---|---|
| Role | Native low-latency video codec (todo.md §32) |
| Codec ID | `THRM` · bitstream `VERSION = 1` |
| Status | Implemented |
| Version | `0.1.0`, lockstep with the workspace |
| MSRV | 1.86 — verified, enforced by CI's `msrv` job |
| License | MIT OR Apache-2.0 |
| Coded formats | `Gray8` and planar `Yuv420`, dimensions multiples of 16 |
| Real-time | **Decoder** real-time-safe; encoder is not (`real_time_encoder: false`) |
| `no_std` | Yes |
| Measured peak memory | 152 503 B against a documented < 1 MiB budget |

## Design (v0.1)

- **I/P coding, no B-frames.** One I-frame, then P-frames forever. Nothing is
  reordered or held back, so a frame encoded at wall-clock *t* is decodable
  at *t*: one-way latency is exactly encode + decode of that frame.
- **Intra refresh.** Each P-frame re-codes a percentage of its macroblocks
  intra on a deterministic rotating cursor that provably sweeps the whole
  picture. Error from a lost packet cannot propagate forever — a receiver
  resynchronises within a bounded number of frames without waiting for a
  full I-frame.
- **Simple prediction.** Four intra modes (DC / vertical / horizontal /
  diagonal) chosen per 8×8 luma block by SAD; full-pixel integer motion
  compensation, single reference frame, edge-clamped. No sub-pixel
  interpolation, no weighted prediction, no multiple references.
- **8×8 DCT-II/III** from `tpt-dsp-core`'s separable `dct_2d`/`idct_2d` —
  transforms live in `tpt-dsp` by the dependency boundary (todo.md §3.1).
- **Real Rice coding**, a run/level stream over each block's zigzag scan,
  with a per-block Rice parameter chosen from `{0,1,2,3}` to minimise coded
  length (signalling `k` beats a fixed `k` on sparse heavy-tailed residuals,
  at 2 bits per coded block).
- **Deblocking** — a 4×4-grid loop filter in the classic H.263 shape runs
  over the whole reconstructed frame, and the *filtered* picture becomes the
  next reference. Encoder and decoder call the identical function on identical
  input, so it cannot cause drift.
- **Skip** — a macroblock whose mean absolute prediction error is below one
  quantiser step is signalled skipped. The threshold is *rate-aware* rather
  than a fixed SAD constant, so skip stays available at coarse `qp` where the
  reconstruction error alone would exceed any fixed bound.
- **Chroma vectors are derived, not signalled** — the luma vector halved,
  the natural 4:2:0 mapping.

## Frame structure

A **macroblock** is 16×16 luma (four 8×8 blocks) plus, for 4:2:0, one 8×8 U

## Usage

```rust
use tpt_video_thrum::{ThrumConfig, ThrumDecoder, ThrumEncoder};
use tpt_media_codec::{Decoder, Encoder};
use tpt_media_core::CodecConfig;

let cfg = CodecConfig::new(*b"THRM", 1);
let tc = ThrumConfig::default(); // qp 26, search_range 8, intra refresh 5%

let mut enc = ThrumEncoder::new(cfg, tc.clone())?;
let packets = enc.encode(&frame_176x144_yuv420)?;

let mut dec = ThrumDecoder::new(cfg, tc)?;
let frames = dec.decode(&packets[0])?;
```

Packet loss: `ThrumDecoder::conceal` repeats the last decoded frame (a
freeze) — the correct concealment for a codec with no temporal hierarchy —
and emits mid-grey before the first frame.

## Wire format and versioning

16-bit bitstream version (rejected up front, todo.md §35) plus a 4-byte
CRC-32 trailer verified before parsing.
`ThrumConfig::max_packet_bits` derives the worst-case packet size from the
level clamp and the per-code bit bound, so the encoder's single allocation
provably cannot overflow.

## Benchmarks & profiling

```bash
cargo bench -p tpt-video-thrum
```

`benches/codec_bench.rs` — I/P encode + decode of 8-frame clips at 176×144
4:2:0 and 640×480 mono (2026-09-28, release profile, criterion):

| bench | median | per-item | real-time factor |
|---|---:|---:|---:|
| `thrum/qcif/encode_i_frame` | 46.8 ms | 5.8 ms/frame | ~57× |
| `thrum/qcif/encode_p_frame` | 92.1 ms | 11.5 ms/frame | ~29× |
| `thrum/qcif/decode` | 15.1 ms | 1.9 ms/packet | ~530× |
| `thrum/vga/encode_i_frame` | 377 ms | 47.1 ms/frame | ~6.8× |
| `thrum/vga/encode_p_frame` | 959 ms | 120 ms/frame | ~2.7× |
| `thrum/vga/decode` | 84.3 ms | 10.5 ms/packet | ~95× |

**Latency** is the P-frame encode + decode sum, with no buffer or reorder
delay: ~13.4 ms one-way at QCIF against a 33.3 ms frame budget at 30 fps;
decode alone ~1.9 ms. The decoder is ~6× cheaper than the encoder — the
asymmetry that matters, since only the decoder is claimed real-time.

P-frame encoding costs ~2× an I-frame because every macroblock pays
`(2·search_range + 1)²` full-pixel SAD evaluations (289 at the default
`search_range = 8`) on top of the intra path's own mode search.
`search_range` is the knob, and the cost is quadratic in it.

Mean packet sizes at the default `qp = 26` on the bench's synthetic clip:
1 074 B/packet at 176×144 4:2:0, 4 357 B at 640×480 mono. These are
*bench-input* rates, not a bitrate target — a real talking head codes
considerably smaller, because skip and intra refresh both key on small
predicted residual. Tables:
[`docs/performance.md`](../../docs/performance.md).

Profiling recipe: `cargo bench -p tpt-video-thrum`, then `cargo flamegraph

## Testing

```bash
cargo test -p tpt-video-thrum
```

45 unit tests (transform/quantiser/entropy round trips, intra-mode
availability, deblocking behaviour, I/P structure, refresh coverage, skip,
concealment, flush, version and CRC rejection, `NeedMoreInput` without a
reference, config validation, codec-private round trip) and 6 property tests
(full-length round trips across four geometries and both formats, `qp`
ordering quality, bit-exact flat frames, determinism, out-of-range header
rejection, and the §38 corruption/truncation/bit-flip sweep).
`tests/realtime_alloc.rs` pins bounded allocation over 24 steady-state passes.

Two real defects were caught by these tests: the decoder walked macroblocks
column-major while the encoder wrote them row-major (≈13 dB PSNR on a smooth
gradient, now ≈45 dB), and the encoder retained its worst-case packet
*capacity* (2.5 MB for a 176×144 packet) instead of the few kilobytes written
— now allocated once at construction and reused, so the steady-state path
does not allocate it at all.

## Known limitations

- 640×480 P-frame encoding alone (~120 ms) exceeds a 33.3 ms budget, so that
  configuration is offline/smaller-frame only until the search window shrinks.
- No B-frames, no sub-pixel motion, no multiple reference frames — all
  deliberate.
- Not reachable from `tpt encode`/`decode`: the CLI's encode family is
  audio-only and rejects `--codec thrum` with an explicit message. Drive the
  crate directly (the CLI's `video`/`y4m` modules are the bridge).

## License

MIT OR Apache-2.0, at your option. See [LICENSE-MIT](../../LICENSE-MIT) and
[LICENSE-APACHE-2.0](../../LICENSE-APACHE-2.0).

--bench codec_bench` (Linux) / `Instruments` (macOS) / `VTune` (Windows).


and one 8×8 V, coded in raster order. **One frame is one packet** —
`PacketFlags::END_OF_FRAME` is set on every packet and no reordering buffer
exists anywhere in the codec.

Anything other than the two coded formats is refused with
`CodecError::Unsupported` rather than silently converted, and dimensions must
be multiples of `MB_SIZE` (16) so the picture is exactly macroblock-aligned:
no padding, no partial macroblocks, no padding-related rate cliff.
