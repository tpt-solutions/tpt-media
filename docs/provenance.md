# Provenance and IP Policy

`tpt-media` is implemented from public-domain mathematics only. Every
non-trivial algorithm carries a provenance record identifying its
source, classification, and last patent review.

## Per-Algorithm Record

For each algorithm, we record:

| Field           | Description                                    |
| --------------- | ---------------------------------------------- |
| `algorithm`     | Human name of the algorithm                     |
| `reference`     | Source paper / textbook / expired patent #     |
| `source`        | GREEN / YELLOW / RED classification             |
| `derivation`    | Where in this repo the implementation lives    |
| `implementation`| Link to the source file                        |
| `license`       | License of the reference work                  |
| `patent review` | Date + reviewer initials                       |
| `status`        | reviewed / under-review / blocked              |

## Source Classifications

```text
GREEN    Public-domain mathematics, textbooks, academic papers,
         expired patents, public standards where legally appropriate.

YELLOW   Open-source implementations (with license compatibility),
         modern patents (with explicit avoidance), ambiguous licensing,
         vendor documentation.

RED      Proprietary implementation source, confidential information,
         restricted specifications, unclear ownership.
```

`tpt-media` accepts only **GREEN** sources. YELLOW sources are allowed
only with an explicit waiver from the maintainers and a documented
derivation path. RED sources are forbidden.

## Per-Crate Records

Each codec crate and each non-trivial module has a `PROVENANCE.md`
that records the algorithms it implements. Example:

```markdown
## Pulse — Lossless Mode

| Algorithm       | Reference                              | Source | License | Patent review | Status   |
| --------------- | -------------------------------------- | ------ | ------- | ------------- | -------- |
| MDCT (N=128)    | Princen & Bradley, 1986                | GREEN  | -       | 2026-09-07 PT | reviewed |
| Sine window     | Oppenheim & Schafer, ch. 7             | GREEN  | -       | 2026-09-07 PT | reviewed |
| Uniform scalar quantizer | Widrow & Kollár, ch. 4         | GREEN  | -       | 2026-09-07 PT | reviewed |
| Static Huffman  | Sayood, "Introduction to Data Compression", ch. 5 | GREEN | - | 2026-09-07 PT | reviewed |
```

## Contribution Rules

See [`CONTRIBUTING.md`](../CONTRIBUTING.md) §2 for the clean-room rules
every contributor must follow.

## When in Doubt

If you are unsure whether a planned algorithm is GREEN, open an issue
with the `provenance/question` label **before** writing any code. We
prefer to spend time on classification rather than later rewrite
work.