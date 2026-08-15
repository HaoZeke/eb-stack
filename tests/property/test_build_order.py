"""Backtests: real easyconfigs in, properties checked, shrinking on failure.

The corpus is an easybuild-easyconfigs checkout, so every example is a recipe
someone actually wrote and upstream actually ships. Hypothesis picks which one,
remembers the ones that failed, and shrinks toward the smallest root that still
fails.

Four properties, each stated as a thing that must hold of any correct answer:

1. a dependency is built before whatever needs it
2. nothing is built twice
3. the same question gets the same answer
4. a root's own dependencies are all present in the order

None of them require EasyBuild to be installed, and none compare against a
recorded snapshot: they compare against what the recipes themselves say, read
by an independent implementation in reference.py.
"""

from __future__ import annotations

import os
import pathlib

import pytest
import reference
from conftest import run_order
from hypothesis import HealthCheck, assume, given, settings
from hypothesis import strategies as st

# Each example runs the binary over a whole tree, so examples are expensive and
# deliberately few. The corpus supplies the variety that a large example count
# would otherwise have to invent.
SETTINGS = settings(
    max_examples=25,
    deadline=None,
    suppress_health_check=[HealthCheck.function_scoped_fixture, HealthCheck.too_slow],
)


def _easyconfigs(tree: pathlib.Path):
    """Every .eb under a tree, following symlinked directories.

    A corpus assembled by symlinking a few letter directories is an ordinary
    way to point these tests at part of a checkout, and pathlib's rglob walks
    straight past it.
    """
    for directory, _subdirs, files in os.walk(tree, followlinks=True):
        for name in sorted(files):
            if name.endswith(".eb"):
                yield pathlib.Path(directory) / name


def _roots(tree: pathlib.Path) -> list[str]:
    """Every recipe in the tree, as `name==version`, read independently."""
    roots = []
    for path in sorted(_easyconfigs(tree)):
        recipe = reference.read(path)
        if recipe is not None:
            roots.append(f"{recipe.name}=={recipe.version}")
    return sorted(set(roots))


@pytest.fixture(scope="session")
def roots(easyconfigs) -> list[str]:
    found = _roots(easyconfigs)
    if not found:
        pytest.skip("no readable easyconfigs in the tree")
    return found


@pytest.fixture(scope="session")
def by_module(easyconfigs) -> dict:
    """Every readable recipe, keyed the way eb-stack prints a module."""
    index = {}
    for path in sorted(_easyconfigs(easyconfigs)):
        recipe = reference.read(path)
        if recipe is not None:
            index[str(path)] = recipe
    return index


@SETTINGS
@given(data=st.data())
def test_a_dependency_is_built_before_what_needs_it(
    eb_stack, easyconfigs, roots, by_module, tmp_path, data
):
    root = data.draw(st.sampled_from(roots), label="root")
    out = tmp_path / "order.txt"
    code, output, lines = run_order(eb_stack, easyconfigs, root, out)
    # Rejected, not passed. An early return would let a corpus that orders
    # nothing report four green properties, which is exactly the failure this
    # harness exists to avoid; assume() makes Hypothesis count the rejection
    # and complain when too many examples are filtered away.
    assume(code == 0)
    assert lines, f"a successful order with no builds: {root}\n{output}"

    position = {path: index for index, path in enumerate(lines)}
    for path, index in position.items():
        recipe = by_module.get(path)
        if recipe is None:
            continue
        for dep_name, dep_version, _suffix, _tc in (
            recipe.dependencies + recipe.builddependencies
        ):
            # Only dependencies the order itself contains can be checked here:
            # whether the order is complete is property four.
            for other_path, other_index in position.items():
                other = by_module.get(other_path)
                if other is None or other.name != dep_name:
                    continue
                if dep_version and other.version != dep_version:
                    continue
                assert other_index < index, (
                    f"{recipe.module} at {index} needs {other.module} at {other_index}"
                    f"\nroot: {root}"
                )
                break


@SETTINGS
@given(data=st.data())
def test_nothing_is_built_twice(eb_stack, easyconfigs, roots, tmp_path, data):
    root = data.draw(st.sampled_from(roots), label="root")
    code, _output, lines = run_order(eb_stack, easyconfigs, root, tmp_path / "order.txt")
    assume(code == 0)
    assert len(lines) == len(set(lines)), f"repeated builds for {root}"


@SETTINGS
@given(data=st.data())
def test_the_same_question_gets_the_same_answer(
    eb_stack, easyconfigs, roots, tmp_path, data
):
    root = data.draw(st.sampled_from(roots), label="root")
    first = run_order(eb_stack, easyconfigs, root, tmp_path / "first.txt")
    second = run_order(eb_stack, easyconfigs, root, tmp_path / "second.txt")
    assert first[0] == second[0], f"exit code moved for {root}"
    assert first[2] == second[2], f"order moved for {root}"


@SETTINGS
@given(data=st.data())
def test_every_declared_dependency_of_the_root_is_in_the_order(
    eb_stack, easyconfigs, roots, by_module, tmp_path, data
):
    root = data.draw(st.sampled_from(roots), label="root")
    out = tmp_path / "order.txt"
    code, _output, lines = run_order(eb_stack, easyconfigs, root, out)
    assume(code == 0)

    present = {}
    for path in lines:
        recipe = by_module.get(path)
        if recipe is not None:
            present.setdefault(recipe.name, set()).add(recipe.version)

    name, _, version = root.partition("==")
    root_recipes = [
        r for r in by_module.values() if r.name == name and r.version == version
    ]
    if not root_recipes:
        return
    # Any build of the root will do: the order chose one of them.
    chosen = next(
        (r for r in root_recipes if r.name in present and r.version in present[r.name]),
        None,
    )
    if chosen is None:
        return
    for dep_name, _dep_version, _suffix, _tc in (
        chosen.dependencies + chosen.builddependencies
    ):
        assert dep_name in present, (
            f"{chosen.module} declares {dep_name}, which the order omits\nroot: {root}"
        )
