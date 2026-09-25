#!/usr/bin/env python3
"""Emit a Spack packages.yaml that registers EESSI installations as externals.

Every EESSI install keeps the easyconfig it was built from under
<install>/easybuild/<Name>-<version>.eb. This reads those files with
EasyBuild's own parser, walks the runtime dependency closure of the modules
named on the command line, resolves each dependency to its install directory
through the toolchain hierarchy, and writes one Spack external per install:

- GCCcore becomes the gcc compiler external, with the compiler paths Spack
  needs in extra_attributes.
- Toolchain bundles (foss, gompi, gfbf, GCC) are not packages and are skipped;
  their members are registered through the modules that depend on them.
- Python extensions in exts_list (SciPy-bundle, Python-bundle-PyPI, Python
  itself) become py-* externals whose prefix is the bundle's prefix.
- Runtime dependencies (EasyBuild's dependencies, not builddependencies) are
  declared on each external with deptypes build, link and run.

When the closure holds one GCCcore version, the gcc entry carries
`require: '@<version>'`, so Spack compiles with EESSI's compiler and keeps the
CPU target the externals are declared for.

Compat-layer packages (zlib, bzip2, ncurses, openssl, ...) are filtered out of
EESSI's software layer and are not emitted here. Register them with
`spack external find --all -p $EESSI_EPREFIX` and the same for
$EESSI_EPREFIX/usr, as Spood's quick_start.sh does, but pass `--exclude` for
every package this script emitted. The compat layer carries its own gcc,
cmake and python, and without the exclusions Spack prefers them because they
are newer.

Usage, inside an environment where /cvmfs/software.eessi.io is mounted and
EasyBuild's Python package is importable:

  eessi-spack-externals.py \\
      --software /cvmfs/software.eessi.io/versions/2025.06/software/linux/x86_64/intel/haswell \\
      --target haswell --spack-packages <(spack list) \\
      Python/3.13.1-GCCcore-14.2.0 SciPy-bundle/2025.06-gfbf-2025a \\
      CMake/3.31.3-GCCcore-14.2.0 Ninja/1.12.1-GCCcore-14.2.0 \\
      > packages.yaml

Output is deterministic: packages sorted by Spack name, externals by version.
"""

from __future__ import annotations

import argparse
import re
import sys
from dataclasses import dataclass, field
from pathlib import Path

# EasyBuild name -> Spack name, where lowercasing is not enough.
SPACK_NAME = {
    "GCCcore": "gcc",
    "make": "gmake",
    "cURL": "curl",
    "OpenBLAS": "openblas",
    "FlexiBLAS": "flexiblas",
    "OpenMPI": "openmpi",
    "ScaLAPACK": "netlib-scalapack",
    "pybind11": "py-pybind11",
    "Eigen": "eigen",
    "Boost": "boost",
    "libxml2": "libxml2",
    "PyYAML": "py-pyyaml",
    "protobuf": "protobuf",
    "pkgconf": "pkgconf",
    "hwloc": "hwloc",
    "UCX": "ucx",
    "libfabric": "libfabric",
    "PMIx": "pmix",
    "OpenSSL": "openssl",
    "SQLite": "sqlite",
    "libffi": "libffi",
    "expat": "expat",
    "GMP": "gmp",
    "MPFR": "mpfr",
    "Tcl": "tcl",
    "Tk": "tk",
    "ICU": "icu4c",
}

# Toolchains and pure bundles of other modules: not Spack packages.
SKIP = {"GCC", "foss", "gompi", "gfbf", "gompic", "fosscuda", "lfoss", "lompi", "lfbf",
        "llvm-compilers", "binutils"}

# Bundles whose exts_list entries are Python packages.
PYTHON_EXTS = {"Python", "SciPy-bundle", "Python-bundle-PyPI", "Python-bundle", "hatchling",
               "poetry", "maturin", "meson-python", "scikit-build-core"}


