---
name: eb-stack-new-package
description: Convert a conda-forge or Spack recipe into a canonical package manifest, CycloneDX SBOM, Resolvo profile locks, separate EasyBuild easyconfigs for product variants, and a remotely built and verified campaign. Use for new EasyBuild packages, foreign recipe imports, eOn or QMCPACK packaging, and Hermes/OMP build-repair loops.
---

# Build a new EasyBuild package

Run the entire workflow from the canonical package plan. Do not hand-select dependency versions or treat a generated recipe as built.

## Inputs

Collect these paths before planning:

- the conda-forge `recipe.yaml`/`meta.yaml` or Spack `package.py`;
- one or more EasyBuild robot trees for the target generation;
- a package profile TOML defining product variants and verification commands;
- a stack policy TOML defining site preferences, locks, and exclusions;
- positional SHA-256 values when the foreign recipe identifies sources only
  by VCS tag or commit;
- layered target TOML naming the remote EasyBuild host and execution backend.

Use conda-forge for eOn and Spack for QMCPACK. Parsing is syntax-aware and preserves selectors, Spack `when=` expressions, source provenance, dependency roles, and unresolved dynamic logic in the manifest.

## Product profiles and EasyBuild variants

Create one product profile per independently installable variant. Each profile emits one `.eb` file.

- Keep the default CPU or standard MPI/OpenMP profile unsuffixed.
- Add a semantic `versionsuffix` only when users need both variants installed, such as `-complex`, `-cuda`, or `-mixed`.
- Do not create suffixes merely because `usempi` or `openmp` is enabled.
- Follow neighboring GROMACS and LAMMPS easyconfigs for naming, option placement, dependencies, sanity checks, and module class.

Example profile layer:

```toml
schema_version = 1

[[profiles]]
name = "default"
default = true
config_options = ["-DQMC_MPI=ON", "-DQMC_OMP=ON", "-DQMC_COMPLEX=OFF"]
verification_commands = [
  { program = "bash", args = ["-lc", "module load {module} && qmca --help"] },
]

[profiles.features]
mpi = true
complex = false

[profiles.toolchain_options]
usempi = true
openmp = true

[[profiles]]
name = "complex"
inherits = "default"
versionsuffix = ["-complex"]
config_options = ["-DQMC_MPI=ON", "-DQMC_OMP=ON", "-DQMC_COMPLEX=ON"]

[profiles.features]
complex = true
```

Verification arguments may use `{module}`, `{package}`, `{version}`, `{profile}`, and `{versionsuffix}`. A campaign can claim `binary-verified` only when at least one declared command succeeds for every declared command set.

## Stack policy

Keep site stack preferences inside Resolvo:

```toml
schema_version = 1
name = "site-stack"

[toolchain]
name = "foss"
version = "2026.1"

[[pins]]
name = "HDF5"
version_requirement = "==2.1.1"
mode = "preferred"
source = "site stack"

[[pins]]
name = "PyTorch"
version_requirement = "==2.9.1"
toolchain = { name = "foss", version = "2024a" }
versionsuffix = ""
mode = "preferred"
source = "distribution stack"
```

Use `toolchain` and `versionsuffix` when a stack requires one exact EasyBuild
artifact rather than only a package version. Omitting either field leaves that
part of the identity unconstrained. Use `preferred` when Resolvo may fall back
to another compatible candidate; the profile lock records the requested and
selected identity and whether fallback occurred. Use `locked` when any other
identity is invalid. A claimed-compatible version that fails to build remains
a build finding; Hermes decides whether the recipe, target, or stack policy
needs repair.

## Plan the package

Inspect first when evaluating parser output:

```sh
eb-stack package inspect \
  --source path/to/recipe.yaml \
  --format conda-forge \
  --toolchain-name foss \
  --toolchain-version 2026.1 \
  --profile-config profiles/qmcpack.toml \
  --out-dir work/qmcpack-inspect
```

Produce the buildable bundle:

```sh
eb-stack package plan \
  --source path/to/package.py \
  --format spack \
  --toolchain-name foss \
  --toolchain-version 2026.1 \
  --profile-config profiles/qmcpack.toml \
  --source-checksum SHA256 \
  --easyconfigs /path/to/easybuild-easyconfigs/easybuild/easyconfigs \
  --easyconfigs /path/to/site-overlay \
  --stack-policy stacks/site.toml \
  --out-dir work/qmcpack
```

The bundle must contain:

- `package.plan.json`: canonical build manifest and residuals;
- `package.sbom.cdx.json`: planned CycloneDX SBOM;
- `locks/<profile>.lock.json`: one Resolvo result per profile;
- `easyconfigs/<letter>/<name>/*.eb`: one deterministic recipe per profile.

Treat a planning error as unresolved input. Do not copy a foreign pin into an `.eb` file to bypass Resolvo.
Repeat `--source-checksum` in manifest source order when the foreign recipe
lacks archive hashes. A VCS commit is not an archive SHA-256; never omit the
checksum or invent one from the commit.

## Check emitted recipes

Run mechanical checks for every emitted recipe:

