# CLI Design (§43)

`tpt-cli` is the TPT media command-line interface: a single `tpt` binary
that probes, encodes, decodes, transcodes, inspects and benchmarks media.
The §43 requirements (todo.md lines 1333–1356) map to code as follows.

## Command surface

```
tpt probe [--json] <input>          introspect a media file (§44)
tpt inspect <input>                 probe with human detail
tpt encode --codec <name> [..] <in> [<out>]    compress
tpt decode --codec <name> [..] <in> [<out>]    decompress
tpt transcode --codec <name> [..] <in> [<out>]
tpt benchmark --codec <name> [..] <in>
tpt video-corpus [--json] [--fps <n>] [--frames <n>]
tpt list-codecs
tpt --help
```

Implementation: `apps/tpt-cli/`. The crate is a normal `std` binary (apps
use std freely) with a `lib.rs` core that is fully unit-testable without a
real filesystem or stdin.

## §43 item → code mapping

| # | §43 requirement | implementation |
|---|---|---|
| 1 | codec selection | `--codec <name>` → `args::resolve_codec` (any native codec name — `cinder\|aura\|reed\|thrum` — or a raw 4-byte codec ID). Unknown codec = usage error (2). `thrum` resolves everywhere, but each command needs the matching *media*: a `.y4m` input for `encode`/`benchmark`, a `.tpt` carrying a video track for `decode`/`transcode`. A video codec on an audio path fails with an explicit "video codec, audio-only" error (1) rather than a misleading unknown-codec. |
| 2 | container selection | `--container tpt\|wav` → `probe::ContainerKind`. Default `tpt`. Invalid = usage error (2). |
| 3 | quality configuration | `--quality 0..=100` → validated `u8`. Out of range = usage error (2). |
| 4 | bitrate configuration | `--bitrate <n>[k\|m]` → `args::parse_bitrate` (`k` = ×1e3, `m` = ×1e6). Overflow / non-numeric = usage error (2). |
| 5 | metadata manipulation | `--set-meta key=value` (repeatable), validated to non-empty key, collected in publish order. |
| 6 | streaming stdin/stdout | `-` as `<input>` (and `<output>`) is read from stdin / written to stdout. Probe reads via injected `read_stdin`. |
| 7 | machine-readable output | `--json` on [`probe`] → deterministic, hand-rolled serializer (`src/json.rs`, no serde). |
| 8 | error reporting | stable exit codes, see below. |

## Exit codes

| code | meaning |
|---|---|
| 0 | success |
| 1 | runtime / I/O error (missing input, unreadable/unsupported media, codec mismatch, write failure) |
| 2 | usage error (unknown command, unknown option, bad value, missing required) |

There is no exit code 3. An earlier draft reserved it for "codec not
wired yet"; with Reed wired, every listed codec is implemented and no
code path produces it.

## Wiring status

- `probe`, `inspect`, `list-codecs`, `--help` are fully implemented and
  tested.
- `encode` is live: reads a 16-bit mono WAV (44.1/48 kHz, matching the
  Cinder/Aura supported-format table), chunks the samples into codec blocks
  (Cinder `2·frame_size` = 512 samples, Aura `block_size` = 1024), zero-pads
  the final block, encodes each block and muxes everything into a `.tpt`
  with a full track descriptor (track timebase `1/sample_rate`, codec
  version from `encoder.info()`, sample rate, channel layout, `--set-meta`
  tags). Parameters that are not self-describing in the codec bitstream
  ride in the track's codec-private `data`: Cinder `bits_per_coef` (the
  `--quality` mapping) as one byte, Aura `block_size` as two little-endian
  bytes. The original sample count is stored in the reserved metadata key
  `tpt.io.sample_count` so `decode` can drop padding. Encodes only the
  `tpt` container (any other `--container` value is an error).
- `decode` is live: sniffs the input, opens the `.tpt`, finds the first
  audio track, decodes every packet with the track's codec (an explicit
  `--codec` must match the track, otherwise exit 1; codec-private `data`
  supplies Cinder `bits_per_coef` and Aura `block_size`), trims block
  padding using `tpt.io.sample_count`, and writes a single 16-bit WAV.
  Decodes only to `wav` output (any other `--container` value is an
  error). `reed` is wired through the same path (`--codec reed`, 8/16
  kHz inputs, `--quality` mapping below).
