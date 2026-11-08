#!/usr/bin/env python3
"""Refuse repository links outside the public document manifest in the llms files.

`scripts/gen-llms-txt.py` runs this before it writes. `prek` runs it on the
committed `llms.txt` and `llms-full.txt`. Both read `scripts/leakguard.toml`,
the manifest of the repository files the generated files may link to; a link
into the repository that names anything else is refused. Links to other sites
are the manual's references and are not governed here.
"""

from __future__ import annotations

import posixpath
import re
import sys
import tomllib
from pathlib import Path
from urllib.parse import unquote, urlparse

ROOT = Path(__file__).resolve().parents[1]
CONFIG = ROOT / "scripts" / "leakguard.toml"
CHECKED = ("llms.txt", "llms-full.txt")
# Where a URL names a file of this repository, at any ref: the manifest governs
# the file, not the ref it is read at. Host and owner/repository compare
# case-insensitively (GitHub treats them so); the document path keeps its case.
REPO_OWNER = "haozeke"
REPO_NAME = "eb-stack"
GITHUB_HOSTS = ("github.com", "www.github.com")
GITHUB_VIEWS = ("blob", "raw", "tree")
RAW_HOSTS = ("raw.githubusercontent.com",)

LINK = re.compile(r"\[[^\]]*\]\((https?://[^)\s]+)\)")


def repository_file(url: str) -> str | None:
    """The repository path a URL names, canonical, or None for another site.

    The host comes from `urlparse().hostname`, so user information and a port
    do not change where a link points; host and owner/repository compare in
    lower case. Percent-encoding is undone until stable, backslashes count as
    separators, the path is normalized, and a path that escapes the tree (a
    `..` segment left over, an absolute path, an empty name) comes back as ""
    so the manifest refuses it. The ref segment after blob/raw/tree is dropped.
    """
    parsed = urlparse(url)
    host = (parsed.hostname or "").lower()
    path = parsed.path
    for _ in range(4):
        decoded = unquote(path)
        if decoded == path:
            break
        path = decoded
    path = path.replace("\\", "/")
    segments = path.split("/")
    # ['', owner, repo, view, ref, ...] on github.com; ['', owner, repo, ref, ...] on raw.
    if len(segments) < 4 or segments[0] != "":
        return None
    owner, repo = segments[1].lower(), segments[2].lower()
    if (owner, repo) != (REPO_OWNER, REPO_NAME):
        return None
    if host in GITHUB_HOSTS:
        if len(segments) < 5 or segments[3] not in GITHUB_VIEWS:
            return None
        relative_segments = segments[5:]
    elif host in RAW_HOSTS:
        relative_segments = segments[4:]
    else:
        return None
    relative = "/".join(relative_segments)
    if not relative or relative.startswith("/"):
        return ""
    normalized = posixpath.normpath(relative)
    if normalized in (".", "..") or normalized.startswith(("../", "/")):
        return ""
    if any(segment in ("..", "") for segment in normalized.split("/")):
        return ""
    return normalized


class Manifest:
    """The repository files the generated public files may link to."""

    def __init__(self, paths: set[str], prefixes: tuple[str, ...]):
        self.paths = paths
        self.prefixes = prefixes

    def allows(self, url: str) -> bool:
        relative = repository_file(url)
        if relative is None:
            return True
        if relative == "":
            return False
        if relative in self.paths or relative.startswith(self.prefixes):
            return True
        # A link to a public directory itself (`tree/main/docs`).
        return any(relative == prefix.rstrip("/") for prefix in self.prefixes)


def string_list(data: dict, key: str) -> list[str]:
    values = data.get(key, [])
    if not isinstance(values, list) or not all(isinstance(value, str) for value in values):
        raise SystemExit(f"scripts/leakguard.toml: {key} must be a list of strings")
    return values


def manifest() -> Manifest:
    data = tomllib.loads(CONFIG.read_text(encoding="utf-8"))
    return Manifest(
        set(string_list(data, "public_repo_paths")),
        tuple(string_list(data, "public_repo_prefixes")),
    )


def violations(name: str, text: str, allow: Manifest) -> list[str]:
    found = []
    for url in LINK.findall(text):
        if allow.allows(url):
            continue
        found.append(
            f"leakguard: {name} links a repository file outside the public document manifest ({url}); "
            "add the document to scripts/leakguard.toml or drop the link"
        )
    return found


def violations_in(files: dict[str, str], allow: Manifest | None = None) -> list[str]:
    allowed = manifest() if allow is None else allow
    found = []
    for name, text in files.items():
        found.extend(violations(name, text, allowed))
    return found


def main() -> int:
    allow = manifest()
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
