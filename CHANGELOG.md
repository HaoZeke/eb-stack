# Changelog

All notable changes to this project are documented here.

## Unreleased

### Security

- `rustls-webpki` 0.103.13 -> 0.103.16 and `webpki-roots` 1.0.9 -> 1.0.10.
  0.103.16 rejects certificate paths that contain the distrusted eMudhra
  emSign intermediate public keys. 1.0.10 removes `emSign ECC Root CA - G3`
  from the bundled root store. The 0.26.11 compatibility crate re-exports
  that store, so the lock resolves it to 1.0.10.

### Documentation

- Keep each command span on one line on the dependency-closure page, and
  put the remaining actors next to their verbs.

- Record the dependency-closure page against the commands. The examples
  directory contains the checksum recipe, both pull-request files, the curl
  recipes, the easyblock tree, and `Tool-1.0.eb`. A test reruns every
  command on the page and diffs the output. The stack document and the
  package-plan document keep a build-only dependency off `dependsOn`.

- Tighten the dependency-closure page. Spell out SBOM, CVE, and MCP on
  first use, and rewrite the sentences a prose review flagged.
  Command lines and recorded output are unchanged.
- Explain what a dependency closure answers, with the fixture commands
  for a planned stack SBOM, a recipe bump, a checksum-dict version bump,
  the MCP schema, pull-request overlap, rebuild-set, component-reach,
  and easyblock-reach. See
  `docs/orgmode/explanation/sbom-blast-radius.org`. `stack sbom` and
  `eb_stack_sbom` read dependency edges from the easyconfigs a lock
  names.
- Reword the doc comments on `DiffRequest::pr` and `python_syntax_error`
  so the generated Rust API pages are valid RST. A span that contains
  angle brackets is left as a single backtick, and a letter glued to a
  closing backtick is not an end-string.

### Added

- `eb-stack component-reach` lists every easyconfig and installed module
  that carries a component at a version or version range, inside one
  toolchain generation, and feeds the recipes that contain it into the
  rebuild closure. Dependencies, builddependencies, bundle components,
  `exts_list` extensions, Cargo `crates`, and versioned names in `sources`
  or `patches` all count. A compat-layer file is an input, not an
  easyconfig. The output directory holds `component-reach.toml`,
  `component-reach.json`, and `component-reach.md`. See
  `docs/orgmode/howto/component-reach.org`.

- `eb-stack rebuild-set` reports which installed modules an in-place
  easyconfig patch forces EESSI to rebuild. It closes the deployed
  easystacks, walks the installed reverse dependents, and classifies each
  one from the easyconfig graph: `no-rebuild` for a runtime dependency,
  `review` for a builddependency only, and `must-rebuild` for the patched
  easyconfig and for a bundle that ships it as an extension. The output
  directory holds `rebuild-set.json`, `rebuild-set.md`, and one
  `rebuilds/<YYYYMMDD>-eb-<version>-cve-patches.yml` per EasyBuild version
  containing class `must-rebuild` only. See
  `docs/orgmode/howto/rebuild-set.org`. `--prefix` reads `lib*/*.a`
  from the installed prefix: a patched module with no static archive is
  no-rebuild for a builddependency-only dependent, and a review dependent
  is one row. `--check-static` compares defined dynamic symbols of the
  remaining review dependents with the patched library's exports and is
  off unless it is passed.

- `pr-overlap` stores each changed easystack and easyconfig on the
  record (`revisions`: path, kind, base and head text or blob ids).
  `--merge-order` reads easystack hunk collisions from that text.
- `pr-overlap --merge-order` writes `merge-order.toml` and appends a Merge
  order table to `overlap.md`. The order builds each shared module once.
  `--merged N` repeats, implies `--merge-order`, and treats those pull
  requests as already installed. Two easystack edits at the same insertion
  point, or with the same added entry text, are recorded as conflicts.
  See `docs/orgmode/howto/pr-overlap.org`.
