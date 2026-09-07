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

The following audits are scheduled for the 1.0 release:

- [ ] Integer-overflow audit (every public API).
- [ ] Bounds-check audit (every reader).
- [ ] Allocation-exhaustion audit (every decoder entry point).
- [ ] Decompression-bomb protection test (every codec).
- [ ] Fuzzing CI operational for bitstream, container, codec, and probe targets.

See `docs/security.md` for the full threat model and audit checklist.