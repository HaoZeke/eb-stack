"""Shared fixtures for the property backtests.

These tests drive the built binary against a real easyconfig tree, so they need
two things from the environment and say so plainly when either is missing:

    EB_STACK_BIN    the binary under test (default: target/release/eb-stack)
    EB_EASYCONFIGS  an easybuild-easyconfigs checkout to read

Nothing here fabricates an easyconfig. The corpus is what upstream actually
ships, which is the point: a generated tree exercises the shapes someone
imagined, and a real one exercises the shapes that exist.
"""

from __future__ import annotations

import os
import pathlib
import subprocess

import pytest


def _env_path(name: str, default: pathlib.Path | None = None) -> pathlib.Path:
    raw = os.environ.get(name)
    if raw:
        return pathlib.Path(raw).expanduser()
    if default is not None:
        return default
    pytest.skip(f"{name} is not set")


@pytest.fixture(scope="session")
def eb_stack() -> pathlib.Path:
    repo = pathlib.Path(__file__).resolve().parents[2]
    binary = _env_path("EB_STACK_BIN", repo / "target" / "release" / "eb-stack")
    if not binary.is_file():
        pytest.skip(f"no binary at {binary}; cargo build --release first")
    return binary


@pytest.fixture(scope="session")
def easyconfigs() -> pathlib.Path:
    tree = _env_path("EB_EASYCONFIGS")
    if not tree.is_dir():
        pytest.skip(f"no easyconfig tree at {tree}")
    return tree


def run_order(binary: pathlib.Path, tree: pathlib.Path, root: str, out: pathlib.Path):
    """One `stack order` run. Returns (exit code, stdout+stderr, order lines)."""
    proc = subprocess.run(
        [
            str(binary),
            "stack",
            "order",
            "--easyconfigs",
            str(tree),
            "--root",
            root,
            "--out",
            str(out),
        ],
        capture_output=True,
        text=True,
        timeout=600,
    )
    lines = []
    if out.is_file():
        lines = [line for line in out.read_text().splitlines() if line.strip()]
    return proc.returncode, proc.stdout + proc.stderr, lines
