#!/usr/bin/env python3
"""Reject Rust doc comments whose code spans cannot become valid RST.

sphinx-rustdocgen rewrites a rustdoc markdown code span (`foo`) into an RST
inline literal (``foo``). RST is stricter than markdown about the delimiters:

- the closing ``-delimiter may not be preceded by whitespace, and the opening
  one may not be followed by it, so a padded span (`` `line N: ` ``) is a
  markdown code span but invalid RST;
- inline markup must be separated from surrounding word characters, so a span
  butted straight against a letter (`` `foo`s ``) is invalid too.

docutils reports both as "Inline literal start-string without end-string" at a
line in the *generated* RST. Nothing in that message points back at the Rust
source, which is why one padded span sat unnoticed in the tree for weeks of
red Documentation builds. This check runs on the source and names the actual
file, line and span.

Usage: check-doc-rst-spans.py [path ...]   (default: src)
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

DOC_LINE = re.compile(r"^\s*(///|//!)(.*)$")
# RST allows these immediately after inline markup; a word character does not.
CLOSING_OK = set(" \t-.,:;!?\\/'\")]}>")


def code_spans(body: str) -> list[tuple[str, int, int]]:
    """Code spans on one doc line as (content, start, end).

    Backticks alternate open/close, so split rather than regex-match pairs: a
    naive pattern happily matches from one span's closer to the next span's
    opener and reports the prose between them.
    """
    if body.count("`") % 2 == 1:
        return []  # unbalanced; reported separately
    spans: list[tuple[str, int, int]] = []
    pos = 0
    parts = body.split("`")
    for i, part in enumerate(parts):
        if i % 2 == 1:  # odd parts are span contents
            spans.append((part, pos - 1, pos + len(part) + 1))
        pos += len(part) + 1
    return spans


def doc_blocks(lines: list[str]) -> list[tuple[int, str]]:
    """Consecutive doc-comment lines joined into (first_lineno, text) blocks.

    A code span may legitimately wrap across two doc lines; the generator joins
    the block before emitting RST, so checking line by line would flag every
    wrapped span as unbalanced. Join first, then check.
    """
    blocks: list[tuple[int, str]] = []
    start: int | None = None
    parts: list[str] = []
    for lineno, line in enumerate(lines, start=1):
        m = DOC_LINE.match(line)
        if m:
            if start is None:
                start = lineno
            parts.append(m.group(2).strip())
        elif start is not None:
            blocks.append((start, " ".join(parts)))
            start, parts = None, []
    if start is not None:
        blocks.append((start, " ".join(parts)))
    return blocks


def check_file(path: Path) -> list[str]:
    problems: list[str] = []
    lines = path.read_text(encoding="utf-8").splitlines()
    for lineno, body in doc_blocks(lines):
        if body.count("`") % 2 == 1:
            problems.append(
                f"{path}:{lineno}: doc comment has an unbalanced backtick, which "
                f"cannot close an RST inline literal"
            )
            continue
        for content, _start, end in code_spans(body):
            if not content:
                continue
            if content[0].isspace() or content[-1].isspace():
                problems.append(
                    f"{path}:{lineno}: code span `{content}` is padded with "
                    f"whitespace; RST inline literals may not be, so drop the padding"
                )
            after = body[end] if end < len(body) else " "
            if after not in CLOSING_OK and not after.isspace():
                problems.append(
                    f"{path}:{lineno}: code span `{content}` is followed by "
                    f"{after!r}; RST needs whitespace or punctuation after inline markup"
                )
    return problems


def main(argv: list[str]) -> int:
    roots = [Path(a) for a in argv[1:]] or [Path("src")]
    files: list[Path] = []
    for root in roots:
        files.extend(sorted(root.rglob("*.rs")) if root.is_dir() else [root])
    problems: list[str] = []
    for f in files:
        problems.extend(check_file(f))
    if problems:
        print("\n".join(problems), file=sys.stderr)
        print(
            f"\n{len(problems)} doc-comment span(s) would produce invalid RST",
            file=sys.stderr,
        )
        return 1
    print(f"checked {len(files)} file(s): doc-comment code spans are RST-safe")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