- `pr-overlap --kind easyconfigs` reads changed `.eb` files, closes them
  over the base tree, and writes the modules the base tree does not
  provide. Other pull requests in the same run can satisfy a dependency.
  The index rows are `same-file`, `same-module`, `shared-new-module`, and
  `depends-on`. `--installed` is optional for this kind. See
  `docs/orgmode/howto/pr-overlap.org`.

- `pr-overlap` closes the easystack lines an open pull request adds and
  reports the modules more than one of them would build. `--pr` takes one
  JSON path or several after the same flag, which is what `in/prs/*.json`
  expands to. `--include-path` keeps one architecture's easystacks,
  `--filter-deps` drops dependencies the way EasyBuild does, and an alias
  in `.modulerc.lua` counts as installed when the module it names is.
  A missing easyblock class is recorded and is not counted as a module to
  build. See `docs/orgmode/howto/pr-overlap.org`.
- `eb-stack easyblock-reach` reports which easyconfigs and installed
  modules a changed easyblock or framework module reaches. Inputs are
  `--file`, a `--range` in a local clone, or `--pr` fetched as
  `refs/pull/<n>/head`. Matching covers the `easyblock` line, the implied
  `EB_<Name>` class, subclasses parsed from the Python class statement,
  extension classes (`exts_defaultclass`, `exts_classmap`, per-extension
  `easyblock`), and bundle components. `--reverse` adds the
  reverse-dependency closure. Output is `reach.toml`, `reach.json`, and
  `reach.md`. See `docs/orgmode/howto/easyblock-reach.org`.
