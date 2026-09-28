# Contributing to tpt-media

`tpt-media` is developed by TPT Solutions. **We do not accept external
code contributions — no pull requests.** Development is issues only:

- **Bug reports** — include the crate, the version/commit, a minimal
  reproduction, and observed vs. expected behavior.
- **Security vulnerabilities** — do *not* open a public issue; follow
  the private reporting process in [`SECURITY.md`](SECURITY.md).
- **Provenance questions** — anything about algorithm sources or
  patent classification, with the `provenance/question` label (§12).
- **Feature or design proposals** — proposals citing sources (see §2)
  are actionable; proposals pointing at an existing implementation to
  imitate are not (§2 forbids exactly that).

The rest of this document is the project's development policy: the
rules the maintainers hold all `tpt-media` code to, published so the
reasoning behind accepted changes is auditable and so issue reporters
know what the project will and will not do. It follows a strict
**clean-room, patent-conscious** development model without exception.

## 1. License

The code is dual-licensed under the MIT License and the Apache License,
Version 2.0, with copyright held by **TPT Solutions**. See
[`LICENSE-MIT`](LICENSE-MIT) and [`LICENSE-APACHE-2.0`](LICENSE-APACHE-2.0).
Since external code contributions are not accepted, there is no CLA or
contributor license agreement; text from issues may be used freely in
fixes and documentation.

## 2. Clean-Room Rules

`tpt-media` is implemented from academic literature and public-domain
mathematics only. **Whoever writes its code — maintainers included —
must not:**

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
disclose this in the provenance record (`docs/provenance.md`) for any
algorithm you touch.

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

- Rust 2021 edition, MSRV **1.86** (verified — see §9.3).
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

The bar above is the rule for *new* real-time APIs. Where a current
implementation falls short of it, the honest claim — and the only one
the docs make — is *bounded* allocation pinned by the §42
allocation/memory tests (`docs/performance.md`, "Memory Budgets"), not
a silent zero-allocation claim.

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

See `docs/provenance.md`. Per-crate records live in that single
document (there are deliberately no per-crate `PROVENANCE.md` files).

## 9. Versioning and API Stability

### 9.1 Release and versioning strategy

- **Lockstep workspace versions.** Every crate ships the version from
  `[workspace.package]` in the root `Cargo.toml` (currently `0.1.0`).
  There are no per-crate version trains: `tpt-media` is one release
  unit, and a change touching five crates ships as one version bump.
  When crates are first published externally they start from the
  workspace version rather than being renumbered.
- **SemVer.** Versions follow [Semantic Versioning](https://semver.org/)
  (see `CHANGELOG.md`). While the leading component is `0`, the *minor*
  number is the breaking-change counter: breaking changes bump `0.y`
  and reset `z`; additive changes and fixes bump `z`. The jump to `1.0`
  is the public stability commitment, not a marketing step.
- **Wire formats are versioned separately from crates.** Every codec
  packet leads with a 16-bit bitstream version and the `.tpt` container
  has its own header version. A wire-format change is therefore both a
  bitstream version bump and a breaking crate change (the `0.y` bump
  above); the codec-level policy is fail-fast — no compatibility mode,
  no guessing (todo.md §35, `docs/codec-design.md` "Bitstream
  Versioning").
- **Record as you land.** Every change gets its `CHANGELOG.md`
  `[Unreleased]` entry when it lands, not retroactively at release
  time.

### 9.2 API stability expectations

- **Before 1.0, no stability guarantee.** Public items may change or
  disappear in any `0.y` release. External consumers should pin exact
  versions; today the crates are workspace-internal path dependencies,
  which is the real stability boundary.
- **Two tiers of public surface.** The foundation, codec, container,
  and pipeline crates are the intended public API of the ecosystem and
  are held to the documentation contracts in §4 (panics, errors,
  complexity, real-time/allocation behavior on every public item). The
  research crates (`tpt-media-representation`, `tpt-media-research`,
  todo.md §56) are explicitly *experimental*: their APIs may churn
  without a changelog entry and they must not be depended on outside
  the workspace.
- **Deprecation, not removal.** A public item that must go away is
  annotated `#[deprecated]` with a migration note and a `CHANGELOG.md`
  entry, and is removed no earlier than the next breaking release.

### 9.3 MSRV

- The workspace MSRV is the `rust-version` field in
  `[workspace.package]` (currently **1.86**), and it is *verified*, not
  aspirational: CI runs `cargo check --workspace --locked
  --all-targets` on exactly that toolchain (the `msrv` job).
- The declared value must be the oldest toolchain that actually builds
  the current dependency tree with the committed `Cargo.lock`. The
  original declaration (1.75) predates any verification and was
  falsified the first time it was tested — `clap_builder` 4.6 in the
  lockfile has an edition-2024 manifest no 1.75 Cargo can parse. Raise
  `rust-version` only when a dependency forces it (prefer pinning a
  dependency lower), record the bump in `CHANGELOG.md`, and update the
  `msrv` job in the same change so the three places can never drift.

## 10. Feature Flags

One convention, established by the foundation crates and enforced by
the `no_std` CI job:

- **`std` is the only default feature**, and it exists in every crate
  that *can* be `no_std`. The crate compiles as `no_std + alloc` with
  `default-features = false`; `std` re-enables error `source` impls,
  file/stdin/stdout bridges, and similar. Workspace members opt in
  explicitly (`features = ["std"]`), which is what lets Reed build the
  same foundation crates bare-metal for `thumbv7em-none-eabi`.
- **Features are additive only.** Enabling a feature may never break a
  build that compiled without it, remove API, or change wire behavior.
- **Capability features gate *modes*, not correctness.** Aura's
  `lossless`/`lossy` select which encode paths exist; neither is a
  silently-degraded version of the other.
- **New crates must be `no_std`-compatible unless they are apps
  (`apps/`) or I/O-bound by nature.** Today eight crates carry
  `#![cfg_attr(not(feature = "std"), no_std)]`:
  `tpt-media-core`, `-bitstream`, `-codec`, `-pipeline`, `-io`,
  `-probe`, Reed, and Thrum — Reed being the one CI-verified
  bare-metal build. `tpt-container`, Cinder, and Aura carry the
  `std`-feature scaffolding but not the attribute, so they are
  std-only today (converting them is follow-up work, not a done
  fact). `tpt-cli` is a std binary by design (§43).

## 11. Benchmarks

- **Each crate owns its benches.** A crate's `benches/` directory is
  the only place its hot paths are measured, and the criterion
  configuration (`sample_size(20)`, 300 ms warm-up, 2 s measurement) is
  fixed *in code* so plain `cargo bench` reproduces it — see
  `docs/performance.md`, "Reproducibility".
- **`docs/performance.md` is the canonical record.** A new benchmark
  lands together with its numbers in that document; a benchmark whose
  numbers were never recorded does not count as the §41 deliverable.
- **Benchmarks do not run in CI.** Shared CI hardware is too noisy for
  the numbers to mean anything. They run (a) whenever a change touches
  a measured hot path — the `CHANGELOG.md` entry states before/after
  figures — and (b) before each release, when the tables in
  `docs/performance.md` are refreshed.
- **A failed budget is a failed build.** If a change breaks a
  documented budget (a frame-budget fraction over 1, a memory budget
  exceeded), it does not land by arguing the benchmark is noisy; it
  lands by fixing the regression or by updating the documented contract
  explicitly, in the same change.

## 12. Disclosure

If you are unsure whether an algorithm the project uses (or an issue
proposes) complies with these rules, open an issue with the
`provenance/question` label — for maintainers, before writing any code
for it. We prefer to spend time on classification rather than later
rewrites.