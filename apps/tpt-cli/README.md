# tpt-media-cli

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
| Version | `0.2.0`, lockstep with the workspace |
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
tpt compare [--json] <reference> <test>   score a decoded file vs its original
tpt list-codecs
tpt completions <bash|zsh|fish|powershell>
tpt doctor
tpt --help
```

## Install

```bash
# from a checkout of this repository
cargo install --path apps/tpt-cli

# or straight from git
cargo install --git https://github.com/TPT-Solutions/tpt-media tpt-media-cli
```

Prebuilt binaries for Linux, macOS and Windows are on the
[releases page](https://github.com/TPT-Solutions/tpt-media/releases). Each
release ships a `SHA256SUMS` file; check a download with
`sha256sum -c SHA256SUMS` (Linux, macOS) or
`Get-FileHash <file> -Algorithm SHA256` (PowerShell).

## Build and run

```bash
cargo build -p tpt-media-cli
cargo run -p tpt-media-cli -- list-codecs

cargo run -p tpt-media-cli -- encode --codec aura --quality 80 in.wav out.tpt
cargo run -p tpt-media-cli -- probe --json out.tpt
cargo run -p tpt-media-cli -- decode --codec aura out.tpt back.wav
cargo run -p tpt-media-cli -- transcode --codec cinder out.tpt small.tpt
cargo run -p tpt-media-cli -- benchmark --codec aura --json in.wav
```

## Options

| Option | Behaviour |
|---|---|
| `--codec <name>` | `cinder`, `aura`, `reed`, `thrum`, `vault`, `mote`, or a raw 4-byte codec ID. Unknown codec = usage error (exit 2) |
| `--container <tpt\|wav>` | Output container. `encode`/`transcode` accept only `tpt`; `decode` writes `wav` for an audio track and y4m (the default) for a video track |
| `--quality 0..=100` | Cinder: band step, 64 at 0 to 1/16 at 100 (log scale; overridden by `--band-step`). Aura: coefficient bit depth. Reed: LSF bit depth. Thrum and Vault: quantiser index. Mote: dictionary size, 64 to 4096 entries. Out of range = usage error |
| `--bitrate <n>[k\|m]` | Target bitrate (`k` = ×1e3, `m` = ×1e6). Video: Vault's frame-level rate control. Audio `encode`, `transcode` and `benchmark`: searches Cinder's quantiser step / Reed's LSF depth for the finest setting at or under the target (overrides `--quality`); Aura is lossless and refuses it. A target below the codec's floor encodes at the floor and says so |
| `--resample <hz>` | Resample audio input to this rate first (windowed-sinc). Encode and benchmark |
| `--target-snr <db>` | Audio (cinder, reed): the coarsest setting whose decoded SNR still meets the target. Overrides `--quality`. Not available for Aura (lossless) |
| `--target-psnr <db>` | Video (`encode`, `transcode`): the coarsest quantiser (Thrum, Vault) or the smallest dictionary (Mote) whose mean PSNR still meets the target. Overrides `--quality` |
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

## Checking a round trip

`tpt compare` scores any decoded file against its original with the same
metrics `benchmark` uses: SNR and log-spectral distance for two WAVs, mean and
worst-frame PSNR plus mean SSIM for two y4m clips. One input may be `-`.

```bash
tpt encode --codec cinder --bitrate 96k in.wav out.tpt
tpt decode out.tpt back.wav
tpt compare in.wav back.wav
```

## Other formats (ffmpeg)

`tpt` reads 8/16/24/32-bit integer and float WAV directly, downmixes stereo
and resamples with `--resample`, so ffmpeg is only needed for formats it
does not read (MP3, FLAC, M4A and so on). Convert to a WAV file:

```bash
ffmpeg -i in.mp3 -c:a pcm_s16le out.wav
tpt encode --codec cinder --target-snr 20 out.wav out.tpt
```

Piping works too, because `tpt` reads the streamed WAV header ffmpeg writes to
a pipe (data size `0xFFFFFFFF`):

```bash
ffmpeg -v error -i in.mp3 -f wav - | tpt encode --codec cinder --target-snr 20 - out.tpt
```

## Self-test and shell completion

`tpt doctor` round-trips the built-in synthetic signals through every native
codec and reports each result. It exits 0 only when all of them pass, so it
is a quick check that a new build or install works on your machine:

```bash
tpt doctor
```

`tpt completions <shell>` prints a completion script for `bash`, `zsh`,
`fish` or `powershell`:

```bash
eval "$(tpt completions bash)"          # bash: add to ~/.bashrc
tpt completions fish | source            # fish
tpt completions powershell | Out-String | Invoke-Expression   # PowerShell
```

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
cargo test -p tpt-media-cli
```

Every command executes through `tpt_media_cli::run(argv, &Ctx)`, where `Ctx` injects
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

- Video I/O goes through the `tpt-media-cli` y4m bridge, not `tpt-media-io`:
  progressive only, `C420*`/`Cmono` only, and aspect ratio is parsed and
  discarded rather than round-tripped (the `.tpt` track model has no aspect
  field).
- `--bitrate`, `--target-snr` and `--target-psnr` are per-file averages or per-file settings, not per-second limits.
- `--quality` on Cinder sets the band step, so it moves the rate; `--bitrate` and `--target-snr` pick the setting for a size or quality instead.
- Container framing costs about 9 bytes per packet in version 0.2 (43 in 0.1). `--bitrate` measures the whole file, including that framing, so on a very short clip a small target can sit below what the framing alone allows.
- `transcode` requires a `.tpt` input and a 44.1/48 kHz source track.
- The JSON writer is hand-rolled (no serde) — which is how three
  malformed-output defects shipped and were then pinned by exact-shape
  regression tests.

## License

MIT OR Apache-2.0, at your option. See [LICENSE-MIT](../../LICENSE-MIT) and
[LICENSE-APACHE-2.0](../../LICENSE-APACHE-2.0).