```sh
eb-stack recipe format work/qmcpack/easyconfigs/q/QMCPACK/*.eb
eb-stack recipe lint work/qmcpack/easyconfigs/q/QMCPACK/*.eb
eb-stack recipe check \
  --recipe work/qmcpack/easyconfigs/q/QMCPACK/QMCPACK-4.3.0-foss-2026.1.eb \
  --easyconfigs /path/to/robot \
  --easyconfigs work/qmcpack/easyconfigs
```

Fix the code under test when a check fails. Do not drop checksums, weaken sanity paths, remove dependencies, or skip tests to clear a finding. Use EasyBuild’s `--check-contrib` and `--inject-checksums` on the EasyBuild host when packaging metadata needs them.

## Route the build

Target configuration is layered as transport → executor → runtime → EasyBuild workload. Keep site hostnames and paths in site-local TOML, not in the public skill.

```sh
eb-stack target list --config targets/base.toml --config targets/site.toml
eb-stack target doctor \
  --config targets/base.toml \
  --config targets/site.toml \
  --target site-builder
```

Use SSH for remote transport, Slurm for isolated jobs when available, and `host`, `podman`, or `docker` for runtime. `target doctor` must pass before a campaign. Never run EasyBuild installs on the control laptop.

Scope `EASYBUILD_INSTALLPATH`, `work_root`, and `tmp_root` to the runtime ABI.
Do not reuse host-built modules inside a container or share compiled modules
between different images. `EASYBUILD_SOURCEPATH` may remain a shared archive
cache.

Size `EASYBUILD_PARALLEL` against the executor's memory allocation, not only
its CPU count. Memory-heavy C++ translation units can consume more than a
gigabyte per compiler process. Start a workstation target at two jobs and
raise it only with measured headroom; a kernel-killed compiler or exhausted
virtual memory is a `resource` finding, not a reason to change the selected
dependency or weaken the recipe.

Cargo reads `.cargo/config.toml` files above its build directory. Keep a
container target's `work_root` outside personal tool-configuration trees. A
recipe that disables compiler wrappers must set `RUSTC_WRAPPER=` and
`CARGO_BUILD_RUSTC_WRAPPER=` to empty values; `unset` reveals an inherited
Cargo wrapper.

## Hermes build-evaluation loop

Hermes is the single campaign-owner role, not a required orchestration
product. It owns classification, repair decisions, retries, and the final
claim ladder. OMP workers are optional concurrent participants that use only
the campaign finding queue for shared coordination.

Use one durable state path for the package:

```sh
eb-stack campaign run \
  --bundle work/qmcpack \
  --config targets/base.toml \
  --config targets/site.toml \
  --target site-builder \
  --state work/qmcpack.campaign.json
```

`campaign run` is a foreground process. Keep it under the site's normal
terminal or service supervisor and inspect `campaign status` from another
shell. If its controller exits, verify that the routed container, scheduler
job, or host command is absent before rerunning the same state. The state lock
releases automatically, but a remote workload can survive its controller.

Hermes owns this loop until the requested claim is established:

1. Read `campaign status`; inspect the newest open finding and its full command evidence.
2. Classify before editing: `transport`, `executor`, `runtime`, `checksum`, `patch`, `dependency-missing`, `configure`, `compile`, `link`, `test`, `install`, `sanity`, `resource`, `timeout`, or `unknown`.
3. Apply target repair for transport/executor/runtime findings; retry resource/timeout findings with corrected allocation; apply deterministic checksum repair mechanically; use package judgment for the remaining recipe failures.
4. Re-run recipe checks after recipe or profile changes.
5. Re-run the same campaign command. Successful retries supersede matching open findings while retaining their evidence history.
6. Continue through post-build verification. Do not stop at a successful compile when verification commands are declared.

`campaign status` emits the complete historical JSON. During a long build,
filter it to `status`, `attempts`, `claims`, `current_recipe`, and findings
whose status is `open` or `in-progress`; retain the full state as the evidence
record.

Never edit a stock recipe to hide a target defect. Never weaken a test or sanity check. Record changed files and concrete verification output in the finding resolution.

## OMP finding coordination

Campaign state is the sole shared work queue. One worker must claim a finding before changing its inputs:

```sh
eb-stack campaign finding claim \
  --state work/qmcpack.campaign.json \
  --id attempt:1:finding:1 \
  --owner omp-worker-1
```

Resolve only after the repair is checked:

```sh
eb-stack campaign finding resolve \
  --state work/qmcpack.campaign.json \
  --id attempt:1:finding:1 \
  --owner omp-worker-1 \
  --action "corrected the complex profile configuration" \
  --evidence "recipe check exits successfully" \
  --change profiles/qmcpack.toml
```

The state lock prevents concurrent writers. Workers must not steal an `in-progress` finding or edit the campaign JSON directly. Hermes resumes the campaign after owned repairs are resolved.

## Claims

Report each rung independently:

1. `resolves`: every profile has a Resolvo lock and emitted recipe.
2. `builds`: every emitted recipe completes through EasyBuild on the configured target.
3. `binary-verified`: all declared post-build commands execute successfully through the same target routing.

An inspect bundle establishes none of these. A planned bundle establishes only `resolves`. A campaign with no verification commands may establish `builds` but not `binary-verified`.

Do not open, edit, or merge a public issue or PR. Prepare the recipe set, evidence, and paste-ready text for the human-owned contribution surface.
