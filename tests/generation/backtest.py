"""Regenerate easyconfigs upstream already ships, and diff against them.

The question this answers is the one that matters for generation: pointed at
an upstream package, does eb-stack write the recipe a maintainer wrote? So the
corpus is upstream's own `PythonPackage` recipes, and the comparison is against
the file itself rather than against a recorded snapshot of our own output.

Each package is regenerated with its own recipe hidden from the robot path,
because eb-stack correctly refuses to emit an overlay for something the tree
already provides, and with its version pinned to the one upstream carries, so
the two files describe the same release and every difference is ours.

    EB_STACK_BIN     the binary under test
    EB_EASYCONFIGS   an easybuild-easyconfigs checkout to read
    EB_GEN_TOOLCHAIN GCCcore version to generate at (default: 14.2.0)
    EB_GEN_LIMIT     how many packages to try (default: 8; each one fetches)

Reported per field, because "close" is not a number: a recipe whose
`moduleclass` is wrong is wrong in a way a maintainer will send back, and one
whose dependency list is wrong will not build.
"""

from __future__ import annotations

import os
import pathlib
import subprocess
import sys
import tempfile

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1] / "property"))
import reference  # noqa: E402

FIELDS = ("easyblock", "toolchain", "moduleclass", "dependencies", "builddependencies", "sources")


def _read(path: pathlib.Path) -> dict | None:
    """The fields a comparison cares about, read from an easyconfig."""
    recipe = reference.read(path)
    if recipe is None:
        return None
    text = path.read_text()
    easyblock = None
    for line in text.splitlines():
        if line.startswith("easyblock"):
            easyblock = line.split("=", 1)[1].strip().strip("'\"")
            break
    moduleclass = None
    for line in text.splitlines():
        if line.startswith("moduleclass"):
            moduleclass = line.split("=", 1)[1].strip().strip("'\"")
            break
    return {
        "name": recipe.name,
        "version": recipe.version,
        "easyblock": easyblock,
        "toolchain": recipe.toolchain_label,
        "moduleclass": moduleclass,
        "dependencies": sorted(name for name, *_ in recipe.dependencies),
        "builddependencies": sorted(name for name, *_ in recipe.builddependencies),
        # `SOURCE_TAR_GZ` and `'%(name)s-%(version)s.tar.gz'` are the same
        # sdist named the same way; upstream writes both spellings.
        "sources": (
            "name-version sdist"
            if "SOURCE_TAR_GZ" in text
            or "SOURCELOWER_TAR_GZ" in text
            or "%(name)s-%(version)s.tar.gz" in text
            or "%(namelower)s-%(version)s.tar.gz" in text
            else "explicit"
        ),
    }


def _robot_without(tree: pathlib.Path, package: str, into: pathlib.Path) -> pathlib.Path:
    """A robot path holding every recipe except one package's own.

    Symlinks per letter directory, so this costs nothing per package, except
    for the one letter that has to lose a single entry.
    """
    farm = into / f"robot-{package}"
    farm.mkdir(parents=True, exist_ok=True)
    letter = package[0].lower()
    for entry in tree.iterdir():
        if not entry.is_dir():
            continue
        target = farm / entry.name
        if target.exists() or target.is_symlink():
            continue
        if entry.name != letter:
            target.symlink_to(entry)
            continue
        target.mkdir()
        for package_dir in entry.iterdir():
            if package_dir.name != package:
                (target / package_dir.name).symlink_to(package_dir)
    return farm


def _generate(binary: str, spec: str, tree: pathlib.Path, toolchain: str, work: pathlib.Path):
    policy = work / "policy.toml"
    policy.write_text(
        "schema_version = 1\n"
        'name = "backtest"\n\n'
        "[toolchain]\n"
        'name = "GCCcore"\n'
        f'version = "{toolchain}"\n'
    )
    out = work / "out"
    proc = subprocess.run(
        [
            binary, "package", "plan",
            "--source", spec,
            "--format", "pypi",
            "--toolchain-name", "GCCcore",
            "--toolchain-version", toolchain,
            "--easyconfigs", str(tree),
            "--stack-policy", str(policy),
            "--out-dir", str(out),
        ],
        capture_output=True,
        text=True,
        timeout=900,
    )
    emitted = list(out.rglob("*.eb"))
    return proc, emitted[0] if emitted else None


def main() -> int:
    binary = os.environ.get("EB_STACK_BIN", "eb-stack")
    tree = pathlib.Path(os.environ["EB_EASYCONFIGS"])
    toolchain = os.environ.get("EB_GEN_TOOLCHAIN", "14.2.0")
    limit = int(os.environ.get("EB_GEN_LIMIT", "8"))

    corpus = []
    for path in sorted(tree.rglob(f"*-GCCcore-{toolchain}.eb")):
        text = path.read_text()
        if "easyblock = 'PythonPackage'" not in text:
            continue
        if "exts_list" in text or "patches" in text or "versionsuffix" in text:
            continue  # a shape this harness cannot state a right answer for yet
        corpus.append(path)
        if len(corpus) >= limit:
            break
    if not corpus:
        print(f"no PythonPackage recipes at GCCcore-{toolchain}")
        return 1

    agreement = {field: [0, 0] for field in FIELDS}
    with tempfile.TemporaryDirectory(prefix="eb-gen-backtest-") as raw:
        work = pathlib.Path(raw)
        for path in corpus:
            want = _read(path)
            if want is None:
                continue
            farm = _robot_without(tree, want["name"], work)
            package_work = work / want["name"]
            package_work.mkdir(exist_ok=True)
            proc, emitted = _generate(
                binary, f"{want['name']}=={want['version']}", farm, toolchain, package_work
            )
            if emitted is None:
                # A refusal is not a failure: eb-stack declines to emit an
                # overlay for something the tree already provides as an
                # extension of another module, which is the right answer.
                plan = package_work / "out" / "package.plan.json"
                provided = plan.is_file() and "already-provided" in plan.read_text()
                if provided:
                    print(f"{want['name']}-{want['version']}: the tree provides it already")
                    continue
                tail = (proc.stdout + proc.stderr).strip().splitlines()[-1:] or ["no output"]
                print(f"{want['name']}-{want['version']}: no recipe emitted: {tail[0][:110]}")
                continue
            got = _read(emitted)
            differences = []
            for field in FIELDS:
                agreement[field][1] += 1
                if got is not None and got[field] == want[field]:
                    agreement[field][0] += 1
                else:
                    differences.append(
                        f"{field}: got {got[field] if got else None!r}, upstream {want[field]!r}"
                    )
            status = "matches upstream" if not differences else "; ".join(differences)
            print(f"{want['name']}-{want['version']}: {status}")

    print("\nfield agreement")
    for field, (hit, total) in agreement.items():
        if total:
            print(f"  {field:20s} {hit}/{total}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
