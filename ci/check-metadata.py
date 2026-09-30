#!/usr/bin/env python3
"""Workspace metadata gate (CONTRIBUTING.md §9.4).

Every workspace member must ship:

  * a README.md and a CHANGELOG.md of its own,
  * at most five `keywords`,
  * only valid crates.io `categories`,
  * a `readme` field that points at a file that exists,
  * a keyword set distinct from every other member's.

The last rule is the point of the gate: sixteen crates sharing one inherited
keyword list is worse than no list at all, because it looks like curation and
is not.

Run locally:      python ci/check-metadata.py
Requires Python   3.11+ (tomllib).
"""

from __future__ import annotations

import re
import sys
import tomllib
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent

# A member's keyword must match crates.io's rule: alphanumerics, '-' and '_'.
KEYWORD_RE = re.compile(r"^[a-zA-Z0-9_-]+$")

# Valid crates.io category slugs. Not the complete list — this is the subset
# a media/DSP/FFI crate can plausibly want. Adding a category here is a
# deliberate act; if your crate needs one that is missing, that is a signal
# to check the slug really exists first.
VALID_CATEGORIES = {
    "algorithms",
    "api-bindings",
    "application-development",
    "asynchronous",
    "audio",
    "benchmarking",
    "buffering",
    "command-line-interface",
    "command-line-utilities",
    "compression",
    "concurrency",
    "config",
    "data-structures",
    "database",
    "development-tools",
    "development-tools::build-utils",
    "development-tools::cargo-build-plugins",
    "development-tools::ffi",
    "development-tools::profiling",
    "development-tools::testing",
    "encoding",
    "external-ffi-bindings",
    "filesystem",
    "game-development",
    "graphics",
    "image-processing",
    "indexing",
    "logging",
    "macros",
    "mathematics",
    "memory-management",
    "multimedia",
    "multimedia::audio",
    "multimedia::images",
    "multimedia::video",
    "networking",
    "no-std",
    "os",
    "parser-implementations",
    "parsing",
    "science",
    "science::astronomy",
    "science::bioinformatics",
    "science::physics",
    "signal-processing",
    "simulation",
    "software-development",
    "system-programming",
    "templates",
    "text-processing",
    "virtualization",
    "wasm",
    "web-programming",
}

MAX_KEYWORDS = 5
MAX_CATEGORIES = 5


def load_toml(path: Path) -> dict:
    with path.open("rb") as fh:
        return tomllib.load(fh)


def main() -> int:
    root = load_toml(REPO_ROOT / "Cargo.toml")
    members = root.get("workspace", {}).get("members", [])
    if not members:
        print("error: no workspace members found in Cargo.toml")
        return 1

    errors: list[str] = []
    keyword_sets: dict[str, str] = {}

    for rel in members:
        crate_dir = REPO_ROOT / rel
        manifest_path = crate_dir / "Cargo.toml"
        if not manifest_path.is_file():
            errors.append(f"{rel}: no Cargo.toml")
            continue

        manifest = load_toml(manifest_path)
        pkg = manifest.get("package", {})
        name = pkg.get("name", rel)

        # 1. A README.md and a CHANGELOG.md of its own.
        for required in ("README.md", "CHANGELOG.md"):
            if not (crate_dir / required).is_file():
                errors.append(f"{name}: missing {required} (CONTRIBUTING.md §9.4)")

        # 2. The declared readme must exist. Cargo resolves an inherited
        #    `readme` relative to the *member* directory, so an inherited
        #    path that exists at the workspace root still fails to package.
        readme = pkg.get("readme")
        if readme is None:
            errors.append(
                f"{name}: no `readme` key — declare `readme = \"README.md\"` "
                f"explicitly rather than relying on inheritance"
            )
        elif not (crate_dir / readme).is_file():
            errors.append(f"{name}: readme {readme!r} does not exist")

        # 3. Keywords.
        keywords = pkg.get("keywords", [])
        if not keywords:
            errors.append(f"{name}: no keywords declared")
        if len(keywords) > MAX_KEYWORDS:
            errors.append(
                f"{name}: {len(keywords)} keywords exceeds the crates.io "
                f"maximum of {MAX_KEYWORDS}"
            )
        for kw in keywords:
            if not KEYWORD_RE.match(kw):
                errors.append(f"{name}: invalid keyword {kw!r}")
        deduped = tuple(sorted(keywords))
        if deduped in keyword_sets:
            errors.append(
                f"{name}: keyword set is identical to {keyword_sets[deduped]} — "
                f"per-crate keywords are the point (§9.4)"
            )
        else:
            keyword_sets[deduped] = name

        # 4. Categories.
        categories = pkg.get("categories", [])
        if not categories:
            errors.append(f"{name}: no categories declared")
        if len(categories) > MAX_CATEGORIES:
            errors.append(
                f"{name}: {len(categories)} categories exceeds the crates.io "
                f"maximum of {MAX_CATEGORIES}"
            )
        for cat in categories:
            if cat not in VALID_CATEGORIES:
                errors.append(
                    f"{name}: {cat!r} is not a known crates.io category "
                    f"(if it is genuinely valid, add it to VALID_CATEGORIES)"
                )

        # 5. The basics `cargo publish` requires anyway.
        for required in ("description", "license", "repository"):
            if not pkg.get(required):
                errors.append(f"{name}: no `{required}` declared")

    if errors:
        print(f"metadata check FAILED ({len(errors)} problem(s)):\n")
        for err in errors:
            print(f"  - {err}")
        return 1

    print(
        f"metadata check passed: {len(members)} crates, each with a README.md, "
        f"a CHANGELOG.md, and distinct keywords/categories."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
