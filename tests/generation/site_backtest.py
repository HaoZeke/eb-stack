"""Regenerate the easyconfigs a site already builds, and diff.

The PyPI backtest asks whether a recipe can be written from upstream metadata.
This asks the question a site cares about: the recipes it builds today are ones
a maintainer already accepted, so re-emitting one at its own toolchain must
give back what is there. Anything else is a difference the tool introduced,
and someone would have to review it by hand at every bump.

The corpus is a list of easyconfig filenames, one per line or embedded in
whatever else the line holds, so a build list of any shape can be pointed at
it. Nothing about the list is stored here.

    EB_STACK_BIN     the binary under test
    EB_BUILDLIST     a file naming easyconfigs to regenerate
    EB_EASYCONFIGS   easyconfig trees, colon-separated, in ascending
                     precedence: upstream first, the site's own overlay last,
                     which is the order the tool merges them in
    EB_SITE_LIMIT    how many to try (default: all)

Three outcomes per recipe, and only the first is success: identical, differs
(with the diff), or refused (with the reason). A refusal is reported rather
than skipped, because a recipe the tool cannot read is a recipe it cannot
help with.
"""

from __future__ import annotations

import difflib
import os
import pathlib
import re
import subprocess
import sys
import tempfile

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1] / "property"))
import reference  # noqa: E402

EASYCONFIG = re.compile(r"[A-Za-z0-9_.+-]+\.eb")


def _trees() -> list[pathlib.Path]:
    raw = os.environ.get("EB_EASYCONFIGS")
    if not raw:
        sys.exit("EB_EASYCONFIGS is not set")
    trees = [pathlib.Path(part).expanduser() for part in raw.split(":") if part]
    missing = [tree for tree in trees if not tree.is_dir()]
    if missing:
        sys.exit(f"not a directory: {missing[0]}")
    return trees


def _corpus(build_list: pathlib.Path) -> list[str]:
    """Every easyconfig the build list names, commented lines included.

    A line commented out is still something the site builds or built; what it
    is not is a statement that the recipe is unreadable.
    """
    names, seen = [], set()
    for line in build_list.read_text().splitlines():
        for name in EASYCONFIG.findall(line):
            if name not in seen:
                seen.add(name)
                names.append(name)
    return names


def _locate(name: str, trees: list[pathlib.Path]) -> pathlib.Path | None:
    """The recipe as the highest-precedence tree carries it.

    Trees are given in the order the tool merges them, lowest precedence
    first, so the last one that has the file is the one that would be built.
    """
    letter = name[0].lower()
    package = name.split("-")[0]
    for tree in reversed(trees):
        direct = tree / letter / package / name
        if direct.is_file():
            return direct
        found = next(tree.rglob(name), None)
        if found is not None:
            return found
    return None


def _bump(binary: str, recipe: pathlib.Path, trees: list[pathlib.Path], work: pathlib.Path):
    """Re-emit one recipe at its own toolchain."""
    parsed = reference.read(recipe)
    if parsed is None:
        return None, "could not be read", None
    policy = work / "policy.toml"
    policy.write_text(
        "schema_version = 1\n"
        'name = "site"\n\n'
        "[toolchain]\n"
        f'name = "{parsed.toolchain_name}"\n'
        f'version = "{parsed.toolchain_version or "system"}"\n'
    )
    out = work / "out"
    command = [
        binary, "package", "bump",
        "--source", str(recipe),
        "--toolchain-name", parsed.toolchain_name,
        "--toolchain-version", parsed.toolchain_version or "system",
        "--stack-policy", str(policy),
        "--out-dir", str(out),
    ]
    for tree in trees:
        command += ["--easyconfigs", str(tree)]
    proc = subprocess.run(command, capture_output=True, text=True, timeout=1800)
    emitted = next(out.rglob("*.eb"), None) if out.is_dir() else None
    if emitted is None:
        reason = (proc.stdout + proc.stderr).strip().splitlines()
        return None, (reason[-1][:150] if reason else "no output"), None
    return emitted, None, parsed


def main() -> int:
    binary = os.environ.get("EB_STACK_BIN", "eb-stack")
    build_list = pathlib.Path(os.environ["EB_BUILDLIST"]).expanduser()
    trees = _trees()
    limit = int(os.environ.get("EB_SITE_LIMIT", "0")) or None

    names = _corpus(build_list)[:limit]
    identical, differing, refused, absent = [], [], [], []
    with tempfile.TemporaryDirectory(prefix="eb-site-backtest-") as raw:
        root = pathlib.Path(raw)
        for index, name in enumerate(names):
            recipe = _locate(name, trees)
            if recipe is None:
                absent.append(name)
                continue
            work = root / f"{index:03d}"
            work.mkdir(parents=True, exist_ok=True)
            emitted, reason, _parsed = _bump(binary, recipe, trees, work)
            if emitted is None:
                refused.append((name, reason))
                print(f"REFUSED  {name}: {reason}", flush=True)
                continue
            want = recipe.read_text()
            got = emitted.read_text()
            if want == got:
                identical.append(name)
                print(f"same     {name}", flush=True)
                continue
            diff = list(
                difflib.unified_diff(
                    want.splitlines(), got.splitlines(), "upstream", "generated", lineterm="", n=0
                )
            )
            differing.append((name, diff))
            print(f"DIFFERS  {name}", flush=True)
            for line in diff[2:8]:
                print(f"         {line}", flush=True)

    total = len(identical) + len(differing) + len(refused)
    print(f"\n{len(identical)}/{total} regenerate exactly")
    print(f"{len(differing)} differ, {len(refused)} refused, {len(absent)} not in any tree")
    if refused:
        print("\nrefusal reasons")
        reasons: dict[str, int] = {}
        for _name, reason in refused:
            key = re.sub(r"[0-9]+", "N", reason or "")[:80]
            reasons[key] = reasons.get(key, 0) + 1
        for reason, count in sorted(reasons.items(), key=lambda item: -item[1])[:10]:
            print(f"  {count:3d}  {reason}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
