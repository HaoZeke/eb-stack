# Changelog

All notable changes to this project are documented here.

## Unreleased

### Added

- Overlay planning treats existing robot modules as leaves: a `Python`
  easyconfig that names `binutils` no longer makes `package plan --format
  pypi` unsatisfiable. `--easyconfigs` is the solve robot only; it does
  not start package-closure discovery.

- `kind = "eessi"` target runtime: `eessi_container.sh --mode exec` is
  the install backend for planned PyPI/CRAN overlays. Plan stays on the
  host. `scripts/eessi-extend-eb.sh` loads `EESSI-extend` and execs `eb`.
  See `examples/targets/eessi-extend.toml`.

- `exts_list` entries are virtual Resolvo provides of the parent bundle.
  A requirement for `numpy` is satisfied by `SciPy-bundle` (and the same
  for `Python-bundle-PyPI` / `R-bundle-CRAN`) instead of failing as a
  missing easyconfig. Profile locks collapse the provide to the parent so
  emitted recipes depend on the bundle, not on a fake `numpy` module.
  Planning `numpy` / `scipy` / `torch` as a PyPI root against a robot
  that already ships them is an empty delta. Without that provider the
  plan refuses a pip overlay instead of emitting a `PythonBundle`.
  Warehouse `requires_dist: null` (live numpy JSON) parses as no extras.
  A leftover such as `eon-akmc` whose other PyPI deps are not in the
  robot keeps those names in `exts_list` instead of failing SAT.
- `--format cargo` reads `Cargo.toml` or crates.io JSON. PyO3/maturin
  crates emit `PythonPackage` with implicit `Rust`, `maturin`, and
  `binutils`; other crates emit `Crate`. Host Cargo wrappers
  (`sccache`, `mold`) are unset. Existing robot Rust modules are leaves.
  `--cargo-source` / `kind = "cargo"` closes a PyPI leftover that is a
  crate (for example `readcon`) as a companion module; remaining pure
  PyPI holes stay `exts_list` extras. Overlay extras prepend the
  install prefix to `PYTHONPATH` so `pip --no-build-isolation` sees
  prior extensions and the loaded EESSI modules. Mesonpy ingest reads
  an optional PEP 518 `build_system` object. Extra site packages and
  meson wrap natives named in that ingest become SAT requirements;
  Resolvo takes `quill` / `cbindgen` / `Eigen` / `PyYAML` from the
  robot when those modules exist. Do not hand-edit the emitted recipe
  for those names. Cargo leftovers and mesonpy wraps share one
  cargo-on-EESSI isolation prelude (`cargo::eessi_cargo_host_isolation`):
  host rustc wrappers and `RUSTFLAGS` are unset, `LINKER` is EESSI gcc,
  and the compat `ld` comes from `uname -m`, not a plan-time x86_64
  literal.
- `--format pypi` reads Warehouse-shaped JSON or a `requirements.txt` and
  emits a `PythonBundle` whose `exts_list` is the leftover package, with
  already-provided extras mapped to the parent bundle.
- `--format cran` reads a DESCRIPTION file, CRAN JSON, or a package list and
  emits one `RPackage` recipe when the robot carries its imports, or a
  `Bundle` with `exts_defaultclass = 'RPackage'` carrying the leftovers in
  `exts_list` when it does not. Base-R packages are recorded as residuals.
- Requirement parsing is one implementation shared by the solver and the
  emitter, covering `==`, `!=`, `>=`, `>`, `<=`, `<`, `~=` (PEP 440) and `^`
  and `~` (Cargo). A clause the language cannot express becomes an
  `unparsed-constraint` residual instead of an empty version set.
- Cargo dependencies keep the requirement the manifest states, and `path` and
  `git` dependencies are reported as judgment residuals: neither can be built
  from a published crate tarball.
- `package plan --package-index FILE` reads a repository index, either the
  format CRAN publishes as `PACKAGES` or a pinned requirements file as
  `pip freeze` writes it, supplying versions and checksums for dependencies
  that state none of their own. A CRAN source now comes from
  CRAN's contrib location rather than the project home page in `DESCRIPTION`,
  and an R bundle carries `exts_default_options` pointing at the current and
  archived CRAN paths.
- Package identity, the pip-overlay refusal list, PyO3 marker crates and the
  crate-to-module renames live in `data/overlay-policy.toml` rather than in
  match arms.
- Name-mode ingest (`--source eon-akmc` with `--format pypi`) fetches
  Warehouse / CRAN / crates.io once and writes `ingest/<format>/`. SAT
  and tests only read that dump. A neighbouring sdist tree overlays
  `pyproject.toml` and `subprojects/*.wrap` as build deps; undeclared
  Python imports are residuals, not silent SAT edges.
