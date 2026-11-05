# Publishing tracker

Status of each `tpt-media` crate on crates.io. All crates ship the lockstep
workspace version (`0.2.0`, see [CONTRIBUTING.md](CONTRIBUTING.md) §9.1).

Last updated: 2026-11-05

## Status

| Tier | Crate | Path | Depends on (internal) | Dry run | Published |
|------|-------|------|------------------------|---------|-----------|
| 0 | `tpt-media-bitstream` | `crates/tpt-media-bitstream` | — | Pass | Yes |
| 0 | `tpt-media-representation` | `crates/tpt-media-representation` | — | Pass | Yes |
| 1 | `tpt-media-core` | `crates/tpt-media-core` | bitstream | Pass | Yes |
| 2 | `tpt-media-codec` | `crates/tpt-media-codec` | bitstream, core | Pass | Yes |
| 2 | `tpt-media-container` | `crates/tpt-media-container` | bitstream, core | Pass | Yes |
| 2 | `tpt-media-io` | `crates/tpt-media-io` | core | Pass | No (rate-limited, retry) |
| 2 | `tpt-media-research` | `crates/tpt-media-research` | representation | Pending | No |
| 3 | `tpt-media-cinder` | `codecs/cinder` | bitstream, codec, core | Pending | No |
| 3 | `tpt-media-aura` | `codecs/aura` | bitstream, codec, core | Pending | No |
| 3 | `tpt-media-reed` | `codecs/reed` | bitstream, codec, core | Pending | No |
| 3 | `tpt-media-thrum` | `codecs/thrum` | bitstream, codec, core | Pending | No |
| 3 | `tpt-media-vault` | `codecs/vault` | bitstream, codec, core | Pending | No |
| 3 | `tpt-media-mote` | `codecs/mote` | bitstream, codec, core | Pending | No |
| 3 | `tpt-media-format` | `containers/tpt-container` | bitstream, codec, container, core | Pending | No |
| 4 | `tpt-media-pipeline` | `crates/tpt-media-pipeline` | codec, container, core | Pending | No |
| 4 | `tpt-media-probe` | `crates/tpt-media-probe` | codec, container, core, format | Pending | No |
| 4 | `tpt-media-sys` | `ffi/tpt-media-sys` | aura, cinder, codec, core, reed | Pending | No |
| 4 | `tpt-media-web` | `apps/tpt-web` | — | Not publishing | No |
| 5 | `tpt-media-cli` | `apps/tpt-cli` | aura, cinder, codec, container, core, format, io, mote, probe, reed, thrum, vault | Pending | No |

External dependencies already on crates.io: `tpt-dsp-core`, `tpt-dsp-io`.

The codecs (`codecs/`) and `tpt-media-format` (`containers/`) are outside
`crates/`, but they are part of the published set. `tpt-media-probe` depends
on `tpt-media-format`, so format must go out before probe.

## Procedure

Work tier by tier. A crate's dry run only succeeds once its internal
dependencies are on crates.io, so each tier is published before the next is
dry-run.

```sh
# from the workspace root
cargo publish -p <crate> --dry-run
cargo publish -p <crate>
```

After each successful publish, change that crate's row to `Yes` and record
the date in the log below.

crates.io limits how many new crates one account can publish in a short
window (HTTP 429). If that happens, wait until the time in the error and
retry the same crate.

## Log

- 2026-11-05: Tracker created.
- 2026-11-05: Added the six codec crates (`codecs/`) at Tier 3; they were missing from the first version.
- 2026-11-05: Published `tpt-media-bitstream`, `tpt-media-representation`, `tpt-media-core`, `tpt-media-codec`, `tpt-media-container`.
- 2026-11-05: `tpt-media-io` dry run passed, publish hit the crates.io new-crate rate limit (429). Retry after 10:11:17 UTC.
- 2026-11-05: `tpt-media-web` set to not publishing, per the recommendation.
