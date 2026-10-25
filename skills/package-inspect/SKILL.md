---
name: eb-stack-package-inspect
description: Parse a conda-forge, Spack, PyPI, CRAN, Cargo, LuaRocks, or Raku recipe into package.plan.json and a planned CycloneDX SBOM. Use before solving or emitting EasyBuild recipes. Inspection establishes no claim-ladder rung.
---

# Inspect a foreign recipe

Run this from the repository root. The command writes two files and does not solve dependencies or emit a recipe.

```sh
eb-stack package inspect \
  --source fixtures/foreign_ingest/conda_eon/recipe.yaml \
  --format conda-forge \
  --toolchain foss/2026.1 \
  --package-config examples/packages/common.toml \
  --package-config examples/packages/eon.toml \
  --out-dir /tmp/eon-inspect
```

Check the outputs:

```sh
test -s /tmp/eon-inspect/package.plan.json
test -s /tmp/eon-inspect/package.sbom.cdx.json
python3 -m json.tool /tmp/eon-inspect/package.plan.json >/dev/null
python3 -m json.tool /tmp/eon-inspect/package.sbom.cdx.json >/dev/null
```

`--format` accepts `auto`, `conda-forge`, `spack`, `pypi`, `cran`, `cargo`, `luarocks`, and `raku`. `--toolchain` is `name/version`, or `system` for the SYSTEM toolchain. `--toolchain-version` alone defaults the name to `foss`. An empty `--toolchain-name` is rejected. Repeat `--package-config` for each TOML layer. Later layers override earlier ones.

Exit status is 0 when both files are written, 1 when a file cannot be read or parsed, and 2 when the command line is rejected.