- `--format luarocks` reads a `*.rockspec`. `--format raku` reads
  `META6.json`. Both lower to `ForeignRecipe` and the same Resolvo
  path. They are not new solvers.
- Example stack policies `examples/stacks/eessi-python-extras.toml` and
  `examples/stacks/eessi-r-extras.toml` for locking EESSI-shipped
  scientific Python / R providers.

### Added

- `package plan` generates a recipe with the structure upstream writes.
  Pointed at a PyPI package at `GCCcore-14.2.0`, the emitted file is
  byte-identical in shape to upstream's own recipes: `PythonPackage` rather
  than a one-entry bundle, `sources = [SOURCE_TAR_GZ]`, a `binutils` build
  dependency, short lists inline, one-line descriptions in single quotes,
  and no empty dependency fields.
- `--source name==version` fetches one PyPI release rather than whatever is
  newest, so regenerating an existing recipe describes the same release and
  the same command gives the same answer tomorrow.
- `tests/generation/backtest.py` regenerates upstream's own
  `PythonPackage` recipes, each with its own recipe hidden from the robot
  path and its version pinned, and diffs field by field. Over the
  GCCcore-14.2.0 corpus: easyblock 10/10, toolchain 10/10, dependencies
  10/10, builddependencies 10/10, sources 10/10, moduleclass 5/10.
- A `moduleclass` is taken from the recipe the tree already carries for a
  package when there is one, and otherwise inferred from the package's Trove
  classifiers. Which of the two happened is recorded as a residual, because
  upstream's own class is a judgement: `einops` is `math` and `fonttools`
  `devel`, and neither follows from the metadata.

### Fixed

- A `pyproject.toml` was never parsed: `text.parse::<toml::Value>()` reads a
  TOML value rather than a document, so every build requirement a project
  states was dropped in silence, and an sdist's own top directory hid the
  file from the lookup as well. With the data flowing, `setuptools`, `pip`
  and `wheel` are recognised as coming with the Python module, a backend and
  its module are one name (`poetry-core` is built by `poetry`), and a build
  requirement the tree carries no module for is dropped with a residual
  rather than kept as a dependency nothing can satisfy.
- `stack solve` against the upstream tree no longer reports a foss generation
  as unsatisfiable. A name the system level carries at several versions is one
  package per version, because that level is the bootstrap layer and every
  dependency on it pins a version exactly: binutils 2.40 builds the GCCcore
  that builds binutils 2.42, and EasyBuild installs both. `foss-2025a` locks
  63 packages.
- Every easyconfig upstream ships now parses: 0 skipped of some 20,600,
  down from 91. The forms that were being lost, each of which dropped a
  whole recipe rather than one field: `version[2:]` and `patchlevels[0]`,
  a dependency list written with braces (a set, which EasyBuild iterates
  and builds), `[...] + local_extra`, a bare `ARCH`, an arch-specific
  dependency version (`{'arch=x86_64': ...}`, nine recipes and the `Java`
  wrappers), and the string methods `split` / `join` / `replace` / `lower`
  / `upper` / `strip`.
- A `versionsuffix` that was assigned and could not be read is refused, on
  the rule already covering dependency lists: the suffix is part of the
  module name, so reading it as absent gives the recipe an identity it will
  not install under.
- Recipes are read the way a robot path actually writes them: a `#` inside
  a multi-line `description` is no longer a comment (Doxygen names C#, and
  the closing `"""` was going with it), f-string versions are read, `%%`
  collapses when a recipe formats a template through `%`, `%(pyver)s` and
  its CUDA / R / Java / Perl siblings come from the dependency rather than
  from the recipe's own version, and `('gettext', '0.19.8.1', '', True)`
  means the system toolchain. Skipped easyconfigs across upstream fall from
  91 to 43 of some 20,600.
- The system toolchain is recognised under its former name, `dummy`, which
  1374 recipes in a 2019 tree still use.
- A dependency that names a module rather than a bare version resolves:
  GCC `8.2.0-2.31.1` for version 8.2.0 with versionsuffix `-2.31.1`, and
  binutils 2.26 with versionsuffix `-GCCcore-5.4.0` for the build at
  GCCcore-5.4.0.

### Added

- `tests/property` backtests the ordering properties against one commit per
  year of easybuild-easyconfigs, materialised with `git archive`. Sampled
  roots that order successfully across 26 commits from 2014 to 2026:
  312/312.

## 0.3.0 - 2026-07-21

First public release: package plan/bump/campaign CLI, Resolvo locks, CycloneDX
SBOMs, multi-platform installers (cargo-dist), and operator documentation.

