---
name: eb-stack-annual-bump
description: Retarget an existing EasyBuild easyconfig to a new toolchain or application version using a canonical EasyBuild-origin manifest, CycloneDX SBOM, Resolvo dependency lock, stack-policy fallback, deterministic recipe emission, and the persisted Hermes/OMP remote build campaign. Use for annual stack rebuilds and package version bumps.
---

# Bump an EasyBuild package

Use this skill only when an EasyBuild recipe already exists. Use `skills/new-package/SKILL.md` for conda-forge or Spack imports.

## Mechanical contract

`package bump` parses the source EasyBuild recipe into the same canonical model used by new packages, retargets its toolchain or application version, solves its dependencies with Resolvo, and writes a bundle containing:

- `package.plan.json` with `origin = easy-build`;
- `package.sbom.cdx.json`;
- `locks/default.lock.json`;
- the retargeted `.eb` file under conventional letter/name layout.

The source recipe provides dependency floors. The target robot trees provide candidates. Resolvo chooses one hierarchy-compatible closure. Do not bypass this by manually copying target dependency versions into a standalone output file.

## Run a toolchain bump

```sh
eb-stack package bump \
  --source GROMACS-2024.4-foss-2023b.eb \
  --toolchain-name foss \
  --toolchain-version 2024a \
  --easyconfigs /path/to/easybuild-easyconfigs/easybuild/easyconfigs \
  --stack-policy stacks/site.toml \
  --out-dir work/gromacs-2024a
```

`--easyconfigs` is repeatable. Put the upstream tree first and a site overlay after it.

## Stack pins and fallback

Express EESSI or site-stack preferences in the stack policy, not in post-processing:

```toml
schema_version = 1
name = "eessi-compatible"

[toolchain]
name = "foss"
version = "2024a"

[[pins]]
name = "HDF5"
version_requirement = "==1.14.3"
mode = "preferred"
source = "EESSI stack"
```

`preferred` asks Resolvo to select the pin and records whether it fell back. `locked` makes the pin mandatory. Compatibility metadata is not build evidence: if a preferred or locked selection fails on the build target, Hermes owns the classified repair loop and may change the policy only with evidence.

Use repeatable `--dep NAME=VERSION` only for a package-specific hard override. It is folded into the solve as a locked pin and appears in the lock evidence.

## Run an application version bump

```sh
eb-stack package bump \
  --source GROMACS-2024.4-foss-2023b.eb \
  --toolchain-name foss \
  --toolchain-version 2024a \
  --version 2025.0 \
  --source-checksum SHA256 \
  --easyconfigs /path/to/robot \
  --stack-policy stacks/site.toml \
  --out-dir work/gromacs-2025
```

Verify source URLs, checksum ordering, and patch applicability for an application version change. Resolvo handles the dependency closure; it cannot prove that an old patch applies to new source.

## Verify the bundle

```sh
eb-stack recipe format work/gromacs-2024a/easyconfigs/g/GROMACS/*.eb
eb-stack recipe lint work/gromacs-2024a/easyconfigs/g/GROMACS/*.eb
eb-stack recipe check \
  --recipe work/gromacs-2024a/easyconfigs/g/GROMACS/GROMACS-2024.4-foss-2024a.eb \
  --easyconfigs /path/to/robot \
  --easyconfigs work/gromacs-2024a/easyconfigs
```

Inspect `locks/default.lock.json` for selected versions and pin fallback outcomes. Compare the emitted recipe with its source and require an explanation for every change outside toolchain, dependency versions, requested application version/checksum, and mechanical formatting.

For a multi-package stack, solve the full candidate set separately:

```sh
eb-stack stack solve \
  --easyconfigs work/overlay \
  --easyconfigs /path/to/robot \
  --policy policy.json \
  --baseline-easyconfigs /path/to/old-generation \
  --baseline-toolchain-version 2023b \
  --lock-out work/stack.lock.json \
  --sbom-out work/stack.cdx.json \
  --build-list-out work/build.list \
  --stack-diff-out work/stack.diff.md
```

## Build and repair

Use the layered target configuration and persisted campaign described in `skills/new-package/SKILL.md`:

```sh
eb-stack target doctor --config targets/base.toml --config targets/site.toml --target site-builder
eb-stack campaign run \
  --bundle work/gromacs-2024a \
  --config targets/base.toml \
  --config targets/site.toml \
  --target site-builder \
  --state work/gromacs-2024a.campaign.json
```

Hermes classifies each failure and drives recipe, policy, or target repair. OMP workers coordinate through `campaign finding claim` and `campaign finding resolve`; they never edit campaign JSON or work the same finding concurrently. Re-run the campaign until the requested claim rung is green.

Build failure classes are evidence, not solver contradictions. A SAT-compatible dependency set can still fail to configure, compile, link, test, install, or pass sanity checks on a concrete target.

## Claims

Report independently:

1. `resolves`: the bump bundle contains a successful Resolvo profile lock.
2. `builds`: the emitted EasyBuild recipe installs on the configured target.
3. `binary-verified`: declared post-build commands run successfully.

A successful `package bump` establishes only `resolves`. Never claim `builds` from recipe parsing, a dry run, or solver output.

Keep one reviewable recipe set per contribution. Do not open or mutate public issues or PRs; return paste-ready material to the human owner.
