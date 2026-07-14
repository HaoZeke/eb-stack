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

```sh
git clone https://github.com/HaoZeke/eb-stack.git
cd eb-stack
cargo test --locked
cargo build --locked --release
install -m755 target/release/eb-stack ~/.local/bin/eb-stack
```

Build the Rust binary on a suitable build host. EasyBuild installs belong on the target selected by the public target configuration, not necessarily on the machine running the CLI.

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

Product profiles are public TOML. Each profile is one independently installable EasyBuild variant; MPI/OpenMP toolchain options alone do not require a suffix.

```sh
eb-stack package plan \
  --source fixtures/foreign_ingest/conda_eon/recipe.yaml \
  --format conda-forge \
  --toolchain-name foss \
  --toolchain-version 2026.1 \
  --profile-config examples/profiles/eon.toml \
  --easyconfigs /path/to/easybuild-easyconfigs/easybuild/easyconfigs \
  --easyconfigs fixtures/eon_foss_2026_1/easyconfigs \
  --stack-policy examples/stacks/foss-2026.1.toml \
  --out-dir /tmp/eon
```

Use `--format spack` and `examples/profiles/qmcpack.toml` for QMCPACK’s `package.py`. Parser output retains source spans, conda selectors, Spack conditions/conflicts, dependency roles, and residual dynamic logic.

Inspect without solving or emitting recipes:

```sh
eb-stack package inspect \
  --source fixtures/foreign_ingest/spack_qmcpack/package.py \
  --format spack \
  --toolchain-name foss \
  --toolchain-version 2026.1 \
  --profile-config examples/profiles/qmcpack.toml \
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

Transport may be local or SSH; execution may be direct or Slurm; runtime may be host, Podman, or Docker. Site hostnames, paths, modules, and scheduler sizing belong in the site layer.

Campaign failures persist as typed findings. OMP workers coordinate repairs through owned queue operations:

```sh
eb-stack campaign finding claim --state /tmp/eon.campaign.json \
  --id attempt:1:finding:1 --owner omp-worker-1

eb-stack campaign finding resolve --state /tmp/eon.campaign.json \
  --id attempt:1:finding:1 --owner omp-worker-1 \
  --action "corrected profile configuration" \
  --evidence "recipe check exits successfully" \
  --change examples/profiles/eon.toml
```

The state file retains attempts, commands, compact logs, ownership, repair evidence, and the claim ladder. A successful retry supersedes the matching open finding.

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
