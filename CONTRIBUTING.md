# Contributing to tpt-media

Thank you for your interest in tpt-media! This project deliberately trades raw
performance for **legal bulletproofness**. Every contributor must follow the
rules below. PRs that violate them will be rejected without prejudice.

## 1. Clean-Room Rule (mandatory)

Contributors **must not** read the source code of any of the following
projects, in whole or in part, while working on tpt-media:

- `x264`, `x265`, `libvpx`, `rav1e`, `dav1d`
- `Opus`, `Vorbis`, `Speex`
- `AAC` reference implementations (e.g. the FAAD / libfdk-aac source trees)
- `FFmpeg` / `Libav` source code (their high-level API and patent status is
  entangled with encoders/decoders we are avoiding)
- Any other encoder/decoder that may be covered by software-patent claims

If you have previously read any of the above, you may still contribute
**mathematics and architectural design** (drawn from textbooks and academic
papers) but you must not translate the patent-encumbered code into Rust.

**Permitted sources:**

- Academic papers (IEEE, EURASIP, JAES, AES convention preprints)
- Textbooks (Oppenheim & Schafer for DSP, Sayood for data compression,
  Rabiner & Schafer for speech processing)
- Public-domain mathematics (the relevant US patents on range coding,
  Huffman, DCT, MDCT, LPC have expired)
- Standards documents published as freely-downloadable PDFs (e.g. ITU-T G.711,
  the IETF Opus spec — but **not** the reference C code)

If in doubt, ask on the issue tracker before writing code.

## 2. Patent-Free Math

The following techniques are **prohibited**:

- **CABAC** or any context-adaptive binary arithmetic coding. Use static
  Range coding or Huffman instead.
- **Fractional pixel interpolation with 6-tap or 8-tap FIR filters.** Use
  full-pixel motion estimation, or a simple 2-tap bilinear half-pixel
  filter (as in `tpt-video-vault`).
- **Spectral Band Replication (SBR)** or any other parametric high-frequency
  regeneration.
- **Parametric stereo** or any other fake-stereo technique.

## 3. Code Style

- Rust edition 2021, MSRV 1.75.
- `#![forbid(unsafe_code)]` in every codec crate by default; `unsafe` is
  permitted only inside `tpt-core`'s SIMD/FFI surfaces and inside
  `tpt-media-sys`.
- Hot paths must be zero-allocation.
- Run `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and
  `cargo test --workspace` before submitting a PR.

## 4. Tests

Every crate must ship unit tests for its public surface. Codec round-trip
tests (`encode -> decode -> compare`) are mandatory for each tier. Lossless
modes must additionally pass a **bit-perfect** test (`encode -> decode` byte
equality on deterministic input).

## 5. Commit / DCO

Sign your commits (`git commit -s`). By signing you certify the
Developer Certificate of Origin (Version 1.1):

> I certify that, to the best of my knowledge, the contribution was created
> in whole or in part by me and I have the right to submit it under the
> project's open-source license. I understand and agree that this project
> and my contribution are public and that a record of the contribution (and
> any related metadata) is maintained indefinitely.

## 6. Reporting Concerns

If you believe a contribution may inadvertently infringe a third-party
patent, **stop work and open a private issue** describing the concern.
Do not publish the PR until the maintainers have reviewed it.