# tpt-video-vault

Vault: TPT high-compression video codec (64x64 superblocks, recursive partitioning, half-pel motion, multi-reference, RDO).

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg)](../../LICENSE-MIT)
[![Rust](https://img.shields.io/badge/rust-1.86%2B-orange.svg)](https://www.rust-lang.org)

The compression-efficiency end of the native video family: where
[Thrum](../thrum/README.md) trades coding gain for latency, Vault spends
encoder work to save bits. Part of the [`tpt-media`](../../README.md)
workspace — [workspace changelog](../../CHANGELOG.md) ·
[changelog](CHANGELOG.md) · [codec design](../../docs/codec-design.md) ·
[provenance](../../docs/provenance.md) ·
[contributing](../../CONTRIBUTING.md).

## At a glance

| | |
|---|---|
| Role | Native high-compression video codec (todo.md §33) |
| Codec ID | `VAUL` · bitstream `VERSION = 1` |
| Status | Implemented (v0.1); not yet wired into the CLI or the web player |
| Version | `0.1.0`, lockstep with the workspace |
| MSRV | 1.86 |
| License | MIT OR Apache-2.0 |
| Coded formats | `Gray8` and planar `Yuv420`, dimensions non-zero multiples of 8 |
| Real-time | **Neither side** is claimed real-time (`Capabilities` is lossy only) |
| `no_std` | The crate is `#![cfg_attr(not(feature = "std"), no_std)]` + `alloc`, like Thrum; bare-metal builds are **not** CI-verified for it |
| Measured peak memory | 482 784 B at 176×144 4:2:0 with 4 references, encoder + decoder together ([`docs/performance.md`](../../docs/performance.md)) |

## Design (v0.1)

- **64×64 superblocks, recursively partitioned** down to a minimum of 8×8.
  A node is one *leaf* (one prediction + one residual) or four children.
  A node that overhangs the picture edge splits implicitly, so any size that
  is a multiple of 8 is coded with no padding. The recursion is structurally
  three levels deep.
- **Half-pel motion vectors.** Full-pel pattern search, then a half-pel
  refinement over the eight neighbouring positions. Interpolation is the
  simplest one there is (a 2-tap average; a 4-sample average on the diagonal)
  because CONTRIBUTING §3 rules out longer filters. Vectors are coded as a
  difference from a left/top neighbour predictor; chroma vectors are derived.
- **Up to 16 reference frames** (`max_refs`, default 4): the last N
  loop-filtered reconstructions, most recent first; each inter leaf names its
  reference. No B-frames, no bi-prediction.
- **Four intra modes** at every block size (DC, vertical, horizontal, and a
  plane-extrapolation mode); chroma shares the luma mode.
- **2D DCT 4×4 to 32×32** from `tpt-dsp-core` (`dct_2d`/`idct_2d`); a 64×64
  leaf is four 32×32 tiles. Quantisation is in orthonormal units so one `qp`
  means the same step at every transform size.
- **RDO with an explicit objective** `J = D + λ·R`: `D` is the squared error
  against the reconstruction the decoder will produce, `R` is the exact bit
  count of the real writer, `λ = LAMBDA_SCALE · step²`. Partition, prediction
  mode, reference and vector are chosen this way. **This is a greedy per-node
  comparison over a heuristically pruned candidate set — it does not find,
  and is not claimed to find, the global minimum of `J`.**
- **Static Rice entropy coding** (`tpt-media-bitstream`), a run/level scheme
  over a diagonal scan, per-block parameter. No context-adaptive coding.
- **Loop filter**: the H.263 Annex J deblocking filter, driven by a
  boundary strength and applied only at real coding-structure edges.

## Usage

```rust
use tpt_media_codec::{CodecConfig, Decoder, Encoder};
use tpt_video_vault::{VaultConfig, VaultDecoder, VaultEncoder};

let vc = VaultConfig {
    width: 176,
    height: 144,
    ..VaultConfig::default() // Yuv420, qp 30, 4 references, 64x64 superblocks
};
let mut enc = VaultEncoder::new(CodecConfig::default(), vc.clone())?;
let mut dec = VaultDecoder::new(CodecConfig::default(), vc)?;

let packet = enc.encode(&frame_176x144_yuv420)?.remove(0);
let out = dec.decode(&packet)?.remove(0);
```

`max_refs` above 8 needs `ResourceLimits::permissive()` (or a raised
`max_reference_frames`) on the decoder: the default limits are strict and
allow 8. `VaultConfig::max_block_size` (8/16/32/64) is encoder-only.
`VaultDecoder::last_stats()` reports which tools the last frame actually used
(leaf sizes, skip/inter/intra, half-pel, reference histogram).

## Wire format and versioning

`16-bit version | 1 intra | 6 qp | 5 num_refs | 1 loop_filter`, then the
superblocks, then a 4-byte CRC-32 trailer verified before any other bit is
parsed; an unsupported version is rejected up front (todo.md §35). The leaf
syntax is in [`docs/codec-design.md`](../../docs/codec-design.md).
`VaultConfig::max_packet_bits` *derives* the worst-case packet size from the
level clamp and the per-code bit bound; the encoder checks every finished
packet against it. A test drives the finest quantiser over uniform noise and
asserts the bound holds.

## Hostile input

The decoder never indexes by a wire value: the CRC and version are checked
first; `qp`, `num_refs`, reference indices and motion vectors are
range-checked; an off-picture vector is edge-clamped, not indexed;
coefficient walks are bounded by the tile size; levels are clamped; and a
failed decode leaves the reference store untouched. Resource limits (§19)
are honoured for packet size, frame size, allocation, reference frames and
recursion depth. The corruption sweep in `tests/property.rs` repairs the CRC
after corrupting the body — random bytes are stopped by the CRC and would
never reach the parser.

## Benchmarks & profiling

```bash
cargo bench -p tpt-video-vault
```

`benches/codec_bench.rs`, 4-frame clips of a gradient with a panning textured
block (2026-09-30, release profile, criterion, 10 samples). The shared
development machine is noisy: the same bench measured about twice as slow
while other builds were running (285 ms vs 180 ms for QCIF intra), so read
the timings as machine-relative; see
[`docs/performance.md`](../../docs/performance.md) for the full table and
caveats.

| bench | median | per frame |
|---|---:|---:|
| `vault/qcif/encode_i_frame` (4 intra frames) | 180 ms | 45 ms |
| `vault/qcif/encode_p_frame` (1 I + 3 P) | 218 ms | 54 ms |
| `vault/qcif/decode` | 7.4 ms | 1.9 ms |
| `vault/cif/encode_i_frame` | 690 ms | 172 ms |
| `vault/cif/encode_p_frame` | 712 ms | 178 ms |
| `vault/cif/decode` | 25.2 ms | 6.3 ms |

Rate and quality on the same bench clip: 202 B/packet at 37.1 dB luma PSNR
(QCIF) and 498 B/packet at 37.9 dB (CIF) at the default `qp = 30`. These are
synthetic-content figures, **not** a compression claim against any other
codec: the content is smooth and highly predictable.

## Testing

```bash
cargo test -p tpt-video-vault
```

38 tests: 15 unit tests (Rice lengths against the real writer, coefficient
block round trips and the worst-case bound at all four transform sizes,
transform round trip and a DC known answer, half-pel interpolation known
answers and off-picture safety, intra availability and a plane-extrapolation
exactness check, loop-filter behaviour, config validation), 18 property tests
(decoder output bit-identical to the encoder's own reconstruction across
sizes that are not multiples of 64, both formats and 1–16 references;
determinism; `qp` ordering; flat frames exact; static scenes and pans cheap;
the RDO objective not worse than a forced 8×8 partition; the packet bound;
hostile headers; resource limits; the corruption sweep; factories; keyframe
reset), 4 coding-tool census tests (large blocks where flat and small where
busy, half-pel vectors chosen on a half-pel shift, older references used and
paying off on an A/B/A/B cut, near-total skip on a repeated frame) and 1
memory test (peak independent of stream length).

## Known limitations

- The encoder is slow (about 45-55 ms/frame at QCIF, 170-180 ms at CIF in release, on a quiet machine)
  and not real-time. The direct `O(n³)` separable DCT from
  `tpt-dsp-core` evaluated at every partition level is the likely dominant cost (not profiled; the time grows with the largest allowed block, see `docs/performance.md`).
- No B-frames, no weighted prediction, no long-term references or reference
  reordering (the references are simply the last N pictures), no rate control,
  no scene-cut detection, no per-block quantiser adaptation, no
  coefficient-level RDO (RDOQ).
- The loop filter is switched by a per-frame flag that the encoder does **not**
  decide by rate–distortion. Measured: it helped a textured clip (up to
  +0.44 dB) and was neutral-to-harmful on a smooth-gradient clip (see
  `docs/performance.md`). It is on by default.
- The closed loop has no drift when encoder and decoder run the same build on
  the same target; the transform is `f32` and the workspace makes no
  cross-target bit-exactness claim (todo.md §39).
- Not wired into `tpt-cli`, the `tpt-web` player or the probe capability
  table, and its quality has not been measured over the §36 video corpus.

## License

MIT OR Apache-2.0, at your option. See [LICENSE-MIT](../../LICENSE-MIT) and
[LICENSE-APACHE-2.0](../../LICENSE-APACHE-2.0).
