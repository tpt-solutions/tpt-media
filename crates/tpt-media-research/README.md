# tpt-media-research

Representation-research harness for `tpt-media`: synthetic signals, rate/distortion + sparsity measurement, and alternative video representations benchmarked against conventional block-transform coding.

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg)](../../LICENSE-MIT)
[![Rust](https://img.shields.io/badge/rust-1.86%2B-orange.svg)](https://www.rust-lang.org)

The measurement side of the representation platform: it produces honest
inputs, measures rate/distortion without bias, and benchmarks alternative
representations against block-transform coding so the next codecs are
designed from data rather than assumption. Part of the
[`tpt-media`](../../README.md) workspace —
[workspace changelog](../../CHANGELOG.md) · [changelog](CHANGELOG.md) ·
[contributing](../../CONTRIBUTING.md).

> **Experimental.** Per CONTRIBUTING §9.2 this crate's API may churn without
> a changelog entry and must not be depended on outside the workspace.

## At a glance

| | |
|---|---|
| Role | Research harness (todo.md §56) |
| Status | Implemented, experimental |
| Version | `0.1.0`, lockstep with the workspace |
| MSRV | 1.86 — verified, enforced by CI's `msrv` job |
| License | MIT OR Apache-2.0 |
| `no_std` | No — offline analysis code, deliberately |
| Unsafe | `#![forbid(unsafe_code)]` |

## What it provides

| Module | Contents |
|---|---|
| `signals` | Synthetic signal generators: `spatial`, `spatiotemporal` |
| `det_rng` | Deterministic seeded PRNG (SplitMix64-style) |
| `entropy` | Empirical entropy estimation |
| `harness` | The matched-rate distortion harness |
| `representations` | `block_dct`, `motion_aligned`, `patch_vq`, `temporal_dct` |
| `kmeans`, `uniform` | Quantizer construction helpers |
| `diagnostics` | Rate/distortion/sparsity reporting |
| `experiments` | The oracle experiment and the methodology phases |

The crate has no representation-*codec* logic of its own: it consumes the
abstract `Representation` contract from
[`tpt-media-representation`](../tpt-media-representation) and generic DSP
primitives from `tpt-dsp-core`.

## Determinism

Every helper is a closed-form or seeded function. There is no `rand`, no
thread-local state and no I/O. Two runs with the same inputs and the same
seed are byte-identical — which is what makes a rate/distortion number
comparable across machines and across commits.

## Usage

```bash
cargo test -p tpt-media-research
cargo bench -p tpt-media-research
```

The experiments are driven by `experiments`; the oracle experiment is the
methodology's reference point for "how good can any representation of this
signal be".

## Design notes

`docs/representation-research.md` is the methodology document, including the
oracle experiment and how this platform stays compatible with the planned
Vault and Mote codecs. The contracts live in
[`tpt-media-representation`](../tpt-media-representation); this crate is the
harness that uses them.

## Benchmarks & profiling

Timing numbers are secondary here — the point is rate/distortion at matched
rate, not throughput. The crate is deliberately offline: allocation and
expensive search are fine, so nothing in it is held to the real-time
contract.

## Testing

```bash
cargo test -p tpt-media-research
```

Determinism is part of the contract, so the seeded generators have explicit
tests asserting identical output for identical seeds, and the harness tests
assert matched-rate comparison behaviour rather than absolute scores.

## Known limitations

- Experimental: no API-stability guarantee, no external consumers.
- Conclusions are only as good as the synthetic corpus; the signals are
  generated, not captured from real content.

## License

MIT OR Apache-2.0, at your option. See [LICENSE-MIT](../../LICENSE-MIT) and
[LICENSE-APACHE-2.0](../../LICENSE-APACHE-2.0).
