#!/usr/bin/env python3
"""CycloneDX 1.5 JSON → Graphviz.

Reads a planned SBOM from `eb-stack stack sbom` and draws the
`dependencies` graph. Build-time edges in
`eb_stack:buildDependsOn` are dashed. Node labels are name@version
plus the EasyBuild toolchain property when present.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path


def ident(s: str) -> str:
    return re.sub(r"[^A-Za-z0-9_]", "_", s)


def short_ref(ref: str) -> tuple[str, str]:
    # pkg:generic/Name@Ver?toolchain=foss-2025a
    name, ver = ref, ""
    if "pkg:generic/" in ref:
        rest = ref.split("pkg:generic/", 1)[1]
        namever, _, _ = rest.partition("?")
        if "@" in namever:
            name, ver = namever.rsplit("@", 1)
        else:
            name = namever
    return name, ver


def props(comp: dict) -> dict[str, str]:
    out = {}
    for item in comp.get("properties") or []:
        name = item.get("name") or item.get("key")
        val = item.get("value")
        if name and val is not None:
            out[str(name)] = str(val)
    return out


def to_dot(bom: dict) -> str:
    comps = {c.get("bom-ref") or c.get("bomRef") or "": c for c in bom.get("components") or []}
    # also metadata component
    meta = (bom.get("metadata") or {}).get("component") or {}
    mref = meta.get("bom-ref") or meta.get("bomRef")
    if mref:
        comps.setdefault(mref, meta)

    lines = [
        "digraph sbom {",
        '  graph [rankdir=LR, bgcolor="transparent", pad="0.25",',
        '         nodesep="0.4", ranksep="0.7", fontname="Inter,Helvetica,sans-serif"];',
        '  node [shape=box, style="rounded,filled", fillcolor="#f7f6f3",',
        '        color="#1a1917", fontname="Inter,Helvetica,sans-serif",',
        '        fontsize=11, penwidth=1.2];',
        '  edge [color="#1a1917", arrowsize=0.7, penwidth=1.1];',
    ]
    seen = set()
    for ref, comp in comps.items():
        if not ref:
            continue
        name, ver = short_ref(ref)
        name = comp.get("name") or name
        ver = comp.get("version") or ver
        p = props(comp)
        tc = p.get("easybuild:toolchain") or ""
        vs = p.get("easybuild:versionsuffix") or ""
        label = name
        if ver:
            label += f"\\n{ver}"
        if vs:
            label += vs
        if tc:
            label += f"\\n{tc}"
        nid = ident(ref)
        seen.add(ref)
        lines.append(f'  {nid} [label="{label}"];')

    for dep in bom.get("dependencies") or []:
        src = dep.get("ref")
        if not src:
            continue
        if src not in seen:
            n, v = short_ref(src)
            lines.append(f'  {ident(src)} [label="{n}\\n{v}"];')
            seen.add(src)
        for dst in dep.get("dependsOn") or []:
            if dst not in seen:
                n, v = short_ref(dst)
                lines.append(f'  {ident(dst)} [label="{n}\\n{v}"];')
                seen.add(dst)
            lines.append(f"  {ident(src)} -> {ident(dst)};")

    lines.append("}")
    return "\n".join(lines) + "\n"


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("sbom", type=Path)
    ap.add_argument("-o", "--out", type=Path)
    args = ap.parse_args()
    bom = json.loads(args.sbom.read_text(encoding="utf-8"))
    if bom.get("bomFormat") != "CycloneDX":
        print("not a CycloneDX document", file=sys.stderr)
        return 2
    dot = to_dot(bom)
    if args.out:
        args.out.write_text(dot, encoding="utf-8")
    else:
        sys.stdout.write(dot)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
