# Coverage-guided fuzzing

Coverage-guided fuzz targets for the parsers that read untrusted input.
The deterministic corruption sweeps in each crate's test suite stay the
first line of defence and run on every CI build. This directory adds the
long-running, coverage-guided layer (todo.md §38, §47).

This is a separate crate with its own `[workspace]`, so libFuzzer does
not become a dependency of the main workspace.

## Targets

| Target | Entry point | Notes |
| --- | --- | --- |
| `aura_decode` | `AuraDecoder::decode` | Appends a valid CRC-32 to each input, otherwise the fuzzer stops at the checksum gate. |
| `cinder_decode` | `CinderDecoder::decode` | No packet checksum. |
| `container_parse` | `tpt_media_format::parse_tpt` | Whole-file parse. |
| `probe` | `tpt_media_probe::probe` | Whole-file probe. |

## Running

Requires nightly Rust and `cargo-fuzz`, and runs on Linux (libFuzzer's
supported host; CI uses Ubuntu).

```sh
cargo install cargo-fuzz --locked
cd fuzz
cargo +nightly fuzz run aura_decode -- -max_total_time=60
```

Any crash is written to `fuzz/artifacts/<target>/`. Reproduce it with
`cargo +nightly fuzz run <target> <artifact-file>`, then add it as a
regression test in the owning crate before fixing it.
