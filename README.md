<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/branding/eb-stack-logo-dark.svg">
    <img src="assets/branding/eb-stack-logo.svg" alt="eb-stack" width="420">
  </picture>
</p>

**Turn foreign and EasyBuild recipes into resolved, build-evaluated package bundles.**

`eb-stack` parses conda-forge, Spack, and EasyBuild recipes into one canonical package plan. It emits a planned CycloneDX SBOM, solves each product profile with Resolvo, writes one conventional `.eb` file per installable variant, and drives the recipes through a persisted remote build campaign.

[![CI](https://github.com/HaoZeke/eb-stack/actions/workflows/ci_test.yml/badge.svg)](https://github.com/HaoZeke/eb-stack/actions/workflows/ci_test.yml)
[![Docs](https://github.com/HaoZeke/eb-stack/actions/workflows/ci_docs.yml/badge.svg)](https://github.com/HaoZeke/eb-stack/actions/workflows/ci_docs.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

## Pipeline

```text
conda-forge / Spack / EasyBuild
              │
              ▼
 canonical package.plan.json ─── package.sbom.cdx.json
              │
              ▼
 profile materialization + stack policy + Resolvo
              │
              ├── locks/<profile>.lock.json
              └── easyconfigs/<letter>/<name>/<variant>.eb
                              │
                              ▼
             target-routed campaign + typed findings
```

New packages and bumps use the same artifacts. A bump is an EasyBuild-origin plan plus SBOM and Resolvo lock, not a standalone text rewrite.

## Claim ladder

Report these claims independently:

1. **resolves** — every requested profile has a Resolvo lock and emitted recipe;
2. **builds** — every emitted recipe completes through EasyBuild on the configured target;
3. **binary-verified** — every declared profile verification command succeeds.

`package inspect` establishes none of them. `package plan` and `package bump` can establish only `resolves`. `campaign run` owns the other two.

## Install

Rust 1.88 or newer is required for the locked dependency graph.

```sh
git clone https://github.com/HaoZeke/eb-stack.git
cd eb-stack
cargo test --locked
cargo build --locked --release
install -m755 target/release/eb-stack ~/.local/bin/eb-stack
```

Build the Rust binary on a suitable build host. EasyBuild installs belong on the target selected by the public target configuration, not necessarily on the machine running the CLI.

## Inspect from a fresh clone

The repository includes foreign-recipe and package-config fixtures, so the parsing
boundary can be exercised without an EasyBuild installation or external robot
tree:

```sh
eb-stack package inspect \
  --source fixtures/foreign_ingest/conda_eon/recipe.yaml \
  --format conda-forge \
  --toolchain-name foss \
  --toolchain-version 2026.1 \
  --package-config examples/packages/common.toml \
  --package-config examples/packages/eon.toml \
  --out-dir /tmp/eon-inspect

python3 -m json.tool /tmp/eon-inspect/package.plan.json >/dev/null
python3 -m json.tool /tmp/eon-inspect/package.sbom.cdx.json >/dev/null
```

This writes the canonical build manifest and planned CycloneDX SBOM. It does
not solve dependencies, emit recipes, or make a build claim; those stages need
an EasyBuild robot tree and a configured build target.

## Documentation

The manual is organized around operator tasks:

- [foreign recipe to verified package](docs/orgmode/tutorial.org);
- [package bundle schemas](docs/orgmode/reference/package-bundles.org);
- [layered build targets](docs/orgmode/reference/targets.org);
- [campaign state and typed findings](docs/orgmode/reference/campaigns.org);
- [repair a failed campaign](docs/orgmode/howto/repair-campaign.org);
- [command-line reference](docs/orgmode/reference/cli.org).

The source manual lives in `docs/orgmode/`; the documentation workflow exports RST into `docs/source/` and validates the Sphinx site.

## Bump an existing recipe

```sh
eb-stack package bump \
  --source tests/repro_fixtures/gromacs/GROMACS-2024.4-foss-2023b.eb \
  --toolchain-name foss \
  --toolchain-version 2024a \
  --easyconfigs tests/repro_fixtures/universe_foss_2024a \
  --out-dir /tmp/gromacs-2024a
```

The output recipe is `/tmp/gromacs-2024a/easyconfigs/g/GROMACS/GROMACS-2024.4-foss-2024a.eb`; the same directory contains its manifest, SBOM, and `default` profile lock. Repeat `--easyconfigs` for overlays and use `--stack-policy` for EESSI or site preferences. `preferred` pins may fall back inside Resolvo; `locked` pins may not.

Known maintainer fixtures cover GROMACS, ScaFaCoS, MDTraj, Fiona, PuLP, and numba across `foss-2023b` → `foss-2024a`.

## Plan a new package

Package policy is public, layered TOML. Parsers preserve foreign names and
syntax without package-name branches; Resolvo matches case/punctuation-equivalent
robot names mechanically. Explicit ecosystem aliases, EasyBuild metadata,
build policy, and independently installable profiles live in package config.
Each profile emits one `.eb` file; MPI/OpenMP toolchain options alone do not
require a suffix.

The Spack adapter parses Python into an AST without importing the recipe. It
statically evaluates literal data, bounded loops, formatting, and `when`
scopes; unsupported runtime expressions become source-located residuals rather
than guessed dependencies.

Aliases use `foreign = "EasyBuild"` when both names share a version domain.
Component-to-provider mappings use
`foreign = { provider = "EasyBuild", constraint = "drop" }`, so a component
release such as a Python package version is never imposed on its containing
EasyBuild provider.

Package policy also has two generic EasyBuild inputs:

```toml
[build.easyconfig_parameters]
general_packages = ["ASPHERE", "KSPACE", "MOLECULE"]

[[build.patches]]
filename = "Orbit-2.0-portability.patch"
sha256 = "4f43b42fdcf84d0cf634d993dd944f252c8243dc612a919fe2825d56f937c8eb"
source = "patches/Orbit-2.0-portability.patch"

[[dependencies.requirements]]
name = "VTK"
roles = ["run"]
```

Easyconfig parameters are typed data, not Python fragments. Requirements
enter the canonical manifest and CycloneDX SBOM before Resolvo selects a
version. Package planning requires every source and patch SHA-256; emitted
checksums remain positional with sources first and patches second. Verified
patch assets are copied beside every emitted recipe.

```sh
eb-stack package plan \
  --source fixtures/foreign_ingest/conda_eon/recipe.yaml \
  --format conda-forge \
  --toolchain-name foss \
  --toolchain-version 2026.1 \
  --package-config examples/packages/common.toml \
  --package-config examples/packages/eon.toml \
  --easyconfigs /path/to/easybuild-easyconfigs/easybuild/easyconfigs \
  --easyconfigs fixtures/eon_foss_2026_1/easyconfigs \
  --stack-policy examples/stacks/eon-foss-2026.1.toml \
  --out-dir /tmp/eon
```

The shipped eOn stack policy carries its reviewed cross-generation PyTorch,
xtb, Eigen, and Meson identities as `preferred` pins. Resolvo admits those
artifact closures and records either the selected identity or a compatible
fallback. The generic `foss-2026.1.toml` template remains unpinned for other
packages.

Use `--format spack`, `examples/packages/common.toml`, and
`examples/packages/qmcpack.toml` for QMCPACK’s
`package.py`. Its Spack version is pinned by commit rather than archive hash,
so planning also needs
`--source-checksum 511d5f368db002f2f77504619e1ada8d4a3034200d25feef6773d12a6ed6d18e`.
Parser output retains source spans, conda selectors, Spack
conditions/conflicts, dependency roles, and residual dynamic logic.

Inspect without solving or emitting recipes:

```sh
eb-stack package inspect \
  --source fixtures/foreign_ingest/spack_qmcpack/package.py \
  --format spack \
  --toolchain-name foss \
  --toolchain-version 2026.1 \
  --package-config examples/packages/common.toml \
  --package-config examples/packages/qmcpack.toml \
  --out-dir /tmp/qmcpack-inspect
```

## Check recipes

```sh
eb-stack recipe format /tmp/eon/easyconfigs/e/eOn/*.eb
eb-stack recipe lint /tmp/eon/easyconfigs/e/eOn/*.eb
eb-stack recipe check \
  --recipe /tmp/eon/easyconfigs/e/eOn/eOn-2.16.0-foss-2026.1.eb \
  --easyconfigs /path/to/robot \
  --easyconfigs /tmp/eon/easyconfigs
```

Checksums are positional: sources first, then patches. Missing dependency output includes compatible hierarchy members and candidates found in other generations. Fix the recipe or companion; do not bypass the check.

## Configure and run builds

Targets are layered as transport → executor → runtime → EasyBuild workload:

```sh
eb-stack target list --config examples/targets/base.toml --config ~/.config/eb-stack/site.toml
eb-stack target doctor \
  --config examples/targets/base.toml \
  --config ~/.config/eb-stack/site.toml \
  --target site-builder

eb-stack campaign run \
  --bundle /tmp/eon \
  --config examples/targets/base.toml \
  --config ~/.config/eb-stack/site.toml \
  --target site-builder \
  --state /tmp/eon.campaign.json
```

`campaign run` is a foreground command. Keep it under the site's normal
terminal or service supervisor and inspect the state from another shell with
`campaign status`. Only one run, claim, or resolution can write a state file at
a time. The process lock releases automatically if the controlling process
exits; before rerunning an interrupted state, confirm that its routed
container, scheduler job, or host command is no longer active.

Transport may be local or SSH; execution may be direct or Slurm; runtime may be host, Podman, or Docker. Site hostnames, paths, modules, and scheduler sizing belong in the site layer. Container targets must use ABI-specific install, work, and temporary roots; only source archives should be shared across runtimes.

Cargo searches ancestor directories for `.cargo/config.toml`. If target storage lives below a bind-mounted home directory, mount the campaign subtree again at a neutral container path such as `/eb-stack-campaigns`, then use that path for runtime `workdir` and EasyBuild `work_root`. The [target reference](docs/orgmode/reference/targets.org) includes the complete TOML pattern.

The runnable local target defaults to two parallel EasyBuild jobs so memory-heavy C++ compilations remain usable on common workstations. Treat `EASYBUILD_PARALLEL`, scheduler CPUs, and scheduler memory as one allocation: raise parallelism only when the target has measured headroom. A compiler process killed by the kernel or exhausted virtual memory is a retryable `resource` finding, not evidence that the selected dependency is incompatible.

For a local public example, build `skills/new-package/container/rocky9/Containerfile`, populate `/tmp/eb-stack/robot`, and use `examples/targets/local-podman.toml`. Keep bundles below `/tmp/eb-stack/bundles` so host and container paths match.

Hermes and OMP name campaign roles, not required orchestration software.
Hermes is the single owner that reads typed findings, applies repairs, and
retries through the requested claim rung. OMP workers are optional concurrent
participants that coordinate exclusively through finding ownership.

Campaign failures persist as typed findings. OMP workers coordinate repairs through owned queue operations:

```sh
eb-stack campaign finding claim --state /tmp/eon.campaign.json \
  --id attempt:1:finding:1 --owner omp-worker-1

eb-stack campaign finding resolve --state /tmp/eon.campaign.json \
  --id attempt:1:finding:1 --owner omp-worker-1 \
  --action "corrected package configuration" \
  --evidence "recipe check exits successfully" \
  --change examples/packages/eon.toml
```

The state file retains attempts, the active recipe, typed failure commands and
compact logs, ownership, repair evidence, and the claim ladder. A successful
retry supersedes the matching open finding.

## Solve a whole stack

```sh
eb-stack stack solve \
  --easyconfigs fixtures/gromacs_2025_to_next/easyconfigs \
  --policy fixtures/gromacs_2025_to_next/policies/prefer_newer.json \
  --baseline-easyconfigs fixtures/gromacs_2025_to_next/easyconfigs \
  --lock-out stack.lock.json \
  --sbom-out stack.cdx.json \
  --build-list-out build.list \
  --stack-diff-out stack.diff.md
```

## MCP

`eb-stack mcp` exposes the same version-one workflows over stdio:

- `eb_package_inspect`, `eb_package_plan`, `eb_package_bump`;
- `eb_recipe_check`, `eb_recipe_format`, `eb_stack_solve`;
- `eb_target_list`, `eb_target_doctor`;
- `eb_campaign_run`, `eb_campaign_status`;
- `eb_campaign_finding_claim`, `eb_campaign_finding_resolve`.

## Skills

- [New package](skills/new-package/SKILL.md): conda-forge/Spack → profiles → bundle → Hermes/OMP campaign.
- [Annual bump](skills/annual-bump/SKILL.md): EasyBuild recipe → SBOM + Resolvo bump bundle → campaign.

The public issue and PR surface belongs to the human operator. The skills produce recipe sets and evidence; they do not open or mutate upstream issues or PRs.

## Tests

```sh
cargo test --locked --all-targets
```

The suite covers foreign syntax adapters, conditions, CycloneDX generation, profile materialization, stack-policy fallback, hierarchy-aware Resolvo locks, variant emission, known bump reproduction, target routing, persisted campaigns, binary verification, finding ownership, CLI, and MCP.

## License

[MIT](LICENSE) · [Code of Conduct](CODE_OF_CONDUCT.md) · [Security](SECURITY.md) · [Contributing](CONTRIBUTING.md)
