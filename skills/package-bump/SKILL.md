---
name: eb-stack-package-bump
description: Retarget an existing EasyBuild recipe onto another toolchain and write the same bundle as package plan. Use for a version or toolchain bump of a recipe the tree already has. A bump can establish resolves and no later claim.
---

# Bump an EasyBuild recipe

Run this from the repository root.

```sh
eb-stack package bump \
  --source tests/repro_fixtures/gromacs/GROMACS-2024.4-foss-2023b.eb \
  --toolchain foss/2024a \
  --easyconfigs tests/repro_fixtures/universe_foss_2024a \
  --out-dir /tmp/gromacs-2024a
```

The recipe, manifest, SBOM, and lock:

```sh
test -s /tmp/gromacs-2024a/package.plan.json
test -s /tmp/gromacs-2024a/package.sbom.cdx.json
test -s /tmp/gromacs-2024a/locks/default.lock.json
test -s /tmp/gromacs-2024a/easyconfigs/g/GROMACS/GROMACS-2024.4-foss-2024a.eb
grep -q "toolchain = {'name': 'foss', 'version': '2024a'}" \
  /tmp/gromacs-2024a/easyconfigs/g/GROMACS/GROMACS-2024.4-foss-2024a.eb
```

`--toolchain` is required as `name/version`. A bump does not default the name to `foss`. `--version` alone exits 2, and `--toolchain-version` alone exits 2. `--toolchain system` names the SYSTEM toolchain. On this GROMACS fixture that value is accepted and the solve then fails, because the universe has no SYSTEM candidate for Python. `--version` changes the package version when a toolchain is also set. `--dep NAME=VERSION` adds a locked pin for a dependency the recipe already has. `--fetch-checksums` downloads sources on a version bump when `--source-checksum` is absent. Omit it unless a network fetch is intended.

A bump establishes `resolves` only.