@dataclass
class Install:
    name: str
    dirname: str          # <version><versionsuffix>[-<tc>-<tcver>]
    prefix: Path
    version: str
    toolchain: tuple[str, str]
    deps: list[tuple] = field(default_factory=list)
    exts: list[tuple[str, str]] = field(default_factory=list)
    easyblock: str = ""


def parse_eb(path: Path) -> dict:
    try:
        from easybuild.framework.easyconfig.parser import EasyConfigParser
    except ImportError as exc:
        raise SystemExit(f"EasyBuild is not importable ({exc}); run with EasyBuild's Python")
    return EasyConfigParser(str(path)).get_config_dict()


def spack_name(eb_name: str, easyblock: str) -> str:
    if eb_name in SPACK_NAME:
        return SPACK_NAME[eb_name]
    if easyblock in ("PythonPackage", "PythonBundle", "CargoPythonPackage", "CargoPythonBundle"):
        return "py-" + normalise(eb_name)
    return normalise(eb_name)


def normalise(name: str) -> str:
    return re.sub(r"[_.]+", "-", name).lower()


class Tree:
    """The software layer of one EESSI CPU target."""

    def __init__(self, software: Path):
        self.root = software / "software"
        if not self.root.is_dir():
            raise SystemExit(f"{self.root} is not a directory")
        self.cache: dict[tuple[str, str], Install] = {}

    def load(self, name: str, dirname: str) -> Install:
        key = (name, dirname)
        if key in self.cache:
            return self.cache[key]
        prefix = self.root / name / dirname
        ebfile = prefix / "easybuild" / f"{name}-{dirname}.eb"
        if not ebfile.is_file():
            raise SystemExit(f"no easyconfig at {ebfile}")
        cfg = parse_eb(ebfile)
        tc = cfg.get("toolchain") or {"name": "system", "version": "system"}
        inst = Install(
            name=name,
            dirname=dirname,
            prefix=prefix,
            version=str(cfg.get("version", "")),
            toolchain=(tc["name"], str(tc["version"])),
            easyblock=str(cfg.get("easyblock") or ""),
        )
        inst.deps = list(cfg.get("dependencies") or [])
        for ext in cfg.get("exts_list") or []:
            if isinstance(ext, (tuple, list)) and len(ext) >= 2:
                inst.exts.append((str(ext[0]), str(ext[1])))
        self.cache[key] = inst
        return inst

    def subtoolchains(self, tc: tuple[str, str]) -> list[str]:
        """Directory suffixes to try for a dependency of a module built with tc.

        Most specific first: the toolchain itself, the MPI and BLAS halves of
        foss, GCC, GCCcore, then no suffix (SYSTEM).
        """
        name, ver = tc
        if name.lower() == "system":
            return [""]
        out = [f"-{name}-{ver}"]
        gcc = self.gcc_version(tc)
        if name in ("foss", "gompi", "gfbf", "lfoss", "lompi", "lfbf"):
            out += [f"-{n}-{ver}" for n in ("gompi", "gfbf") if n != name]
        if gcc:
            out += [f"-GCC-{gcc}", f"-GCCcore-{gcc}"]
        out.append("")
        return out

    def gcc_version(self, tc: tuple[str, str]) -> str | None:
        name, ver = tc
        if name in ("GCC", "GCCcore"):
            return ver
        tdir = self.root / name / ver / "easybuild" / f"{name}-{ver}.eb"
        if not tdir.is_file():
            return None
        for dep in parse_eb(tdir).get("dependencies") or []:
            if dep[0] in ("GCC", "GCCcore"):
                return str(dep[1])
        return None

    def resolve(self, dep: tuple, parent_tc: tuple[str, str]) -> tuple[str, str] | None:
        name, version = str(dep[0]), str(dep[1])
        suffix = str(dep[2]) if len(dep) > 2 and dep[2] else ""
        if len(dep) > 3 and dep[3]:
            t = dep[3]
            if isinstance(t, dict):
                t = (t["name"], t["version"])
            candidates = [f"-{t[0]}-{t[1]}" if str(t[0]).lower() != "system" else ""]
        else:
            candidates = self.subtoolchains(parent_tc)
        base = self.root / name
        for tcs in candidates:
            dirname = f"{version}{suffix}{tcs}"
            if (base / dirname).is_dir():
                return name, dirname
        return None