- Sandboxed recipe file tools on `eb-stack mcp`: `eb_recipe_read`,
  `eb_recipe_list`, `eb_recipe_edit` (exact string replace; a repeated match
  is not written unless `replace_all` is set), and `eb_recipe_write`. Paths
  stay under `EB_STACK_WORKSPACE` or `eb-stack mcp --workspace` (otherwise
  the server's current directory). `..`, absolute paths outside that root,
  and symlinks that leave it are rejected.
- `recipe lint` and `eb_recipe_lint` report what upstream checks, without
  relying on `eb --check-contrib` (which never reports F541): E999 for a file
  that is not valid Python; the easyconfigs CI lint (F541, W605, W291); the
  check-contrib pycodestyle codes (E501, E225-E228, E231); the easyconfigs
  test-suite checks (`EB_PY_NO_USER_SITE`, `EB_SANITY_PATHS_SHAPE`,
  `EB_LOCAL_VAR_NAMING`, `EB_REDEFINED_PARAM`, `EB_CHECKSUM_COUNT`); and
  review conventions (`EB_FMT_PERCENT`, `EB_FMT_CONCAT`, `EB_FMT_MISSING_F`,
  `EB_FSTRING_QUOTE_REUSE`, `EB_FMT_TEMPLATE_VAR`, `EB_TEMPLATE_UNKNOWN`,
  `EB_SHLIB_HARDCODED`, `EB_DEP_DUPLICATE`, and `EB_FMT_TEMPLATE_REMOVED`
  with `--base` / `base`). Every finding has a severity (error or warning)
  and the output has a `clean` flag, false while any error or any `EB_FMT_*`
  formatting finding remains (`error_free` reports errors only); the CLI
  exits non-zero on errors only. Each EB_FMT_PERCENT finding shows the
  f-string built from the flagged expression. `EB_DEP_EMPTY_VERSION` warns on
  a (build)dependency with version `''`. EB_TEMPLATE_UNKNOWN knows the
  extension-filter templates (`%(ext_name)s`, `%(ext_version)s`, `%(src)s`)
  and easyblock-added ones, and skips Python `%`-dict keys; EB_CHECKSUM_COUNT
  counts easyblock patch options such as `patch_ctypes_ld_library_path`;
  EB_SHLIB_HARDCODED flags `.so` only in library paths and sanity checks. `f'lib/%(name)s.{SHLIB_EXT}'` is correct and
  is not flagged. When `eb` is on `PATH`, the same command runs
  `eb --check-contrib` and merges its findings. A missing `eb` is not a lint
  failure by itself. `fixtures/lint_*/` holds one bad/good/expected.json
  fixture per rule, enforced by `tests/lint_fixtures.rs`.
- Formatting findings are scoped. `EB_FMT_*` findings outside the review
  scope carry `"info": true` and do not count against `clean`: untouched
  `%%(...)s`-escaped `%` entries, and (with an explicit `base` / `--base`)
  lines unchanged from the base. `review_lines` keeps named lines in scope.
- New errors: EB_FSTRING_ESCAPED_TEMPLATE (`%%(...)s` inside an f-string),
  and in diff mode EB_SANITY_ENTRY_REMOVED and EB_EASYBLOCK_CHANGED, plus the
  EB_SANITY_PATHS_REMOVED warning. `eb_recipe_lint` uses the file as the
  server first saw it in the session as the diff base when `base` is not
  given (`rebaseline` resets it). EB_DEP_EMPTY_VERSION is now an error.
- `eb_dep_versions` lists the versions of a dependency that have an
  easyconfig in a local tree, for a toolchain and for other toolchains, and
  says so (instead of guessing) when there is none or no tree at all.
- `eb_recipe_format` rewrites the recipe in place, returns the text with
  `dry_run`, and writes `out` only as a `.eb` path inside the workspace whose
  directory exists, so a review fix leaves no stray files.
- `eb_recipe_grep` searches files inside the workspace root (literal or
  regex) and returns path, line, and text per match.
- `recipe check` and `eb_recipe_check` fail on a recipe that is not valid
  Python. `eb_recipe_check` accepts a `.eb` file in `easyconfigs` as the
  easyconfigs directory it sits in, and reports a clear error for other
  non-directory paths.
- `eb_recipe_check` requires `easyconfigs` to contain at least one path
  (`minItems: 1`). An empty list returns
  `argument easyconfigs requires at least one path`.

- `package plan` and `package bump` raise a `missing-easyblock` residual
  (stage `emit`, severity `judgment`, id `missing-easyblock:<class>`) when a
  package resolves to a generic easyblock and carries variant options that
  differ between profiles, hand-written test or sanity parameters, or a
  binary name that follows the profiles. `--emit-easyblock` (MCP argument
  `emit_easyblock`) renders an easyblock skeleton to
  `easyblocks/<letter>/<module>.py`, ships it with the recipes, and moves the
  variant options out of the emitted easyconfigs. See "When a package owes an
  easyblock" in `docs/orgmode/reference/package-bundles.org`.

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
- `package plan` generates a recipe with the structure upstream writes.
  Pointed at a PyPI package at `GCCcore-14.2.0`, the emitted file is
  byte-identical in shape to upstream's own recipes: `PythonPackage` rather
  than a one-entry bundle, `sources = [SOURCE_TAR_GZ]`, a `binutils` build
  dependency, short lists inline, a one-line description in one pair of quotes,
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

- A package-plan SBOM keeps a build-only, host-only, or test-only
  dependency off `dependsOn`. Those refs are
  `eb_stack:buildDependsOn` on the root, the same convention a stack
  SBOM uses. A dependency with no role, or with a run role, stays on
  `dependsOn`.
- `stack solve --sbom-out` reads dependency edges and source checksums
  from the easyconfigs the lock names, the same document `stack sbom`
  writes for that lock.
- A relative `easyconfig_path` in a stack lock is the recipe beside the
  lock file when that file exists, and otherwise the same path against
  the process directory. `stack sbom` and `eb_stack_sbom` share that
  lookup and report `recipes_read`, `recipes_unreadable`, and
  `with_artifact_facts`.
- `eb_stack_sbom` reads dependency edges and source checksums from the
  easyconfigs a lock names, the same way `eb-stack stack sbom` does. A
  runtime dependency is a CycloneDX `dependsOn` edge. A builddependency
  is `eb_stack:buildDependsOn`. An unreadable path still contributes
  nothing, and the document does not invent edges between every package.
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
