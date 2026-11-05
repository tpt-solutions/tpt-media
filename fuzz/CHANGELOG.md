# Changelog — tpt-media-fuzz

Scoped record of changes to `tpt-media-fuzz`, the coverage-guided fuzz harness
in this directory. It is not a published crate and is not part of the workspace,
so it does not follow the lockstep version. The workspace-wide release log is the
root [`CHANGELOG.md`](../CHANGELOG.md), which records any change that touches the
harness together with the crates it exercises (CONTRIBUTING §9.1).

Format follows [Keep a Changelog](https://keepachangelog.com/).

## [Unreleased]

### Added

- **Coverage-guided fuzz targets and a scheduled fuzzing CI job (2026-11-04).**
  Four `cargo-fuzz` (libFuzzer) targets for the parsers that read untrusted
  input: `aura_decode`, `cinder_decode`, `container_parse` and `probe`. The
  `aura_decode` target appends a valid CRC-32 to each input, so the fuzzer does
  not stop at the checksum gate. The harness is a separate crate with its own
  `[workspace]`, so libFuzzer is not a dependency of the main workspace. CI runs
  it on a schedule. See [`README.md`](README.md) for the targets and how to run
  them.
