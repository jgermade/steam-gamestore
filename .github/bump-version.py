#!/usr/bin/env python3
"""Raise the workspace version in Cargo.toml and print the new one.

Every crate in the workspace inherits `version.workspace = true`, so this one
number is the version of everything gamestore ships. `release.yml` calls it, and
the tag it creates is this number with a `v` in front.

Usage: bump-version.py (major | minor | patch | X.Y.Z)
"""

import pathlib
import re
import sys

# The version line inside [workspace.package], and only that one: the same
# `version = "..."` spelling appears under [workspace.dependencies] entries.
SECTION = re.compile(r"^\[workspace\.package\]\s*$", re.MULTILINE)
VERSION = re.compile(r'^version\s*=\s*"(\d+)\.(\d+)\.(\d+)"\s*$', re.MULTILINE)
EXPLICIT = re.compile(r"^\d+\.\d+\.\d+$")


def bump(current: tuple[int, int, int], part: str) -> str:
    major, minor, patch = current
    if part == "major":
        return f"{major + 1}.0.0"
    if part == "minor":
        return f"{major}.{minor + 1}.0"
    if part == "patch":
        return f"{major}.{minor}.{patch + 1}"
    raise SystemExit(f"bump-version.py: not a version or a part to raise: {part}")


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        raise SystemExit(__doc__)

    part = argv[1]
    manifest = pathlib.Path("Cargo.toml")
    text = manifest.read_text()

    section = SECTION.search(text)
    if section is None:
        raise SystemExit("bump-version.py: Cargo.toml has no [workspace.package] section")

    found = VERSION.search(text, section.end())
    if found is None:
        raise SystemExit("bump-version.py: no version = \"X.Y.Z\" under [workspace.package]")

    if EXPLICIT.match(part):
        new = part
    else:
        new = bump((int(found[1]), int(found[2]), int(found[3])), part)

    manifest.write_text(text[: found.start()] + f'version = "{new}"' + text[found.end() :])
    print(new)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
