#!/usr/bin/env python3
"""Refuse agent-instruction links in the public llms files unless allowlisted.

`scripts/gen-llms-txt.py` runs this before it writes. `prek` runs it on the
committed `llms.txt` and `llms-full.txt`. Both read `scripts/leakguard.toml`.
"""

from __future__ import annotations

import re
import sys
import tomllib
from pathlib import Path
from urllib.parse import urlparse

ROOT = Path(__file__).resolve().parents[1]
CONFIG = ROOT / "scripts" / "leakguard.toml"
CHECKED = ("llms.txt", "llms-full.txt")

# The manual index publishes this name. Other instruction files are the same
# class of link, so a later page cannot add one without an allowlist entry.
INSTRUCTION_FILES = ("AGENTS.md", "CLAUDE.md", "GEMINI.md")

LINK = re.compile(r"\[[^\]]*\]\((https?://[^)\s]+)\)")


def allow_urls() -> set[str]:
    data = tomllib.loads(CONFIG.read_text(encoding="utf-8"))
    urls = data.get("allow_urls", [])
    if not isinstance(urls, list) or not all(isinstance(url, str) for url in urls):
        raise SystemExit("scripts/leakguard.toml: allow_urls must be a list of strings")
    return set(urls)


def instruction_file(url: str) -> str | None:
    name = Path(urlparse(url).path).name
    if name in INSTRUCTION_FILES:
        return name
    return None


def violations(name: str, text: str, allow: set[str]) -> list[str]:
    found = []
    for url in LINK.findall(text):
        filename = instruction_file(url)
        if filename is None or url in allow:
            continue
        found.append(
            f"leakguard: {name} links to {filename} ({url}); "
            "allowlist the URL in scripts/leakguard.toml or drop the link"
        )
    return found


def violations_in(files: dict[str, str], allow: set[str] | None = None) -> list[str]:
    allowed = allow_urls() if allow is None else allow
    found = []
    for name, text in files.items():
        found.extend(violations(name, text, allowed))
    return found


def main() -> int:
    allow = allow_urls()
    found = []
    for name in CHECKED:
        path = ROOT / name
        text = path.read_text(encoding="utf-8") if path.is_file() else ""
        found.extend(violations(name, text, allow))
    if found:
        for line in found:
            print(line, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
