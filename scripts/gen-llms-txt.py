#!/usr/bin/env python3
"""Write llms.txt, llms-full.txt, and per-page markdown from the Org manual.

The Org files under docs/orgmode are the manual. Sphinx publishes each page
as HTML. This script writes the markdown form the llms.txt index links to,
plus one file that concatenates those pages. llms.txt and llms-full.txt are
committed. The per-page files under docs/llm are generated at docs build
time and are not stored in git. Run with --check to fail when the committed
files differ from a fresh generation.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
ORG_ROOT = ROOT / "docs" / "orgmode"
LLM_ROOT = ROOT / "docs" / "llm"
SITE = "https://eb-stack.rgoswami.me"
REPO = "https://github.com/HaoZeke/eb-stack/blob/main"
INDEX_NAME = "llms.txt"
FULL_NAME = "llms-full.txt"

SECTION_FOR_PREFIX = (
    ("howto/", "How-to guides"),
    ("reference/", "Reference"),
    ("explanation/", "Explanation"),
)

OPTIONAL_STEMS = {"contributing", "changelog"}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check",
        action="store_true",
        help="exit 1 when committed llms.txt or llms-full.txt differs",
    )
    parser.add_argument(
        "--pages-only",
        action="store_true",
        help="write docs/llm page markdown and leave the committed files",
    )
    args = parser.parse_args()
    pages = load_pages()
    files = render(pages)
    if args.check and args.pages_only:
        print("pass only one of --check and --pages-only", file=sys.stderr)
        return 2
    if args.check:
        return check(files)
    if args.pages_only:
        write({name: text for name, text in files.items() if name.startswith("docs/llm/")})
        return 0
    write(files)
    return 0


def load_pages() -> list[dict[str, str]]:
    order = toctree_order((ORG_ROOT / "index.org").read_text(encoding="utf-8"))
    found = {
        path.relative_to(ORG_ROOT).with_suffix("").as_posix(): path
        for path in sorted(ORG_ROOT.rglob("*.org"))
    }
    sequence: list[str] = []
    for name in order:
        if name in found and name not in sequence:
            sequence.append(name)
    if "index" not in sequence:
        sequence.insert(0, "index")
    for name in sorted(found):
        if name not in sequence:
            sequence.append(name)
    pages = []
    for name in sequence:
        source = found[name].read_text(encoding="utf-8")
        title = page_title(source, name)
        body = org_to_markdown(source)
        pages.append(
            {
                "name": name,
                "title": title,
                "body": body,
                "summary": summarize(body),
                "section": section_for(name),
            }
        )
    return pages


def toctree_order(index_text: str) -> list[str]:
    order: list[str] = []
    in_tree = False
    for line in index_text.splitlines():
        stripped = line.strip()
        if stripped.startswith(".. toctree::"):
            in_tree = True
            continue
        if not in_tree:
            continue
        if stripped.startswith(":"):
            continue
        if stripped == "":
            continue
        if line.startswith("   ") and " " not in stripped and not stripped.startswith("."):
            order.append(stripped)
            continue
        in_tree = False
    return order


def page_title(source: str, name: str) -> str:
    for line in source.splitlines():
        if line.upper().startswith("#+TITLE:"):
            raw = line.split(":", 1)[1].strip()
            return inline_plain(raw) or name
    return name


def section_for(name: str) -> str:
    if name in OPTIONAL_STEMS:
        return "Optional"
    if name in {"index", "tutorial"}:
        return "Getting started"
    for prefix, section in SECTION_FOR_PREFIX:
        if name.startswith(prefix):
            return section
    return "Optional"


def org_to_markdown(source: str) -> str:
    lines = source.splitlines()
    out: list[str] = []
    state = "normal"
    i = 0
    while i < len(lines):
        line = lines[i]
        stripped = line.strip()
        upper = stripped.upper()
        if state == "export":
            if upper.startswith("#+END_EXPORT"):
                state = "normal"
            i += 1
            continue
        if state == "src":
            if upper.startswith("#+END_SRC") or upper.startswith("#+END_EXAMPLE"):
                out.append("```")
                state = "normal"
            else:
                out.append(line.rstrip())
            i += 1
            continue
        if state == "quote":
            if upper.startswith("#+END_QUOTE"):
                state = "normal"
            else:
                out.append("> " + inline(line.strip()))
            i += 1
            continue
        if state == "drawer":
            if stripped == ":END:":
                state = "normal"
            i += 1
            continue
        if upper.startswith("#+BEGIN_EXPORT"):
            state = "export"
            i += 1
            continue
        if upper.startswith("#+BEGIN_SRC") or upper.startswith("#+BEGIN_EXAMPLE"):
            lang = ""
            if upper.startswith("#+BEGIN_SRC"):
                parts = stripped.split()
                if len(parts) > 1 and not parts[1].startswith(":"):
                    lang = parts[1]
            out.append(f"```{lang}")
            state = "src"
            i += 1
            continue
        if upper.startswith("#+BEGIN_QUOTE"):
            state = "quote"
            i += 1
            continue
        if stripped.startswith(":") and stripped.endswith(":") and stripped != ":END:":
            # A property drawer, not a definition line in prose.
            if upper.startswith(":PROPERTIES:") or upper.startswith(":LOGBOOK:"):
                state = "drawer"
                i += 1
                continue
        if upper.startswith("#+") or stripped.startswith("# "):
            i += 1
            continue
        heading = re.match(r"^(\*+)\s+(.*)$", line)
        if heading:
            level = min(len(heading.group(1)), 6)
            out.append("#" * level + " " + inline(heading.group(2).strip()))
            i += 1
            continue
        if stripped.startswith("|"):
            out.append(table_line(stripped, out))
            i += 1
            continue
        if stripped == "":
            out.append("")
            i += 1
            continue
        out.append(inline(line.rstrip()))
        i += 1
    text = "\n".join(out).strip() + "\n"
    text = re.sub(r"\n{3,}", "\n\n", text)
    return text


def table_line(stripped: str, previous: list[str]) -> str:
    cells = stripped.strip("|").split("|")
    if set(stripped.strip("| ")) <= set("-|+"):
        columns = 1
        if previous and previous[-1].startswith("|"):
            columns = max(previous[-1].count("|") - 1, 1)
        else:
            columns = max(len(cells), 1)
        return "| " + " | ".join(["---"] * columns) + " |"
    rendered = [inline(cell.strip()) for cell in cells]
    return "| " + " | ".join(rendered) + " |"


def inline(text: str) -> str:
    text = re.sub(
        r"\[\[file:([^]\n]+?)\]\[([^]\n]+?)\]\]",
        lambda match: file_link(match.group(1), match.group(2)),
        text,
    )
    text = re.sub(
        r"\[\[(https?://[^]\s]+?)\]\[([^]\n]+?)\]\]",
        lambda match: f"[{inline_plain(match.group(2))}]({match.group(1)})",
        text,
    )
    text = re.sub(
        r"\[\[(https?://[^]\s]+?)\]\]",
        lambda match: match.group(1),
        text,
    )
    text = re.sub(r"<<[^>\n]+>>", "", text)

    def code(match: re.Match[str]) -> str:
        return "`" + match.group(1) + "`"

    text = re.sub(r"~([^~\n]+)~", code, text)
    text = re.sub(r"=([^=\n]+)=", code, text)
    text = re.sub(r"(?<!\*)\*([^*\n]+)\*(?!\*)", r"**\1**", text)
    return text


def file_link(target: str, label: str) -> str:
    stem = target.removeprefix("./")
    if stem.endswith(".org"):
        stem = stem[: -len(".org")]
    return f"[{inline_plain(label)}]({SITE}/{stem}.html.md)"


def inline_plain(text: str) -> str:
    text = re.sub(r"~([^~\n]+)~", r"\1", text)
    text = re.sub(r"=([^=\n]+)=", r"\1", text)
    text = re.sub(r"\[\[https?://[^]\s]+\]\[([^]\n]+)\]\]", r"\1", text)
    text = re.sub(r"\[\[file:[^]\n]+\]\[([^]\n]+)\]\]", r"\1", text)
    text = text.replace("*", "")
    text = re.sub(r"(?<![\w/])/([A-Za-z][^/\n]{0,40})/(?![\w/])", r"\1", text)
    return " ".join(text.split())


def summarize(body: str) -> str:
    paragraph: list[str] = []
    in_fence = False
    for line in body.splitlines():
        if line.startswith("```"):
            if paragraph:
                break
            in_fence = not in_fence
            continue
        if in_fence or line.startswith("#") or line.startswith("|"):
            if paragraph:
                break
            continue
        if line.strip() == "":
            if paragraph:
                break
            continue
        paragraph.append(line.strip())
    text = inline_plain(" ".join(paragraph))
    text = re.sub(r"\[([^\]]+)\]\([^)]+\)", r"\1", text)
    text = text.replace("`", "")
    if ". " in text:
        first = text.split(". ", 1)[0]
        if len(first) >= 40:
            text = first + "."
    if len(text) > 220:
        text = text[:217].rsplit(" ", 1)[0]
    return text or "Manual page."


def render(pages: list[dict[str, str]]) -> dict[str, str]:
    files: dict[str, str] = {}
    full_parts = [
        "# eb-stack",
        "",
        "> "
        + "eb-stack parses conda-forge, Spack, PyPI, CRAN, Cargo, LuaRocks, "
        + "Raku, and EasyBuild recipes into one package plan, a planned "
        + "CycloneDX SBOM, Resolvo locks, and conventional EasyBuild recipes.",
        "",
        "This file is the manual in one document. The short index is llms.txt.",
        "",
    ]
    for page in pages:
        page_md = f"# {page['title']}\n\n{page['body']}"
        if not page_md.endswith("\n"):
            page_md += "\n"
        files[f"docs/llm/{page['name']}.md"] = page_md
        full_parts.append(page_md.rstrip())
        full_parts.append("")
    files[FULL_NAME] = "\n".join(full_parts).rstrip() + "\n"
    files[INDEX_NAME] = render_index(pages)
    return files


def render_index(pages: list[dict[str, str]]) -> str:
    sections = [
        "Getting started",
        "How-to guides",
        "Reference",
        "Explanation",
        "Skills",
        "Project",
        "Optional",
    ]
    grouped: dict[str, list[str]] = {name: [] for name in sections}
    for page in pages:
        url = f"{SITE}/{page['name']}.html.md"
        grouped[page["section"]].append(
            f"- [{page['title']}]({url}): {page['summary']}"
        )
    for skill in skill_entries():
        grouped["Skills"].append(skill)
    grouped["Project"].extend(project_entries())
    lines = [
        "# eb-stack",
        "",
        "> "
        + "eb-stack parses conda-forge, Spack, PyPI, CRAN, Cargo, LuaRocks, "
        + "Raku, and EasyBuild recipes into one package plan, a planned "
        + "CycloneDX SBOM, Resolvo locks, and conventional EasyBuild recipes.",
        "",
        "The binary is `eb-stack`. The crate is `eb_stack`. Rust 1.88 or "
        "newer builds it from source. Each link below is the markdown form "
        "of a page on https://eb-stack.rgoswami.me/. `llms-full.txt` repeats "
        "those pages in one file. Contributor instructions are in AGENTS.md. "
        "The Model Context Protocol server is `eb-stack mcp`.",
        "",
    ]
    for name in sections:
        items = grouped[name]
        if not items:
            continue
        lines.append(f"## {name}")
        lines.append("")
        lines.extend(items)
        lines.append("")
    return "\n".join(lines).rstrip() + "\n"


def skill_entries() -> list[str]:
    entries = []
    for path in sorted((ROOT / "skills").glob("*/SKILL.md")):
        text = path.read_text(encoding="utf-8")
        name, description = skill_frontmatter(text, path.parent.name)
        url = f"{REPO}/{path.relative_to(ROOT).as_posix()}"
        entries.append(f"- [{name}]({url}): {description}")
    return entries


def skill_frontmatter(text: str, fallback: str) -> tuple[str, str]:
    if not text.startswith("---\n"):
        return fallback, "Skill."
    end = text.find("\n---\n", 4)
    if end < 0:
        return fallback, "Skill."
    block = text[4:end]
    name = fallback
    description = "Skill."
    for line in block.splitlines():
        if line.startswith("name:"):
            name = line.split(":", 1)[1].strip()
        elif line.startswith("description:"):
            description = line.split(":", 1)[1].strip()
    if len(description) > 220:
        description = description[:217].rsplit(" ", 1)[0]
    return name, description


def project_entries() -> list[str]:
    entries = [
        (
            "README",
            f"{REPO}/README.md",
            "Install, the claim ladder, and pointers to the manual, the skills, and the Model Context Protocol server.",
        ),
        (
            "AGENTS.md",
            f"{REPO}/AGENTS.md",
            "Build, test, lint, and documentation commands for a contributor.",
        ),
    ]
    if (ROOT / "server.json").is_file():
        entries.append(
            (
                "MCP server manifest",
                f"{REPO}/server.json",
                "Registry entry for the eb-stack Model Context Protocol server.",
            )
        )
    return [f"- [{title}]({url}): {summary}" for title, url, summary in entries]


def write(files: dict[str, str]) -> None:
    for relative, text in files.items():
        path = ROOT / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
    stale = []
    if LLM_ROOT.is_dir():
        live = {path.relative_to(ROOT).as_posix() for path in LLM_ROOT.rglob("*.md")}
        expected = {name for name in files if name.startswith("docs/llm/")}
        stale = sorted(live - expected)
    for relative in stale:
        (ROOT / relative).unlink()


def check(files: dict[str, str]) -> int:
    drifted = []
    for relative in (INDEX_NAME, FULL_NAME):
        text = files[relative]
        path = ROOT / relative
        current = path.read_text(encoding="utf-8") if path.is_file() else None
        if current != text:
            drifted.append(relative)
    if drifted:
        print("llms.txt drift:", file=sys.stderr)
        for name in drifted:
            print(f"  {name}", file=sys.stderr)
        print("Run: python3 scripts/gen-llms-txt.py", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