- `--quality` maps `0..=100` onto the codec coefficient bit depth
  (Cinder/Aura `bits_per_coef`, 4–12 bits; Reed LSF bit depth).
  `--bitrate` drives Vault's frame-level rate control on the video
  path (`target_bits_per_frame` = bitrate / fps); on the audio path it
  remains validated but not applied (no audio codec has rate control).
- `transcode` is live (2026-09-14): decodes the input `.tpt` (first
  audio track, padding trimmed via `tpt.io.sample_count`), re-encodes
  with the `--codec` target through the same encode path as `encode`,
  and muxes a fresh `.tpt`. The source track's metadata tags are
  carried over; a `--set-meta` pair replaces a carried tag of the same
  key, and the sample-count key is refreshed to the decoded count.
  Requires a `.tpt` input (a WAV input suggests `tpt encode`) and a
  44.1/48 kHz source track.
- `benchmark` is live (2026-09-14): reads the same WAV shape `encode`
  accepts, runs one untimed warm-up pass plus `--runs` (default 5)
  measured encode+decode passes, and reports median/min wall time,
  real-time factors, payload bytes / bits-per-sample / bitrate, and the
  §37 quality metrics (SNR + log-spectral distance from
  `tpt_cli::quality`) of the round trip. `--json` emits the same data
  structurally; a bit-exact round trip reports `"bit_exact": true`
  instead of an unrepresentable infinite `snr_db`.

- **Video is live too** (2026-09-28, y4m bridge): `encode` and
  `benchmark` take a YUV4MPEG2 (`.y4m`) input with `--codec thrum` and
  encode one packet per frame into a video track carrying the geometry,
  pixel format and frame rate (`--quality` maps onto Thrum's quantiser
  index); `decode` writes a video track back out as `.y4m`, so it takes
  the default container rather than `--container wav`; `transcode`
  re-encodes it. The bridge (`src/y4m.rs`) is byte-exact — what goes in
  comes out, so a lossless round trip is a byte-identical file — and
  deliberately narrow: progressive only (`Ip`), `C420*`/`Cmono` only, and
  aspect ratio is parsed then discarded because the `.tpt` track model has
  no aspect field. It lives in `tpt-cli`, not in `tpt-media-io`.
- `video-corpus` is live (2026-09-28; generalised to all three video codecs
  2026-10-01): measures `thrum`, `vault` or `mote` (`--codec`, default
  `thrum`) over the §36 synthetic video corpus — PSNR, SSIM, bitrate,
  bits-per-pixel, and encode/decode CPU and frame latency per scene —
  generating its own content so it reads no files (`--fps`, `--frames`,
  `--json`). It reports the codec it actually measured, and counts *decoded
  pictures* rather than packets in the frame totals, because Mote prefixes
  its first frame with a dictionary packet that carries none.

## Testability

All commands execute through `tpt_cli::run(argv_without_prog, &Ctx)` where
`Ctx` injects `read_file`, `read_stdin` and `write_file` closures. Binary
stdout (an output path of `-` or omitted) is returned as
`Outcome.binary_stdout` for tests to inspect. Tests in
`src/lib.rs#tests` supply in-memory fakes and assert on exit code +
captured streams (including a full Aura encode→decode round trip), so the
whole surface is covered deterministically with no I/O. File/stdin/stdout
handling is confined to `src/main.rs`.

## Files

- `apps/tpt-cli/src/main.rs` — binary: argv, real fs/stdin, exit propagation.
- `apps/tpt-cli/src/lib.rs` — `run()`, public API, unit tests.
- `apps/tpt-cli/src/args.rs` — hand-rolled parser, `Command`/`OpSpec`,
  `resolve_codec`, `parse_bitrate`, `USAGE`.
- `apps/tpt-cli/src/json.rs` — minimal deterministic JSON writer.
- `apps/tpt-cli/src/commands.rs` — command execution, `probe_json`,
  `list_codecs`, encode-family validation.