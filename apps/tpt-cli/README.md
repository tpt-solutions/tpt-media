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
| Scope | Audio via the WAV bridge **and** video via the y4m bridge |

## Commands

```text
tpt probe [--json] <input>          introspect a media file (§44)
tpt inspect <input>                 probe with human detail
tpt encode --codec <name> [..] <in> [<out>]
tpt decode --codec <name> [..] <in> [<out>]
tpt transcode --codec <name> [..] <in> [<out>]
tpt benchmark --codec <name> [..] <in>
tpt video-corpus [--json] [--fps <n>] [--frames <n>]
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
| `--codec <name>` | `cinder`, `aura`, `reed`, `thrum`, `vault`, `mote`, or a raw 4-byte codec ID. Unknown codec = usage error (exit 2) |
| `--container <tpt\|wav>` | Output container. `encode`/`transcode` accept only `tpt`; `decode` writes `wav` for an audio track and y4m (the default) for a video track |
| `--quality 0..=100` | Maps onto the codec's coefficient bit depth (Cinder/Aura 4–12 bits, Reed LSF bit depth). Out of range = usage error |
| `--bitrate <n>[k\|m]` | Parsed and validated (`k` = ×1e3, `m` = ×1e6). **No effect yet** — no codec here has rate control |
| `--set-meta key=value` | Repeatable; published in order, non-empty keys validated |
| `--runs <n>` | Measured passes for `benchmark` (default 5) |
| `--json` | Deterministic machine-readable output for `probe`, `benchmark` and `video-corpus` |
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

## Audio and video

Audio rides the WAV bridge: a 16-bit mono WAV at 44.1/48 kHz in, `cinder` /
`aura` / `reed` into a `.tpt`, and back out as WAV.

Video rides the YUV4MPEG2 (`.y4m`) bridge, which is byte-exact and
deliberately narrow — progressive only, `C420*` or `Cmono`, no aspect-ratio
round trip:

```bash
tpt encode   --codec thrum --quality 60 in.y4m out.tpt   # one packet per frame
tpt decode   --codec thrum out.tpt back.y4m
tpt benchmark --codec thrum --json in.y4m
```

All three video codecs — `thrum`, `vault` and `mote` — are driven by the
y4m bridge for `encode`, `decode`, `transcode` and `benchmark`, and each
declares its own geometry limits (a frame the codec cannot code is refused
by the codec itself, with that codec named in the message, rather than by a
rule duplicated in the CLI). `list-codecs` reports all of them.

`transcode` moves between video codecs, naming both ends of the conversion
(`vault -> mote`).

`tpt video-corpus` measures a video codec (`--codec`, Thrum by default) over
the §36 synthetic video corpus — PSNR, SSIM, bitrate, encode/decode CPU and
frame latency per scene — without reading any files:

```bash
tpt video-corpus --codec vault --frames 8
tpt video-corpus --codec mote --json
```

The frame count it reports is decoded *pictures*, not packets, so Mote's
out-of-band dictionary packet does not understate its rate.

A video codec on a *non*-video path — `encode --codec thrum` with a WAV, or a
`.tpt` whose track is audio — fails with an explicit explanation rather than a
misleading unknown-codec error.

## What `probe` reports about codecs

`probe`/`inspect` resolve each track's codec against a registry holding the
four native codecs, so the capability line reflects *this* binary:

```text
Codec THRM: native=true, decoder=true, encoder=true, lossless=false, lossy=true, rt=true
```

The flags come from each codec's own declared capabilities, not from a
hard-coded table. (`tpt_media_probe::probe`, the library entry point, answers
from an empty registry because a library cannot know what its caller linked in;
`tpt` uses `probe_with_registry` instead.)

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

- Video I/O goes through the `tpt-cli` y4m bridge, not `tpt-media-io`:
  progressive only, `C420*`/`Cmono` only, and aspect ratio is parsed and
  discarded rather than round-tripped (the `.tpt` track model has no aspect
  field).
- `--bitrate` is accepted and validated but has no effect.
- `transcode` requires a `.tpt` input and a 44.1/48 kHz source track.
- The JSON writer is hand-rolled (no serde) — which is how three
  malformed-output defects shipped and were then pinned by exact-shape
  regression tests.

## License

MIT OR Apache-2.0, at your option. See [LICENSE-MIT](../../LICENSE-MIT) and
[LICENSE-APACHE-2.0](../../LICENSE-APACHE-2.0).
