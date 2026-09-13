# Security Policy

`tpt-media` processes untrusted media files from the public internet (browser
drag-and-drop, network streams, downloaded files). The codecs, container
parsers, and bitstream readers are the security boundary.

## Threat Model

`tpt-media` assumes the **input media file is hostile**. An attacker may
craft a `.tpt` file (or a raw bitstream for a codec) whose goal is one or
more of:

| Goal                                     | Mitigation in `tpt-media`               |
| ---------------------------------------- | --------------------------------------- |
| Crash the decoder (DoS)                  | Bounded resource limits (see §19)       |
| Allocate unbounded memory                | Hard limits on frame size, packet size, allocation, reference frames |
| Trigger stack overflow via recursion     | Recursion depth limits                  |
| Cause integer overflow                   | Audited arithmetic, checked arithmetic where possible |
| Read out of bounds                       | Bounds-checked readers, fuzz-tested parsers |
| Hijack control flow via panic            | No `unwrap` in library code, no panics on malformed input |
| Exfiltrate data via timing side-channels | Constant-time primitives where relevant; documented limitations elsewhere |
| Exploit FFI boundaries                   | Minimal C surface, opaque handles, explicit ownership, documented thread-safety |

## Supported Versions

| Version | Supported           |
| ------- | ------------------- |
| 0.1.x   | :white_check_mark: Yes |
| < 0.1   | :x: No              |

## Reporting a Vulnerability

**Please do not file a public issue for security problems.**

Email `security@tpt-solutions.example` (PGP key to be added). We will
acknowledge within 3 business days and aim to issue a fix or mitigation
within 30 days for critical issues, 90 days for others.

## Audits

Scheduled for the 1.0 release; completed ones are recorded with the date
they landed and what they found:

- [x] Integer-overflow audit (every public API) — done 2026-09-11; fixed a 32-bit wrap in `decode_tmta`.
- [x] Bounds-check audit (every reader) — done 2026-09-11; fixed an out-of-bounds spectrum write in Pulse.
- [x] Allocation-exhaustion audit (every decoder entry point) — done 2026-09-11; fixed the `decode_idx` entry-count bomb.
- [x] Decompression-bomb protection test (every codec) — done 2026-09-11 (same fix + §19 resource limits).
- [x] Fuzzing CI operational for bitstream, container, codec, and probe targets — done: deterministic, seeded corruption/truncation sweeps run as ordinary tests in CI (see `docs/security.md`).
- [x] Malformed-timestamp audit — done 2026-09-14; extreme/duplicate/descending PTS handled without panic (one unsigned-envelope quirk documented).
- [ ] External security review (pre-1.0).

See `docs/security.md` for the full threat model, audit details, and the
fuzzing strategy.