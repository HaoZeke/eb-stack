"""The same properties, against the easyconfig tree as it used to be.

A recipe written in 2019 uses conventions that have since changed: toolchain
names, dependency spellings, template constants, whole easyblocks. Testing only
against HEAD asks whether the tool handles what upstream writes today, and a
site's robot path routinely holds recipes far older than that.

So the corpus here is history itself. Each commit is materialised once with
`git archive`, which is cheap and leaves the working checkout alone, and the
properties are the ones in test_build_order: a dependency precedes what needs
it, nothing is built twice, the answer is stable, and a root's declared
dependencies are all present.

    EB_EASYCONFIGS_GIT   an easybuild-easyconfigs clone with history
    EB_HISTORY_COMMITS   comma-separated commits (default: one per year)

A commit that predates a feature is not a failure of that feature. What these
catch is the opposite: a tool that reads today's spelling and quietly
mis-reads yesterday's.
"""

from __future__ import annotations

import os
import pathlib
import subprocess
import tarfile
import tempfile

import pytest
import reference
from conftest import run_order
from hypothesis import HealthCheck, assume, given, settings
from hypothesis import strategies as st
from test_build_order import _easyconfigs

# One commit per year, chosen by date rather than by tag so the set does not
# depend on how upstream happened to tag that year.
DEFAULT_COMMITS = "2019-07-01,2022-07-01,2024-07-01,2026-07-01"

SETTINGS = settings(
    max_examples=10,
    deadline=None,
    suppress_health_check=[HealthCheck.function_scoped_fixture, HealthCheck.too_slow],
)


def _repo() -> pathlib.Path:
    raw = os.environ.get("EB_EASYCONFIGS_GIT")
    if not raw:
        pytest.skip("EB_EASYCONFIGS_GIT is not set")
    path = pathlib.Path(raw).expanduser()
    if not (path / ".git").is_dir():
        pytest.skip(f"{path} is not a git checkout")
    return path


def _resolve(repo: pathlib.Path, spec: str) -> tuple[str, str]:
    """A commit for a spec, which may be a date or a revision."""
    if "-" in spec and spec.count("-") == 2 and spec.replace("-", "").isdigit():
        args = ["git", "log", f"--before={spec}", "-n1", "--format=%H %ad", "--date=short"]
    else:
        args = ["git", "log", "-n1", "--format=%H %ad", "--date=short", spec]
    out = subprocess.run(args, cwd=repo, capture_output=True, text=True).stdout.strip()
    if not out:
        pytest.skip(f"no commit for {spec}")
    sha, date = out.split(maxsplit=1)
    return sha, date


@pytest.fixture(scope="session")
def history_repo() -> pathlib.Path:
    return _repo()


def _materialize(repo: pathlib.Path, sha: str, into: pathlib.Path) -> pathlib.Path | None:
    """The easyconfigs directory at one commit, extracted into `into`.

    `git archive` writes a tarball of one subtree without touching the working
    checkout, which matters because the checkout is someone's actual clone.
    """
    target = into / sha[:12]
    if target.is_dir():
        return target
    archive = into / f"{sha[:12]}.tar"
    result = subprocess.run(
        ["git", "archive", "--format=tar", "-o", str(archive), sha, "easybuild/easyconfigs"],
        cwd=repo,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        return None
    target.mkdir(parents=True, exist_ok=True)
    with tarfile.open(archive) as tar:
        tar.extractall(target, filter="data")
    archive.unlink(missing_ok=True)
    tree = target / "easybuild" / "easyconfigs"
    return tree if tree.is_dir() else None


@pytest.fixture(scope="session")
def historical_trees(history_repo) -> list[tuple[str, str, pathlib.Path]]:
    """Every requested commit, materialised, as (sha, date, tree)."""
    specs = os.environ.get("EB_HISTORY_COMMITS", DEFAULT_COMMITS).split(",")
    workdir = pathlib.Path(tempfile.mkdtemp(prefix="eb-stack-history-"))
    trees = []
    for spec in specs:
        sha, date = _resolve(history_repo, spec.strip())
        tree = _materialize(history_repo, sha, workdir)
        if tree is not None:
            trees.append((sha, date, tree))
    if not trees:
        pytest.skip("no historical trees could be materialised")
    return trees


@pytest.fixture(scope="session")
def historical_roots(historical_trees) -> dict:
    """Readable roots per tree, and the recipes behind them."""
    index = {}
    for sha, _date, tree in historical_trees:
        recipes = {}
        for path in sorted(_easyconfigs(tree)):
            recipe = reference.read(path)
            if recipe is not None:
                recipes[str(path)] = recipe
        roots = sorted({f"{r.name}=={r.version}" for r in recipes.values()})
        index[sha] = (roots, recipes)
    return index


@SETTINGS
@given(data=st.data())
def test_old_trees_still_order_dependencies_first(
    eb_stack, historical_trees, historical_roots, tmp_path, data
):
    sha, date, tree = data.draw(
        st.sampled_from(historical_trees), label="commit"
    )
    roots, recipes = historical_roots[sha]
    assume(roots)
    root = data.draw(st.sampled_from(roots), label="root")

    code, output, lines = run_order(eb_stack, tree, root, tmp_path / "order.txt")
    assume(code == 0)
    assert lines, f"{date} {sha[:12]}: empty order for {root}\n{output}"

    position = {path: index for index, path in enumerate(lines)}
    for path, index in position.items():
        recipe = recipes.get(path)
        if recipe is None:
            continue
        for dep_name, dep_version, _suffix, _tc in (
            recipe.dependencies + recipe.builddependencies
        ):
            # Satisfied when some build of it was built earlier: several
            # builds of one name can be present, and any earlier one will do.
            matching = [
                other_index
                for other_path, other_index in position.items()
                if (other := recipes.get(other_path)) is not None
                and other.name == dep_name
                and (not dep_version or other.version == dep_version)
            ]
            if matching and not any(other < index for other in matching):
                pytest.fail(
                    f"{date} {sha[:12]}: {recipe.module} at {index} needs {dep_name} "
                    f"{dep_version}, built only at {sorted(matching)}\nroot: {root}"
                )


@SETTINGS
@given(data=st.data())
def test_old_trees_do_not_lose_a_declared_dependency(
    eb_stack, historical_trees, historical_roots, tmp_path, data
):
    sha, date, tree = data.draw(st.sampled_from(historical_trees), label="commit")
    roots, recipes = historical_roots[sha]
    assume(roots)
    root = data.draw(st.sampled_from(roots), label="root")

    code, _output, lines = run_order(eb_stack, tree, root, tmp_path / "order.txt")
    assume(code == 0)

    present = {}
    for path in lines:
        recipe = recipes.get(path)
        if recipe is not None:
            present.setdefault(recipe.name, set()).add(recipe.version)

    name, _, version = root.partition("==")
    chosen = next(
        (
            recipes[path]
            for path in lines
            if (r := recipes.get(path)) is not None
            and r.name == name
            and r.version == version
        ),
        None,
    )
    assume(chosen is not None)
    for dep_name, _dep_version, _suffix, _tc in (
        chosen.dependencies + chosen.builddependencies
    ):
        assert dep_name in present, (
            f"{date} {sha[:12]}: {chosen.module} declares {dep_name}, "
            f"which the order omits\nroot: {root}"
        )