### Changed

- The public eOn policy targets the core + `with_rgpot` product on a single
  foss-2026.1 / GCCcore-15.2.0 generation: no cross-generation PyTorch/xtb
  pins, no staged cargo-c preconfigopts, torch-family conda host deps
  excluded from the solve, and readcon-core plus rgpot closed as authored
  companion easyconfigs. `fixtures/eon_core_rgpot` freezes the upstream
  eOn 2.17.2 draft PR file set as the regression surface.

### Added

- `skills/upstream-pr/SKILL.md`: easybuild-easyconfigs PR conventions
  distilled from real merged and rejected PRs (single-generation closures,
  readable recipes, precedent citation, AI disclosure, test-report
  lifecycle).

- MCP tools `eb_recipe_lint` and `eb_stack_sbom`, plus explicit optional
  schemas for `eb_package_bump`, `eb_recipe_check`, `eb_recipe_format`, and
  `eb_stack_solve`, so the MCP catalog matches the version-one CLI surface.
- CI job for package catalog, package closure, closure write, and source-root
  discovery suites (previously only covered by local `cargo test --all-targets`).
- Claim-ladder, command-surface, and pipeline diagrams (Graphviz source in the
  manuals; PNG/SVG under `assets/illustrations/`). CLI reference documents
  package catalog and source-root plan flags.
- Package-neutral source-root discovery for package closure: ordered local
  EasyBuild, conda-forge, and Spack indexes close robot holes without a
  committed per-package catalog entry. Explicit catalogs remain optional
  overrides. EasyBuild bumps preserve toolchain family (for example GCCcore
  maps to the GCCcore member of the target hierarchy).
- Public example `examples/package-sources/local-roots.toml` and CLI/MCP flags
  `--package-sources`, `--easybuild-source`, `--conda-source`, `--spack-source`.
- Catalog provider kinds `foreign` (default) and `easybuild-bump` for
  package-source catalog entries, so recursive package closure can retarget an
  existing EasyBuild recipe through the annual-bump pipeline instead of
  substituting a foreign archive.
- Public package-neutral catalog example at
  `examples/package-catalog/mixed-providers.toml`.
- Canonical schema-versioned package plan shared by conda-forge, Spack, and
  EasyBuild inputs, with source provenance, structured conditions, variants,
  rules, build intent, product profiles, output requests, and residuals.
- Planned CycloneDX SBOM generation from canonical package intent and solved
  EasyBuild stack locks, including primary source hashes, VCS identities, and
  hashed distribution references.
- Per-profile materialization and Resolvo selection with preferred pins,
  locked pins, candidate exclusions, and recorded fallback outcomes.
- One conventional EasyBuild recipe and profile lock per installable product
  profile; default profiles remain unsuffixed.
- Positional source-checksum overrides at the CLI and MCP emission boundary,
  with complete source coverage required before a recipe is emitted.
- Canonical new-package and bump bundles containing `package.plan.json`,
  `package.sbom.cdx.json`, profile locks, and EasyBuild recipes.
- Layered public TOML build targets covering local/SSH transport,
  direct/Slurm execution, host/Podman/Docker runtime, and EasyBuild workload.
- Persisted build campaigns with exact routed commands, independent claim
  ladder, typed findings, ownership, resolution evidence, and retry
  supersession.
- Profile binary-verification commands with package/module/profile
  placeholders.
- Version-one CLI and MCP surfaces for package planning, recipe checks, stack
  solving, targets, campaigns, and finding coordination.
- Public new-package and annual-bump skills implementing the Hermes/OMP
  build-evaluation loop.

### Changed

- Existing-recipe bumps use the same SBOM, manifest, Resolvo lock, EasyBuild
  emission, target routing, and campaign model as new packages.
- Existing robot artifacts keep independent build-only dependency contexts
  during package-profile solving, matching EasyBuild's installed artifact
  model.
- Recipe style lint/format is namespaced under `recipe` and remains purely
  mechanical.
- Documentation, examples, CI, and acceptance fixtures use only the
  version-one command and MCP names.
- CI enforces the Rust 1.88 minimum, formatting, clippy with warnings denied,
  and public metadata contracts.
- Rust-backed fixture recipes reset Cargo compiler wrappers without exposing
  host configuration inherited through mounted build paths.
- Campaign state uses an OS-backed exclusive guard with process-identity
  metadata, so interrupted controllers do not leave permanent stale locks.

### Removed

- The unreleased scaffold ingest, companion placeholder, intermediate plan,
  standalone bump, and auto-emitter APIs.
- Compatibility shims and legacy CLI/MCP command names.
- Generated placeholder recipes with dummy sources or checksums.
