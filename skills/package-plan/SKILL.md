---
name: eb-stack-package-plan
description: Resolve a foreign recipe with Resolvo and emit the manifest, planned CycloneDX SBOM, profile lock, and EasyBuild recipe. Use when a robot tree and a stack policy are available. A plan can establish resolves and no later claim.
---

# Plan a package

Run this from the repository root. The fixture robot is enough. A full easybuild-easyconfigs tree is not required for this example.

```sh
eb-stack package plan \
  --source fixtures/foreign_ingest/pypi_bs4/pypi.json \
  --format pypi \
  --toolchain foss/2026.1 \
  --easyconfigs fixtures/foreign_ingest/pypi_bs4/robot \
  --stack-policy examples/stacks/eessi-python-extras.toml \
  --out-dir /tmp/bs4-plan
```

The command prints the paths it wrote. These files must exist:

```sh
test -s /tmp/bs4-plan/package.plan.json
test -s /tmp/bs4-plan/package.sbom.cdx.json
test -s /tmp/bs4-plan/locks/default.lock.json
test -s /tmp/bs4-plan/easyconfigs/b/beautifulsoup4/beautifulsoup4-4.12.3-foss-2026.1.eb
```

`--easyconfigs` and `--stack-policy` are required. Repeat `--easyconfigs` for an overlay. Later trees win on one name, version, toolchain, and versionsuffix. `--toolchain` is `name/version`. `--toolchain-version` alone defaults the name to `foss`. An empty `--toolchain-name` is rejected. `--source-checksum` is repeatable, once per source artifact, when the recipe has no digest.

The emitted recipe for this fixture uses `easyblock = 'PythonPackage'`. A plan establishes `resolves` only. It does not establish `builds` or `binary-verified`.
