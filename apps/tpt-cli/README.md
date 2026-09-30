# tpt-cli

TPT media command-line interface (probe, encode, decode, transcode, inspect, benchmark).

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg)](../../LICENSE-MIT)
[![Rust](https://img.shields.io/badge/rust-1.86%2B-orange.svg)](https://www.rust-lang.org)

The `tpt` binary: the workspace's user-facing surface. It reads and writes
WAV, muxes and demuxes `.tpt`, and drives the native codecs end to end.
Part of the [`tpt-media`](../../README.md) workspace —
[workspace changelog](../../CHANGELOG.md) · [changelog](CHANGELOG.md) ·
[CLI design](../../docs/cli-design.md) · [contributing](../../CONTRIBUTING.md).

## At a glance

| | |
|---|---|
| Role | Command-line application (todo.md §43) |
| Binary name | `tpt` |
| Status | All commands implemented |
| Version | `0.1.0`, lockstep with the workspace |
| MSRV | 1.86 — verified, enforced by CI's `msrv` job |
| License | MIT OR Apache-2.0 |
| Scope | **The `encode`/`decode`/`transcode`/`benchmark` family is audio-only** |

## Commands

```text
tpt probe [--json] <input>          introspect a media file (§44)
tpt inspect <input>                 probe with human detail
tpt encode --codec <name> [..] <in> [<out>]
tpt decode --codec <name> [..] <in> [<out>]
tpt transcode --codec <name> [..] <in> [<out>]
tpt benchmark --codec <name> [..] <in>
tpt list-codecs
tpt --help
```

## Build and run

```bash
cargo build -p tpt-cli
cargo run -p tpt-cli -- list-codecs

cargo run -p tpt-cli -- encode --codec aura --quality 80 in.wav out.tpt
cargo run -p tpt-cli -- probe --json out.tpt
cargo run -p tpt-cli -- decode --codec aura out.tpt back.wav
cargo run -p tpt-cli -- transcode --codec cinder out.tpt small.tpt
cargo run -p tpt-cli -- benchmark --codec aura --json in.wav
```

## Options

| Option | Behaviour |
|---|---|
| `--codec <name>` | `cinder`, `aura`, `reed`, `thrum`, or a raw 4-byte codec ID. Unknown codec = usage error (exit 2) |
| `--container <tpt\|wav>` | Output container; `encode` accepts only `tpt`, `decode` only `wav` |
| `--quality 0..=100` | Maps onto the codec's coefficient bit depth (Cinder/Aura 4–12 bits, Reed LSF bit depth). Out of range = usage error |
| `--bitrate <n>[k\|m]` | Parsed and validated (`k` = ×1e3, `m` = ×1e6). **No effect yet** — no codec here has rate control |
| `--set-meta key=value` | Repeatable; published in order, non-empty keys validated |
| `--runs <n>` | Measured passes for `benchmark` (default 5) |
| `--json` | Deterministic machine-readable output for `probe` and `benchmark` |
| `-` as path | Read from stdin / write to stdout |

## Exit codes

| Code | Meaning |
|---|---|
| 0 | success |
| 1 | runtime / I/O error (missing input, unreadable or unsupported media, codec mismatch, write failure) |
| 2 | usage error (unknown command or option, bad value, missing required) |

There is deliberately no exit code 3; an earlier draft reserved it for
"codec not wired yet", and with every listed codec implemented, no code path
produces it.

## Video scope

`--codec thrum` resolves and `tpt list-codecs` advertises it, but the
encode-family commands are audio-only and reject it with an explicit "this is
a video codec and these commands are audio-only" message rather than a
misleading unknown-codec error. Driving it means using
[`tpt-video-thrum`](../../codecs/thrum) directly.

## Testing

```bash
cargo test -p tpt-cli
```

Every command executes through `tpt_cli::run(argv, &Ctx)`, where `Ctx` injects
`read_file` / `read_stdin` / `write_file` closures and binary stdout is
returned as `Outcome.binary_stdout`. Tests supply in-memory fakes and assert
exit code plus captured streams — including full Aura encode→decode round
trips — with no filesystem and no stdin. Real I/O is confined to
`src/main.rs`.

Also here: the §36 synthetic corpora (`signals.rs` audio, `video.rs` video —
closed-form SplitMix64 generation, no binary fixtures), the §37 quality
metrics (`quality.rs`: SNR, log-spectral distance, PSNR, SSIM, bitrate,
bits-per-pixel), and the §42 memory harness (`memprobe.rs`, the tracking
allocator behind `tests/memory_budget.rs`).

## Known limitations

- `encode`/`decode`/`transcode`/`benchmark` handle audio only (WAV in,
  `.tpt`/WAV out).
- `--bitrate` is accepted and validated but has no effect.
- `transcode` requires a `.tpt` input and a 44.1/48 kHz source track.
- The JSON writer is hand-rolled (no serde) — which is how three
  malformed-output defects shipped and were then pinned by exact-shape
  regression tests.

## License

MIT OR Apache-2.0, at your option. See [LICENSE-MIT](../../LICENSE-MIT) and
[LICENSE-APACHE-2.0](../../LICENSE-APACHE-2.0).
