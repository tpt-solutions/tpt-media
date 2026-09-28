# Contributing to tpt-media

Thank you for your interest in `tpt-media`. This project follows a strict
**clean-room, patent-conscious** development model that every contributor
must respect.

## 1. License

By contributing, you agree that your contributions are dual-licensed under
the MIT License and the Apache License, Version 2.0, with copyright held by
**TPT Solutions**. See [`LICENSE-MIT`](LICENSE-MIT) and
[`LICENSE-APACHE-2.0`](LICENSE-APACHE-2.0).

## 2. Clean-Room Rules

`tpt-media` is implemented from academic literature and public-domain
mathematics only. **Contributors must not:**

- Read or refer to the source code of `x264`, `x265`, `rav1e`, `libvpx`,
  `libaom`, `opus`, `AAC` reference implementations, or any other proprietary
  or GPL-encumbered media project while working on `tpt-media`.
- Copy code structure, identifier naming, or unusual implementation
  patterns from those projects.
- Reverse-engineer bitstream layouts from proprietary formats.

**Acceptable sources:**

- Academic papers (IEEE, ACM, pre-prints).
- Standard textbooks (Oppenheim & Schafer for DSP, Sayood for Data
  Compression, Bosi & Goldberg for audio coding).
- Public standards where legally appropriate (IETF RFCs, ISO/IEC
  standards that are openly licensed).
- Algorithms whose original patents have expired.

If you have previously worked on any of the prohibited projects, you must
disclose this in the `PROVENANCE.md` record for any algorithm you touch.

## 3. Patent-Conscious Design

`tpt-media` does **not** claim absolute patent-free status — patent
enforceability is jurisdictional. We instead target **patent-conscious
design** by avoiding techniques that we know are encumbered:

- **No CABAC / Context-Adaptive Coding.** We use static Range coding or
  Huffman only.
- **No complex fractional-pixel interpolation.** We use full-pixel motion
  estimation, or simple bilinear half-pixel (2-tap). We do not use 6-tap
  or 8-tap half/quarter-pixel filters.
- **No Spectral Band Replication (SBR).** We do not synthesise high
  frequencies from low ones.
- **No parametric stereo.** Stereo is encoded as separate channels.
- **No intra prediction modes known to be encumbered** (we implement only
  modes we can derive from textbooks or expired-prior-art).

The legal status of every algorithm is recorded in the per-crate
provenance documents (see `docs/provenance.md`).

## 4. Code Style

- Rust 2021 edition, MSRV **1.75**.
- `cargo fmt` must produce no diffs.
- `cargo clippy -- -D warnings` must pass.
- All public APIs must have rustdoc comments documenting:
  - panics
  - error conditions
  - safety (for `unsafe` blocks)
  - algorithmic complexity
  - real-time / allocation guarantees
- No global mutable state.
- No `unwrap()`/`expect()` in library code; only in tests and examples.
- `unsafe` is permitted only inside documented `// SAFETY:` blocks and only
  in two places: SIMD kernels (delegated to `tpt-dsp`) and the FFI bridge
  (`ffi/tpt-media-sys`).

## 5. Real-Time Paths

Code on the documented real-time path must:

- Not allocate (no `Vec::new`, no `Box::new`, no `String`).
- Not block on I/O or locks.
- Have bounded execution time.
- Use caller-owned buffers.

Mark real-time-safe APIs with `// SAFETY: real-time-safe: no allocation,
bounded work` in their docstring and ensure they appear in the
"Real-time profile" section of `docs/performance.md`.

## 6. Testing

- Every public primitive must have a unit test.
- Every codec must have a round-trip test (encode → decode → compare).
- Every container parser must have a fuzz target.
- Every numerical transform must have a round-trip and a known-answer test.
- Performance changes must include a benchmark before/after.

## 7. Commit Messages

We use [Conventional Commits](https://www.conventionalcommits.org/):

```text
feat(cinder): add packet loss concealment
fix(container): reject truncated CLUS chunks
docs(readme): clarify licensing
test(aura): add bit-perfect lossless round-trip
bench(mdct): add 2048-point benchmark
```

## 8. Provenance

Every non-trivial algorithm ships with a provenance record:

| Field           | Description                                    |
| --------------- | ---------------------------------------------- |
| algorithm       | Human name of the algorithm                     |
| reference       | Source paper / textbook / expired patent #    |
| source          | GREEN / YELLOW / RED classification            |
| derivation      | Where in this repo the implementation lives    |
| implementation  | Link to the source file                        |
| license         | License of the reference work                  |
| patent review   | Date + reviewer initials                       |
| status          | reviewed / under-review / blocked              |

See `docs/provenance.md` and the per-crate `PROVENANCE.md` files.

## 9. Disclosure

If you are unsure whether your planned contribution complies with these
rules, open an issue with the `provenance/question` label before writing
any code.