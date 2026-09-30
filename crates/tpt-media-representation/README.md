# tpt-media-representation

Swappable media representation contracts (analysis/synthesis, quantisation, rate/distortion, sparsity) for the tpt-media representation-research platform.

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg)](../../LICENSE-MIT)
[![Rust](https://img.shields.io/badge/rust-1.86%2B-orange.svg)](https://www.rust-lang.org)

Formalises `z = T(x)` / `x̂ = T⁻¹(z)` minimising `J = D(x, x̂) + λ·R(z)`,
where `T` is **never assumed** to be a DCT, a wavelet, a block transform, or
motion-compensated residuals. Part of the
[`tpt-media`](../../README.md) workspace —
[workspace changelog](../../CHANGELOG.md) · [changelog](CHANGELOG.md) ·
[contributing](../../CONTRIBUTING.md).

> **Experimental.** Per CONTRIBUTING §9.2 this crate's API may churn without
> a changelog entry and must not be depended on outside the workspace.

## At a glance

| | |
|---|---|
| Role | Representation *contracts* (todo.md §56) |
| Status | Implemented, experimental |
| Version | `0.1.0`, lockstep with the workspace |
| MSRV | 1.86 — verified, enforced by CI's `msrv` job |
| License | MIT OR Apache-2.0 |
| `no_std` | No — this is `std`-only analysis code, deliberately |
| Profile | **Offline only.** Nothing here is held to the real-time/`no_std` contract (todo.md §14) |
| Unsafe | `#![forbid(unsafe_code)]` |

## What it provides

| Item | Purpose |
|---|---|
| `Representation` | The analysis/synthesis contract: `analyse` / `synthesise` over a `SignalView`/`SignalViewMut` |
| `Quantizer` | The quantisation contract: `quantise` / `dequantise` |
| `RateEstimator` | Empirical rate of a coefficient block |
| `DistortionMetric`, `Mse`, `Rmse`, `psnr_db` | Distortion metrics |
| `SignalView`, `SignalViewMut`, `SignalBuffer`, `SignalDims` | The spatiotemporal signal type |
| `SparsityReport`, `CoeffMagnitudes` | Sparsity measurement |

Deliberately small: contracts and the signal type, nothing else. Concrete
representations, synthetic signals and the harness live in
[`tpt-media-research`](../tpt-media-research).

## Usage

```rust
use tpt_media_representation::{
    psnr_db, DistortionMetric, Quantizer, RateEstimator, Representation, SignalBuffer, SignalDims,
};

// z = T(x)
let coeffs = representation.analyse(&signal)?;
// rate and distortion of the coded form
let rate = rate_estimator.estimate(&coeffs);
let x_hat = representation.synthesise(&coeffs)?;
let d = Mse.compute(&signal, &x_hat);
```

The point of the abstraction is that `T`, the quantizer and the metric can each
be swapped without rewriting the comparison harness.

## Feature flags

None. This crate is not `no_std` and has no optional features; it is offline
research code, where dynamic allocation, larger scratch and expensive search
are all acceptable.

## Design notes

`docs/representation-research.md` carries the methodology, the oracle
experiment, and the compatibility story with the planned Vault/Mote codecs
(todo.md §33–34). The research platform's purpose is to test whether
conventional block-transform coding is actually the right choice for these
signals before Vault and Mote are designed around it.

## Testing

```bash
cargo test -p tpt-media-representation
```

Contract-level tests for the metrics and views; the substantive experiments
run through `tpt-media-research`.

## Known limitations

- Experimental: no API-stability guarantee, no external consumers.
- Traits are object-safe only where the signatures allow it; the research
  harness uses generics for the hot paths.

## License

MIT OR Apache-2.0, at your option. See [LICENSE-MIT](../../LICENSE-MIT) and
[LICENSE-APACHE-2.0](../../LICENSE-APACHE-2.0).
