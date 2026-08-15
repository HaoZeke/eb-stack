"""An independent reading of an easyconfig, to check the Rust one against.

An easyconfig is Python, and EasyBuild reads one by executing it. So does this,
in a namespace holding the constants a recipe may mention. Deliberately not a
second regex parser: two parsers written the same way share their blind spots,
and the whole value of a differential test is that the two sides fail
differently.

What is returned is only what the properties need: identity and the
dependencies a recipe declares. A file that cannot be executed returns None,
which the tests treat as "no opinion" rather than as agreement.
"""

from __future__ import annotations

import pathlib
from dataclasses import dataclass, field

SYSTEM = {"name": "system", "version": "system"}


class _Anything:
    """Stands in for whatever a recipe imports or calls.

    Recipes reach for helpers this reader does not have. Answering every
    attribute and call with another one of these keeps execution going far
    enough to reach the assignments that matter, and anything genuinely
    unreadable still raises and is skipped.
    """

    def __getattr__(self, _name):
        return _Anything()

    def __call__(self, *_args, **_kwargs):
        return _Anything()

    def __getitem__(self, _key):
        return _Anything()

    def __iter__(self):
        return iter(())

    def __str__(self):
        return ""

    def __repr__(self):
        return "<unknown>"

    # Recipes build strings out of constants, so the placeholder has to survive
    # concatenation, formatting and joining without raising.
    def __add__(self, _other):
        return _Anything()

    def __radd__(self, _other):
        return _Anything()

    def __mod__(self, _other):
        return _Anything()

    def __rmod__(self, _other):
        return _Anything()

    def __len__(self):
        return 0

    def __bool__(self):
        return True

    def __eq__(self, _other):
        return False

    def __hash__(self):
        return 0


class _Namespace(dict):
    """Globals in which an unknown name is a placeholder rather than an error.

    EasyBuild defines dozens of constants a recipe may use, and enumerating
    them here would be a second copy of a list that changes upstream. Anything
    not defined resolves to something inert instead, so execution reaches the
    assignments the tests care about. Real builtins still resolve normally.
    """

    def __missing__(self, _key):
        return _Anything()


@dataclass
class Recipe:
    """What one easyconfig says about itself."""

    name: str
    version: str
    toolchain_name: str
    toolchain_version: str
    versionsuffix: str = ""
    dependencies: list = field(default_factory=list)
    builddependencies: list = field(default_factory=list)

    @property
    def toolchain_label(self) -> str:
        if self.toolchain_name.lower() == "system":
            return "system"
        return f"{self.toolchain_name}-{self.toolchain_version}"

    @property
    def module(self) -> str:
        """The identity eb-stack prints, so the two can be compared directly."""
        return f"{self.name}-{self.version}-{self.toolchain_label}{self.versionsuffix}"


def _namespace() -> dict:
    """The constants a recipe may mention, with templates left as text."""
    ns = _Namespace({
        "SYSTEM": SYSTEM,
        "OS_PKG_OPENSSL_DEV": "openssl-dev",
        "ARCH": "x86_64",
        "OS_NAME": "linux",
        "OS_VERSION": "0",
    })
    for constant in (
        "SOURCE_TAR_GZ",
        "SOURCELOWER_TAR_GZ",
        "SOURCE_TAR_XZ",
        "SOURCELOWER_TAR_XZ",
        "SOURCE_TAR_BZ2",
        "SOURCELOWER_TAR_BZ2",
        "SOURCE_ZIP",
        "SOURCELOWER_ZIP",
        "SOURCE_TGZ",
        "SOURCELOWER_TGZ",
        "SOURCE_WHL",
        "SOURCELOWER_WHL",
        "GITHUB_SOURCE",
        "GITHUB_LOWER_SOURCE",
        "PYPI_SOURCE",
        "PYPI_LOWER_SOURCE",
        "SOURCEFORGE_SOURCE",
    ):
        ns[constant] = f"%({constant})s"
    return ns


def _dep_tuple(entry) -> tuple | None:
    """One dependency, as (name, version, versionsuffix, toolchain-label|None)."""
    if isinstance(entry, str):
        return (entry, "", "", None)
    if isinstance(entry, dict):
        return None  # dict-form deps carry no single identity to compare
    if not isinstance(entry, (tuple, list)) or not entry:
        return None
    name = entry[0]
    if not isinstance(name, str):
        return None
    version = entry[1] if len(entry) > 1 and isinstance(entry[1], str) else ""
    suffix = entry[2] if len(entry) > 2 and isinstance(entry[2], str) else ""
    toolchain = None
    if len(entry) > 3:
        tc = entry[3]
        if isinstance(tc, dict):
            toolchain = "system" if tc.get("name", "").lower() == "system" else None
        elif isinstance(tc, (tuple, list)) and len(tc) >= 2:
            toolchain = f"{tc[0]}-{tc[1]}"
        elif tc is True:
            toolchain = "system"
    return (name, version, suffix, toolchain)


def read(path: pathlib.Path) -> Recipe | None:
    """Read one easyconfig, or None when it cannot be read at all."""
    try:
        source = path.read_text()
    except OSError:
        return None

    ns = _namespace()
    try:
        exec(compile(source, str(path), "exec"), ns)  # noqa: S102
    except Exception:
        return None

    name, version = ns.get("name"), ns.get("version")
    toolchain = ns.get("toolchain")
    if not isinstance(name, str) or not isinstance(version, str):
        return None
    if not isinstance(toolchain, dict):
        return None
    tc_name = toolchain.get("name")
    tc_version = toolchain.get("version")
    if not isinstance(tc_name, str) or not isinstance(tc_version, str):
        return None

    suffix = ns.get("versionsuffix", "")
    if not isinstance(suffix, str):
        suffix = ""

    def deps(key):
        raw = ns.get(key, [])
        if not isinstance(raw, (list, tuple)):
            return []
        out = []
        for entry in raw:
            parsed = _dep_tuple(entry)
            if parsed is not None:
                out.append(parsed)
        return out

    return Recipe(
        name=name,
        version=version,
        toolchain_name=tc_name,
        toolchain_version=tc_version,
        versionsuffix=suffix,
        dependencies=deps("dependencies"),
        builddependencies=deps("builddependencies"),
    )