def closure(tree: Tree, roots: list[str]) -> tuple[list[Install], list[str]]:
    seen: dict[tuple[str, str], Install] = {}
    missing: list[str] = []
    stack = []
    for r in roots:
        name, _, dirname = r.partition("/")
        stack.append((name, dirname))
    while stack:
        key = stack.pop()
        if key in seen:
            continue
        inst = tree.load(*key)
        seen[key] = inst
        for dep in inst.deps:
            found = tree.resolve(dep, inst.toolchain)
            if found is None:
                missing.append(f"{inst.name}/{inst.dirname} -> {dep[0]} {dep[1]} (filtered or compat)")
                continue
            stack.append(found)
        # The compiler of every non-SYSTEM install is a dependency too.
        gcc = tree.gcc_version(inst.toolchain)
        if gcc and (tree.root / "GCCcore" / gcc).is_dir():
            stack.append(("GCCcore", gcc))
    return list(seen.values()), missing


BUNDLE_EASYBLOCKS = ("Bundle", "PythonBundle", "CargoPythonBundle")


def emit(installs: list[Install], tree: Tree, target: str,
         known: set[str] | None = None, dropped: set[str] | None = None) -> str:
    """Render packages.yaml. With `known` (Spack's package names), names Spack
    does not package are left out, both as externals and as dependencies,
    and collected in `dropped`."""
    by_key = {(i.name, i.dirname): i for i in installs}
    entries: dict[str, list[dict]] = {}
    dropped = dropped if dropped is not None else set()

    def ok(sname: str) -> bool:
        if known is None or sname in known:
            return True
        dropped.add(sname)
        return False

    def gcc_of(inst: Install) -> str | None:
        return tree.gcc_version(inst.toolchain)

    python_of: dict[tuple[str, str], str] = {}
    for inst in installs:
        if inst.name == "Python":
            python_of[(inst.name, inst.dirname)] = inst.version

    def spec_of(inst: Install) -> str:
        return f"{spack_name(inst.name, inst.easyblock)}@{inst.version} target={target}"

    for inst in installs:
        if inst.name in SKIP:
            continue
        sname = spack_name(inst.name, inst.easyblock)
        # A bundle is a set of extensions, not a package: register what it
        # contains and not the bundle itself. Python is both.
        is_bundle = inst.easyblock in BUNDLE_EASYBLOCKS
        deps = []
        gcc = gcc_of(inst)
        if inst.name != "GCCcore" and gcc:
            deps.append({"spec": f"gcc@{gcc} target={target}"})
        py_version = None
        for dep in inst.deps:
            found = tree.resolve(dep, inst.toolchain)
            if not found or found[0] in SKIP:
                continue
            d = by_key[found]
            if d.name == "Python":
                py_version = d.version
            if d.easyblock in BUNDLE_EASYBLOCKS and d.name != "Python":
                # A bundle is not registered, so depend on what it contains.
                if d.name in PYTHON_EXTS:
                    for ext_name, ext_ver in d.exts:
                        ename = "py-" + normalise(ext_name)
                        if ok(ename):
                            deps.append({"spec": f"{ename}@{ext_ver} target={target}",
                                         "deptypes": ["build", "link", "run"]})
                continue
            if ok(spack_name(d.name, d.easyblock)):
                deps.append({"spec": spec_of(d), "deptypes": ["build", "link", "run"]})
        entry = {"spec": spec_of(inst), "prefix": str(inst.prefix), "dependencies": deps}
        if inst.name == "GCCcore":
            b = inst.prefix / "bin"
            entry["spec"] = f"gcc@{inst.version} languages:='c,c++,fortran' target={target}"
            entry["dependencies"] = []
            entry["extra_attributes"] = {"compilers": {
                "c": str(b / "gcc"), "cxx": str(b / "g++"), "fortran": str(b / "gfortran")}}
        if not is_bundle and ok(sname):
            entries.setdefault(sname, []).append(entry)

        if inst.name in PYTHON_EXTS and inst.exts:
            pyv = inst.version if inst.name == "Python" else py_version
            for ext_name, ext_ver in inst.exts:
                ename = "py-" + normalise(ext_name)
                if not ok(ename):
                    continue
                edeps = []
                if gcc:
                    edeps.append({"spec": f"gcc@{gcc} target={target}"})
                if pyv:
                    edeps.append({"spec": f"python@{pyv} target={target}",
                                  "deptypes": ["build", "link", "run"]})
                entries.setdefault(ename, []).append({
                    "spec": f"{ename}@{ext_ver} target={target}",
                    "prefix": str(inst.prefix),
                    "dependencies": edeps,
                })

    lines = ["# Generated by eessi-spack-externals.py from the easyconfigs EESSI keeps",
             "# with each install. Compat-layer packages are not included: add them",
             "# with spack external find over $EESSI_EPREFIX and $EESSI_EPREFIX/usr.",
             "packages:"]
    gcc_versions = sorted({i.version for i in installs if i.name == "GCCcore"})
    for sname in sorted(entries):
        # One external per (name, version, prefix); bundles can repeat a name.
        uniq = {}
        for e in entries[sname]:
            uniq.setdefault((e["spec"], e["prefix"]), e)
        lines.append(f"  {sname}:")
        if sname == "gcc" and len(gcc_versions) == 1:
            # Pin the compiler to EESSI's GCCcore: a compat-layer or host gcc
            # found by detection is older, cannot target the same CPU, and then
            # none of the externals below match.
            lines.append(f"    require: '@{gcc_versions[0]}'")
        lines.append("    externals:")
        for e in sorted(uniq.values(), key=lambda e: e["spec"]):
            lines.append(f"    - spec: {e['spec']}")
            lines.append(f"      prefix: {e['prefix']}")
            if e["dependencies"]:
                lines.append("      dependencies:")
                for d in e["dependencies"]:
                    lines.append(f"      - spec: {d['spec']}")
                    if "deptypes" in d:
                        lines.append("        deptypes: [" + ", ".join(d["deptypes"]) + "]")
            else:
                lines.append("      dependencies: []")
            if "extra_attributes" in e:
                lines.append("      extra_attributes:")
                lines.append("        compilers:")
                for k, v in e["extra_attributes"]["compilers"].items():
                    lines.append(f"          {k}: {v}")
    return "\n".join(lines) + "\n"


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--software", required=True, type=Path,
                    help="EESSI software prefix for one CPU target, e.g. .../software/linux/x86_64/intel/haswell")
    ap.add_argument("--target", required=True, help="Spack target name for these installs (spack arch -t)")
    ap.add_argument("--spack-packages", type=Path,
                    help="Output of `spack list`; names Spack does not package are left out")
    ap.add_argument("roots", nargs="+", help="Root modules as Name/version-toolchain")
    args = ap.parse_args(argv)

    known = None
    if args.spack_packages:
        known = {line.strip() for line in args.spack_packages.read_text().splitlines() if line.strip()}
    tree = Tree(args.software)
    installs, missing = closure(tree, args.roots)
    dropped: set[str] = set()
    sys.stdout.write(emit(installs, tree, args.target, known, dropped))
    for m in sorted(set(missing)):
        print(f"not in the software layer: {m}", file=sys.stderr)
    for name in sorted(dropped):
        print(f"no Spack package, left out: {name}", file=sys.stderr)
    print(f"{len(installs)} installs in the closure", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
