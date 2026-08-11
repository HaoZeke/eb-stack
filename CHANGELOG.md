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
  for those names.
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
- `package plan --package-index FILE` reads a repository index in the format
  CRAN publishes as `PACKAGES`, supplying versions and checksums for
  dependencies that state none of their own. A CRAN source now comes from
  CRAN's contrib location rather than the project home page in `DESCRIPTION`,
  and an R bundle carries `exts_default_options` pointing at the current and
  archived CRAN paths.
- Package identity, the pip-overlay refusal list, PyO3 marker crates and the
  crate-to-module renames live in `data/overlay-policy.toml` rather than in
  match arms.
- Example stack policies `examples/stacks/eessi-python-extras.toml` and
  `examples/stacks/eessi-r-extras.toml` for locking EESSI-shipped
  scientific Python / R providers.

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
