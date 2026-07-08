#!/usr/bin/env python3
"""Resolve easyconfig fields with EasyBuild's parser (fixture oracle).

Uses EasyConfigParser (no modules tool / Lmod required) plus the same class of
template substitutions EasyBuild applies for name/version/toolchain-derived
%(…)s keys. Not part of `cargo test`; run manually to refresh golden JSON:

  source ~/.venvs/easybuild/bin/activate
  python scripts/resolve_easyconfig_eb.py \\
      fixtures/parser_hardcases/easyconfigs/*.eb \\
      -o fixtures/parser_hardcases/resolved/

Requires EasyBuild (framework) importable, e.g. from ~/.venvs/easybuild.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path


def _require_eb():
    try:
        from easybuild.framework.easyconfig.parser import EasyConfigParser
        from easybuild.framework.easyconfig.templates import TEMPLATE_CONSTANTS
    except ImportError as exc:  # pragma: no cover
        raise SystemExit(
            "EasyBuild not importable. Activate ~/.venvs/easybuild (or install "
            f"easybuild-framework). Import error: {exc}"
        ) from exc
    return EasyConfigParser, TEMPLATE_CONSTANTS


def version_part_templates(version: str) -> dict[str, str]:
    parts = version.split(".")
    out: dict[str, str] = {}
    if parts and parts[0] != "":
        out["version_major"] = parts[0]
    if len(parts) > 1:
        out["version_minor"] = parts[1]
        out["version_major_minor"] = ".".join(parts[:2])
    if len(parts) > 2:
        out["version_patch"] = parts[2]
        out["version_minor_patch"] = ".".join(parts[1:3])
        out["version_major_minor_patch"] = ".".join(parts[:3])
    return out


def build_templates(cfg: dict, template_constants) -> dict[str, str]:
    tv: dict[str, str] = {}
    if cfg.get("name") is not None:
        tv["name"] = str(cfg["name"])
        if tv["name"]:
            tv["nameletter"] = tv["name"][0]
    if cfg.get("version") is not None:
        tv["version"] = str(cfg["version"])
        tv.update(version_part_templates(tv["version"]))
    vs = cfg.get("versionsuffix")
    tv["versionsuffix"] = "" if vs is None else str(vs)
    tc = cfg.get("toolchain") or {}
    if isinstance(tc, dict):
        if tc.get("name") is not None:
            tv["toolchain_name"] = str(tc["name"])
        if tc.get("version") is not None:
            tv["toolchain_version"] = str(tc["version"])
    for cst in template_constants:
        if isinstance(cst, (list, tuple)) and len(cst) >= 2:
            tv[str(cst[0])] = str(cst[1])
    return tv


_TMPL_RE = re.compile(r"%\(([^)]+)\)s")


def resolve_templates(val, templates: dict[str, str]):
    if isinstance(val, str):

        def repl(m: re.Match[str]) -> str:
            key = m.group(1)
            return templates[key] if key in templates else m.group(0)

        cur = val
        for _ in range(8):
            nxt = _TMPL_RE.sub(repl, cur)
            if nxt == cur:
                break
            cur = nxt
        return cur
    if isinstance(val, list):
        return [resolve_templates(x, templates) for x in val]
    if isinstance(val, tuple):
        return [resolve_templates(x, templates) for x in val]
    if isinstance(val, dict):
        return {str(k): resolve_templates(v, templates) for k, v in val.items()}
    return val


def toolchain_to_obj(tc) -> dict[str, str] | None:
    if tc is None:
        return None
    if isinstance(tc, dict):
        return {"name": str(tc["name"]), "version": str(tc["version"])}
    if isinstance(tc, (list, tuple)) and len(tc) >= 2:
        return {"name": str(tc[0]), "version": str(tc[1])}
    raise TypeError(f"unsupported toolchain value: {tc!r}")


def dep_tuple_to_obj(dep) -> dict:
    """Map an EasyBuild dependency tuple/list to a resolved dep object."""
    if isinstance(dep, str):
        # Filename form is out of scope for hardcase goldens; keep raw.
        return {"name": dep, "version": "", "versionsuffix": None, "toolchain": None}
    if not isinstance(dep, (list, tuple)) or len(dep) < 2:
        raise TypeError(f"unsupported dependency entry: {dep!r}")
    name = str(dep[0])
    version = str(dep[1])
    versionsuffix = None
    toolchain = None
    if len(dep) >= 3:
        versionsuffix = str(dep[2])
    if len(dep) >= 4:
        toolchain = toolchain_to_obj(dep[3])
    return {
        "name": name,
        "version": version,
        "versionsuffix": versionsuffix,
        "toolchain": toolchain,
    }


def ext_to_obj(ext) -> dict:
    if isinstance(ext, str):
        return {"name": ext, "version": ""}
    if not isinstance(ext, (list, tuple)) or len(ext) < 2:
        raise TypeError(f"unsupported exts_list entry: {ext!r}")
    return {"name": str(ext[0]), "version": str(ext[1])}


def resolve_easyconfig_text(text: str) -> dict:
    EasyConfigParser, TEMPLATE_CONSTANTS = _require_eb()
    cfg = EasyConfigParser(rawcontent=text).get_config_dict(validate=False)
    templates = build_templates(cfg, TEMPLATE_CONSTANTS)
    fields = {}
    for key in (
        "name",
        "version",
        "versionsuffix",
        "toolchain",
        "dependencies",
        "builddependencies",
        "exts_list",
    ):
        if key in cfg and cfg[key] is not None:
            fields[key] = resolve_templates(cfg[key], templates)
        else:
            fields[key] = None if key in ("versionsuffix",) else (
                [] if key in ("dependencies", "builddependencies", "exts_list") else None
            )

    tc = toolchain_to_obj(fields["toolchain"])
    if tc is None:
        raise SystemExit("resolved easyconfig missing toolchain")

    return {
        "name": str(fields["name"]),
        "version": str(fields["version"]),
        "versionsuffix": (
            None
            if fields["versionsuffix"] is None
            else str(fields["versionsuffix"])
        ),
        "toolchain": tc,
        "dependencies": [dep_tuple_to_obj(d) for d in (fields["dependencies"] or [])],
        "builddependencies": [
            dep_tuple_to_obj(d) for d in (fields["builddependencies"] or [])
        ],
        "exts_list": [ext_to_obj(e) for e in (fields["exts_list"] or [])],
    }


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("easyconfigs", nargs="+", type=Path, help=".eb files to resolve")
    ap.add_argument(
        "-o",
        "--output-dir",
        type=Path,
        help="Write <stem>.resolved.json files here (default: stdout one object)",
    )
    args = ap.parse_args(argv)

    results = []
    for path in args.easyconfigs:
        text = path.read_text(encoding="utf-8")
        resolved = resolve_easyconfig_text(text)
        resolved["source_easyconfig"] = path.name
        results.append(resolved)
        if args.output_dir:
            args.output_dir.mkdir(parents=True, exist_ok=True)
            out = args.output_dir / f"{path.stem}.resolved.json"
            out.write_text(json.dumps(resolved, indent=2) + "\n", encoding="utf-8")
            print(f"wrote {out}", file=sys.stderr)

    if not args.output_dir:
        json.dump(results if len(results) != 1 else results[0], sys.stdout, indent=2)
        sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
