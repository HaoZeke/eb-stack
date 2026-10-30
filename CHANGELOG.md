# Changelog

All notable changes to this project are documented here.

## Unreleased

### Fixed

- A version bump moves the file name in a positional checksum's trailing
  comment to the new version.
- A package-config layer parameter replaces the recipe's whole value: the
  old value's `+=` continuations are removed, and a `concat` value that does
  not fit one line is written as `key =` then `key +=` lines.
- `pr-overlap --kind easyconfigs` applies `--filter-deps` before the
  reverse walk. A filtered dependency such as Java or CUDA is left out of
  `reverse_dependents` and does not create a `depends-on` row.
- `pr-overlap --kind easyconfigs` puts a patched easyconfig, and every
  reverse dependent classed `must-rebuild`, into `to_build`. `review` and
  `no-rebuild` stay on the record and out of the build set. `--merge-order`
  and `--impact` schedule that set. A pull request that patches a module
  is ordered before a pull request that builds a dependent of it.
- Tests that write a script and then execute it write the file from a
  short-lived shell. A sibling fork can no longer leave that script
  busy at exec (`Text file busy`, ETXTBSY).
- `stack order` reads `EASYBUILD_FILTER_DEPS` when `--filter-deps` is
  omitted, and the flag wins when it is passed. The filtered dependency
  and the dependencies it would have pulled stay out of the order.
- `stack order`, `pr-overlap`, `rebuild-set` and the reach commands do
  not resolve a bare dependency tuple to a SYSTEM recipe. An explicit
  SYSTEM tuple still does, matching `stack solve` and `recipe check`.
- The build list names every locked module, including two versions of
  one name at one toolchain, and lists runtime and build dependencies
  first. It is keyed by name, version, versionsuffix and toolchain,
  the same identity as the closure SBOM.
- `--merged` and `--impact` say a pull request has no record when it
  is in the input but changes no easyconfig and no easystack. A
  number that is not in the input is still reported as unknown.
- `stack solve` names the SYSTEM tuple when a dependency exists only
  there, the same hint `recipe check` prints.
- `stack solve` does not resolve a bare dependency tuple to a SYSTEM
  recipe. A stack whose only candidate is at SYSTEM is rejected unless
  the tuple names SYSTEM or the recipe itself is SYSTEM.
- A closure SBOM keys each component, its checksums and its dependency
  entry by name, version, versionsuffix and toolchain. Two versions of
  one name keep their own edges and hashes.
- An unreadable easyconfig tree prints its operating-system error
  once.
- `campaign finding claim` removes its lock guard when the claim
  ends, including when the state file is missing.
- `--merged N` exits 2 when pull request N has no overlap record,
  the same rejection `--impact` already used, and writes no output.
- `recipe check --recipe` and `recipe format` name the recipe when
  the file cannot be read.
- `recipe check` says a bare dependency tuple does not reach SYSTEM,
  and suggests `('Name', 'version', '', SYSTEM)`, instead of listing
  SYSTEM as a hierarchy member or a generation.
- A missing `--lock` names that path once.
- `component-reach` rejects a purl type with no carrier model before
  it creates `--out-dir`.
- An empty `--toolchain-name`, or an empty `toolchain_name` in a
  Model Context Protocol (MCP) call, is a usage error. 0.3.0 kept the
  empty name: inspect wrote a manifest with no toolchain name, and
  bump failed later on toolchain `-<version>`.
- MCP `policy` warns and is still read, the same way the CLI treats
  `--policy`. `stack_policy` is the spelling to pass.
- MCP `merged` and `impact` reject a bare number. They take an array
  of pull-request numbers, the same shape as the repeated CLI flags.
- The Sphinx build does not start when Org export left
  `docs/source/index.rst` missing or empty. Rust API generation
  deletes only the generated tree under `docs/source/crates`.
- Org export to reStructuredText resolves deferred element values
  before ox-rst reads them, and fetches ox-rst
  `b73eff187eebac24b457688bfd27f09eff434860` into a cache outside the
  tree after checking its sha256. `emacs -Q` keeps a user-installed
  Org or ox-rst from changing the build. A page that fails names its
  file and the export exits non-zero. Org 9.7 stores `~code~` text as
  an `org-element-deferred` object; ox-rst passes that object to a
  string function.
- A foreign package plan no longer treats a SYSTEM bootstrap recipe
  (`CMake-3.18.4`, `pkgconf-1.8.0`) as the dependency for a generation
  that already has that tool on GCCcore. A dependency that names the
  SYSTEM toolchain still uses the bootstrap recipe. A name that exists
  only at SYSTEM, such as Java or CUDA, still resolves there, and the
  emitted recipe writes it `('Java', '17', '', SYSTEM)`. 0.3.0 wrote
  `('Java', '17')`, which EasyBuild reads as a build with the recipe's
  own toolchain.
- `stack solve` labels a baseline parsed from easyconfigs with that
  baseline's toolchain (`foss-2025a`), the same shape as the solved
  generation label.
- The build list and stack diff in `docs/orgmode/howto/emit-reports.org`
  are the files `stack solve` writes for the GROMACS fixture, including
  Python and OpenMPI 5.0.3.
- An easystack baseline uses the same toolchain label as the solved lock
  (`foss-2025b`), not `baseline-from-easystack-foss-2025b`.
- Recipe lint modules live under `src/eb_style/` so `sphinx-rustdocgen`
  can open them. A `#[path]` attribute is joined onto the parent file
  and the generator then fails to read the module. A pull-request ref
  is written `refs/pull/N/head`.
- `eb-stack` prints a failure as `error:` and the cause chain. A missing
  file or a rejected value no longer appends a stack backtrace.
- A rejected flag or flag combination exits 2. A missing file, a parse
  failure, or a failed check exits 1. See the exit status table in
  `docs/orgmode/reference/cli.org`.
- Input failures name the flag, the path, and the value. A pull-request
  record that is the wrong shape names `--pr` and the file. `--version`
  and `--generated-at` that do not parse are rejected. A `--changed`
  path that is not a file is not treated as an easyconfig name. A write
  that fails names the path.
- `target doctor` reports a probe that cannot be started as a failed check
  and still prints the other three layers. A missing `podman` or
  `eessi_container.sh` used to abort the report before any check was shown.
- Profile locks keep a standard robot path from `easybuild/easyconfigs/`
  and otherwise only the recipe filename. A relative `--easyconfigs` path
  no longer records the checkout prefix.
- Campaign finding summaries name the failure class and the numeric exit
  status. A command that exits 1 reads as exit 1, and a probe that never
  starts reads as having no exit status.
- `eb_package_plan` reads `package_index`, and `eb_package_bump` honors
  `strict_patches`. Both flags already existed on the command line.
- `eb-stack easyblock-reach` keeps an `easyblock` key on a `components` or
  `exts_list` assignment the parser had to skip. GFE's component list is a
  comprehension plus a tuple, and that tuple names `CMakeMake`.
- `eb-stack easyblock-reach` follows a nested easyblock class and a
  module-level name bound to a class. QuantumESPRESSO's
  `EB_QuantumESPRESSOcmake(CMakeMake)` is nested inside
  `EB_QuantumESPRESSO`, so a change to `CMakeMake` lists that class and
  the QuantumESPRESSO easyconfigs. A framework module still reaches the
  easyblocks that import it and their subclasses.
- `eb-stack pr-overlap --kind easyconfigs` counts `depends-on` only when
  the added module is new to the base tree. An added file for a module
  the base already provides is not a dependency of another pull request.
- `eb-stack easyblock-reach` follows imports through
  `easybuild.framework` and `easybuild.tools`. A change to `tools/run.py`
  reaches an easyblock that imports `tools/filetools.py` when that module
  imports `tools/run.py`. An easyblock that only imports another easyblock
  is not reached that way.
- The tutorial and how-tos run from this checkout. Upstream EasyBuild
  trees come from `scripts/fetch-upstream-trees.sh`. `pr-overlap`,
  `component-reach`, `rebuild-set`, the GROMACS bump, the easystack
  diff, and the Spack externals script use files in this checkout. A
  missing EasyBuild program is `examples/targets/missing-easybuild.toml`;
  claiming its finding is the repair how-to.
- `recipe templates --help` says `--export` prints the constant table and
  the derived-key rules, and that `--name` with `--version` prints the
  resolved values. `EB_DEP_EMPTY_VERSION` is an error; the command
  reference no longer lists it as a warning.
- `recipe easyblock` uses the framework character map. The `filetools`
  keys written `r'\''` and `r'\\'` are two-character strings, `\'` and
  `\\`. `encode_string` looks up one character at a time, so `'` and `\`
  pass through. Decoding `_apostrophe_` and `_backslash_` writes those
  keys back.
- `recipe lint` JSON includes `info` and `in_review` when a finding sets
  them, the same fields `eb_recipe_lint` already returned. An
  `EB_FMT_PERCENT` hit on a `%%(...)s` escape is info and does not count
  against `clean`.
- `recipe lint` accepts the template names EasyBuild defines: the
  `*minver` forms (`pyminver`, `javaminver`, `perlminver`, `rminver`),
  `cuda_cc_gencode`, and the version aliases (`ver_maj`, `ver_maj_min`,
  `ver_min`, `ver_min_patch`, `ver_patch`, `version_prefix`,
  `version_suffix`). Triton was reported as `EB_TEMPLATE_UNKNOWN` for
  `%(pyminver)s`.
- `EB_CHECKSUM_COUNT` counts `postinstallpatches` with the other patches,
  and counts sources a file appends in a loop or builds with a
  comprehension. A length that is only known after the file runs, including
  a branch on `ARCH`, is left alone. libtool, Geant4-data, Quantum
  ESPRESSO, glslang and the model recipes were reported as missing
  checksums they have.
- `recipe lint` no longer reports `EB_TEMPLATE_UNKNOWN` for an even run of
  `%` in front of `(name)s`. EasyBuild preserves `%%(source)s` so the
  easyblock can format it; STREAM's `cmds_map` is that pattern.
- `package plan` against an overlay that is not a full robot names the
  missing package, the trees it scanned, and `--easyconfigs` or
  `--easybuild-source` with an upstream easybuild-easyconfigs tree from
  `scripts/fetch-upstream-trees.sh`.
- An unknown `criteria` entry fails the solve. It used to be dropped, so a
  typo such as `-newest` solved with no optimisation criterion at all and
  the lock recorded an empty list.
- `stack-diff-out` with no baseline names `--baseline-easyconfigs` and
  `--baseline-easystack`. The previous text told the caller to pass
  `--baseline`, which the command line does not accept.
- `eb_campaign_run` reports a failed campaign as a tool error. The CLI
  exits 1 after printing the state. The MCP result now sets `isError`
  and returns that same state.
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
- Every easyconfig upstream publishes now parses: 0 skipped of some 20,600,
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
- `component-reach` honours the purl type. `pkg:cargo/curl` and
  `pkg:npm/curl` do not match the cURL easyconfig. A purl type with no
  carrier model exits 2.
- An Open Source Vulnerabilities (OSV) range that is not SEMVER or
  ECOSYSTEM, or an event that does not parse, is reported. A package
  whose ranges were all unparsed counts as review.
- A Common Vulnerabilities and Exposures (CVE) `component-reach` run
  sums closure counts into the summary. `changed`, `must-rebuild`,
  `review`, and `no-rebuild` come from the carriers.

### Security

- `rustls-webpki` 0.103.13 -> 0.103.16 and `webpki-roots` 1.0.9 -> 1.0.10.
  0.103.16 rejects certificate paths that contain the distrusted eMudhra
  emSign intermediate public keys. 1.0.10 removes `emSign ECC Root CA - G3`
  from the bundled root store. The 0.26.11 compatibility crate re-exports
  that store, so the lock resolves it to 1.0.10.

### Documentation

- Crate keywords are `easybuild`, `resolvo`, `sbom`, `hpc`, and `mcp`.
  Categories add `parser-implementations` and `config` to
  `command-line-utilities`, `development-tools`, and `science`.
  `pyproject.toml` records the same keywords and the PyPI
  classifiers. The README points at `llms.txt`, `AGENTS.md`, the
  skills, and the Model Context Protocol setup.

- Each HTML page takes its meta description from the first body
  paragraph. `robots.txt` names the sitemap. The build copies
  `llms.txt`, `llms-full.txt`, and each page's markdown into the
  site, and a page that has markdown links that file with
  `rel="alternate"`.

- `server.json` is the Model Context Protocol registry manifest for
  cargo package `eb_stack`. The manual page `reference/mcp` records
  the stdio client configuration. The README includes the visible
  `mcp-name: io.github.HaoZeke/eb-stack` token.

- Skills for inspect, plan, bump, a stack SBOM, pr-overlap, and the
  Model Context Protocol server live under `skills/`. Each command in
  those files is one the current `eb-stack` binary runs. `AGENTS.md`
  lists them next to the packaging skills.

- `AGENTS.md` names the gates CI runs: `cargo fmt --all --check`,
  `cargo clippy --locked --all-targets -- -D warnings`,
  `cargo test --locked --all-targets`, `cargo package --locked`,
  `uvx prek run -a`, and `pixi run -e docs docbld`.

- `scripts/gen-llms-txt.py` writes `llms.txt` and `llms-full.txt` from
  `docs/orgmode`, plus one markdown file per page. The docs build
  regenerates the page files. They are not stored in git. `llms.txt`
  is the index from the llms.txt specification: a title, a short
  summary, and lists of links to those pages. `cargo test --test
  llms_txt` fails when the committed files drift from the manual.

- Reference and explanation pages use the same checkout checks as the
  tutorial and how-tos. Bash examples name files that exist in this
  tree or under `/tmp`, and the pages drop the wording those checks
  reject.

- Walkthrough commands write build lists, locks, SBOMs, diffs, order
  files, and hash files under `/tmp`.

- The pull-request how-to runs on recorded software-layer pull requests
  1585 and 1728 and on the recorded open easyconfigs pull requests.
  The reference example uses those same files.

- The dependency-closure page runs on recorded slices of
  easybuild-easyconfigs develop `981d2123`, the v5.4.1 easyblocks,
  framework, and easyconfigs tags, the conda-forge zlib feedstock at
  `e34000ac`, and the Spack zlib package at `a19611de`.
  `docs/examples/sbom-blast-radius` is removed. `tests/sbom_page_drift.rs`
  reruns every command on the page.

- The easyconfigs overlap how-to names `pr-overlap --open` as the command
  that fetches pull requests, and its merge-order heading names the
  easyconfigs depends-on edges. `fetch-prs.sh` is the collector for a
  clone that already has `gh` and git.

- The PyPI and Comprehensive R Archive Network (CRAN) how-tos describe
  the easyblocks the planners emit. A CRAN plan keeps
  `easyblock = RPackage` and emits a `Bundle` recipe. The command map
  includes `repro summary`. The idle campaign example omits
  `current_recipe`, which the state file omits when no recipe is
  active.

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

- `docs/orgmode/howto/easyconfigs-pr-overlap.org` records that easyconfigs
  run: the four pull requests, the develop commit, and the shared
  Mathematica module. `--impact` is documented on the software-layer
  how-to, the CLI page, the format page, and the merge-order explanation.

### Added

- `eb-stack stack arch` refuses a plan that includes, or depends on, an
  entry an architecture cannot build. The message names the entry and
  the architecture. `data/arch-unsupported.toml` is the list: GDRCopy on
  riscv64 (software-layer #1775 and #1822) and bwa-mem2 on aarch64
  (#1876). With `--installed`, an entry in a shared easystack that has
  never been installed on that architecture, and is not in its
  `additional_arch` easystack, is flagged. `stack batch` uses the same
  list before it writes a plan. `eb_stack_arch` runs the same check.
- `package bump --fetch-checksums` decides three more patch cases from the
  new source: a patch that only creates files the release already contains, a patch
  whose changes the release carries under different surrounding lines, and
  a patch that only pins `bazel_dep` modules `MODULE.bazel` declares itself.
- `package bump --fetch-checksums` replaces a patch the new source leaves
  undecided with a rebased file named for the new release
  (`<Name>-<version>_<description>.patch`) found beside the source recipe or
  under `<letter>/<Name>` of an `--easyconfigs` root, when that file applies.
- `package bump --drop-patch NAME=REASON` records an operator's patch drop
  with its reason; `--latest-secondary` moves `local_*`-versioned later
  sources (gc's `libatomic_ops`) to their newest stable release. The MCP bump
  tool takes `drop_patch` and `latest_secondary`.
- `eb-stack stack from-commit` reports each `from-commit` and
  `include-easyblocks-from-commit` pin already contained in the EasyBuild
  release named by `--easybuild-version` or by `eb-X.Y.Z` in the easystack
  file name. A released pin whose easyconfig blob matches the tag can be
  removed. A released pin whose blob differs needs a person. The command
  exits 1 when any pin is already in the release. `eb_stack_from_commit`
  runs the same check.
- The `eb_component_reach` schema lists `osv_cache`, which the CLI
  accepts as `--osv-cache`.
- `eb-stack pr-overlap --impact N` writes `merge-impact.toml`. For that pull
  request alone, it names the other open pull requests whose build set
  shrinks and the depends-on edges from it that resolve.

- `scripts/fetch-upstream-trees.sh` creates a checkout of pinned easybuilders
  easyconfigs, easyblocks, and framework trees. The `Upstream trees`
  workflow sources that cache and runs `cargo test --include-ignored`
  every week, on a push that changes the pinned tree list in that
  script, and by hand.

- `eb_stack_order`, `eb_stack_easystack`, `eb_stack_sections`,
  `eb_repro_summary`, `eb_recipe_easyblock`, `eb_recipe_templates`, and
  `eb_recipe_hierarchy` call the same runners as the CLI commands. Each
  tool's schema lists that command's flags. A test fails when a CLI
  subcommand has no MCP tool.

- `eb_pr_overlap`, `eb_rebuild_set`, `eb_component_reach`, and
  `eb_easyblock_reach` call the same runners as the CLI commands.
  `eb_pr_overlap` takes `kind` `software-layer` or `easyconfigs`,
  an easyconfigs closure, a merge order, and `--impact`.
  The reports written under `out_dir` match the CLI byte for byte.

- `package bump --fetch-checksums` downloads each `sources` entry from the
  recipe's `source_urls` when the version changed and `--source-checksum`
  was omitted, and writes the SHA-256 into the recipe and the SBOM. A git
  checkout, a missing URL, or a failed download stays empty. The attempts
  are `sources.fetch.json` and a `source:fetch` residual.

- `eb-stack stack order` sequences every pinned build a set of roots needs.
  A name the generation builds twice stays twice. `--oldest` takes the
  oldest admissible version of an unpinned requirement. `--easystack-out`
  writes that sequence as an easystack. `--hashes-out` writes one sha256
  per module over the easyconfig and the hashes of what it needs.

- `eb-stack stack easystack` writes a lock as the YAML `eb --easystack`
  reads. `--option <file.eb>:<option>=<value>` sets a per-file `eb` option,
  spelled without the leading dashes.

- `eb-stack stack sections` regroups a Jenkins build list under the
  `#### Title ####` headers of a previous list. Entries keep their flags
  and whether they are commented out. `--unfiled` names the header for
  packages the previous list does not file.

- Packages on that Jenkins build list still need a reproduction score.
  `eb-stack repro summary` reads the per-case JSON under `--scores` and
  prints the scoreboard table: case, pull request, `EXACT` / `SEMANTIC` /
  `MATERIAL`, allowance, and residual lines. `--json` prints the case
  records. `--ratchet` checks the run against the committed allowance
  counts and exits nonzero when they disagree.

- MCP easyblock tools: `eb_module_lint` (E999, E501, whitespace,
  `EBM_RUN_CMD`, `EBM_OLD_SUPER`, `EBM_CFG_OVERRIDE`,
  `EBM_TEST_STEP_OWN_CMD`, `EBM_THREADED_VARIANT`), `eb_easyblock_dry_run`
  (`eb -x`, per-step commands, toolchain options, threaded findings, and
  `failure_kind` of `setup`, `environment`, or `easyblock`), `eb_build_log`,
  and `eb_build_test` (only when `EB_STACK_ALLOW_BUILD=1`).
  `eb_recipe_apply_suggestion` puts a review suggestion on the anchored
  lines. `eb_recipe_diff` reports each hunk with the top-level name it
  touches.

- `pr-overlap --kind easyconfigs --open OWNER/NAME` reads the open pull
  requests of that repository over HTTPS from the public GitHub API and
  adds them to any `--pr` files. `--base-ref` is the git ref for their
  base text and defaults to `develop`. `eb_pr_overlap` takes the same
  `open` and `base_ref` arguments. Head text comes from
  `raw.githubusercontent.com` at the head commit. A 404 on the base ref
  means the file was added. No token is sent, and nothing from a pull
  request is executed.

- `eb-stack component-reach` lists every easyconfig and installed module
  that carries a component at a version or version range, inside one
  toolchain generation, and feeds the recipes that contain it into the
  rebuild closure. Dependencies, builddependencies, bundle components,
  `exts_list` extensions, Cargo `crates`, and versioned names in `sources`
  or `patches` all count. A Python wheel is its own row: a `.whl` filename
  in `sources` or `checksums`, an `exts_list` entry whose source is a
  wheel, or a top-level `source_tmpl` that names a wheel. A Cargo recipe
  also matches a crate that appears only as a checksum key such as
  `rustls-webpki-0.102.8.tar.gz`; the recipe's own tarball is not that
  crate. A compat-layer file is an input, not an easyconfig. The output
  directory contains `component-reach.toml`, `component-reach.json`, and
  `component-reach.md`. See `docs/orgmode/howto/component-reach.org`.
  `--purl` takes a package URL such as `pkg:cargo/rustls-webpki@0.102.8`.
  `--cve` resolves an OSV id, follows GitHub Security Advisory
  (GHSA), RUSTSEC, PYSEC, and GO aliases when the CVE record has
  only a git range, and keeps named
  version tags such as `libXpm-3.5.17`. Several affected packages are
  one document with `kind` `cve`. `--easystack-url` reads a deployed
  easystack over https. `eb_component_reach` on `eb-stack mcp` runs
  the same query.

- `eb-stack rebuild-set` reports which installed modules an in-place
  easyconfig patch forces EESSI to rebuild. It closes the deployed
  easystacks, walks the installed reverse dependents, and classifies each
  one from the easyconfig graph: `no-rebuild` for a runtime dependency,
  `review` for a builddependency only, and `must-rebuild` for the patched
  easyconfig and for a bundle that includes it as an extension. The output
  directory contains `rebuild-set.json`, `rebuild-set.md`, and one
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
  JSON path or several after the same flag, matching what `in/prs/*.json`
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
  f-string built from the flagged expression. `EB_DEP_EMPTY_VERSION` is an
  error on a (build)dependency with version `''`. EB_TEMPLATE_UNKNOWN
  knows the
  extension-filter templates (`%(ext_name)s`, `%(ext_version)s`, `%(src)s`)
  and easyblock-added ones, and skips Python `%`-dict keys; EB_CHECKSUM_COUNT
  counts easyblock patch options such as `patch_ctypes_ld_library_path`;
  EB_SHLIB_HARDCODED flags `.so` only in library paths and sanity checks. `f'lib/%(name)s.{SHLIB_EXT}'` is correct and
  is not flagged. When `eb` is on `PATH`, the same command runs
  `eb --check-contrib` and merges its findings. A missing `eb` is not a lint
  failure by itself. `fixtures/lint_*/` contains one bad/good/expected.json
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
  `easyblocks/<letter>/<module>.py`, includes it with the recipes, and moves the
  variant options out of the emitted easyconfigs. See "When a package owes an
  easyblock" in `docs/orgmode/reference/package-bundles.org`.

- Overlay planning treats existing robot modules as leaves: a `Python`
  easyconfig that names `binutils` no longer makes `package plan --format pypi`
  unsatisfiable. `--easyconfigs` is the solve robot only; it does
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
  that already includes them is an empty delta. Without that provider the
  plan refuses a pip overlay instead of emitting a `PythonBundle`.
  Warehouse `requires_dist: null` (live numpy JSON) parses as no extras.
  A leftover such as `eon-akmc` whose other PyPI deps are not in the
  robot keeps those names in `exts_list` instead of failing the
  Boolean satisfiability (SAT) solve.
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
  identical in shape to upstream's own recipes: `PythonPackage` rather
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

- `tests/property` backtests the ordering properties against one commit per
  year of easybuild-easyconfigs, materialised with `git archive`. Sampled
  roots that order successfully across 26 commits from 2014 to 2026:
  312/312.

### Tests

- `tests/pr_overlap_easyconfigs_open` replays four open easybuild-easyconfigs
  pull requests (22823, 24575, 27131, 27215) against develop `981d2123`. The
  overlap files match the run on the full tree. The ignored test
  `live_github_open_easyconfigs_pull_request` reads the public GitHub API.

### Changed

- `stack solve --stack-diff-out` writes ASCII: `to` between the
  baseline and solved labels and between versions, parentheses around an
  easyconfig path, and `none` for a missing value.

- `recipe hierarchy` without `--export` prints only the known
  generations. `--export` also prints the synthetic GCC-family rule.
  The plain command used to print both.

- Package commands, `stack solve`, the MCP tools, and
  `skills/golden-replay/replay.sh` pass `--toolchain name/version` and
  `--stack-policy`. The older spellings stay accepted, are omitted
  from `--help`, and warn on the CLI and in the MCP result when the
  call accepts them. The MCP package tools require a toolchain through
  the schema's `anyOf`, and a call without one names `toolchain`.
- The CLI and the MCP tools share one usage error, one toolchain
  parser, and one foreign-format parser.
- `--toolchain system` and `--toolchain SYSTEM` select the SYSTEM
  toolchain. `system/` is the same value. The version is empty.
- `--help` for every command includes an example, and every flag has a
  description. Defaults the command applies are shown there.
- `stack solve` reads the policy file with `--stack-policy`, the same flag
  as `package plan` and `package bump`. Package commands take
  `--toolchain name/version`, the spelling `stack solve` uses. An empty
  version is kept: a SYSTEM recipe has one. A command line without a
  toolchain version is still rejected. Inspect and plan accept a lone
  `--toolchain-version` and use `foss`. Bump does not.
- MCP package tools accept `toolchain` as `name/version`. `eb_stack_solve`
  reads the policy file as `stack_policy`.
- Help examples name `zlib`. Production source does not name a package
  the solver used to special-case.
- The Rust API toctree is added once, by the Sphinx build.
- The stack docs match the commands: build-list order and versions, the
  stack-diff labels and punctuation, `solver.criteria`, lock-SBOM
  `dependsOn` edges, policy hierarchy membership, and `stack order`,
  `stack easystack`, and `stack sections`.
- Package plan docs list the closure files every `--easyconfigs` plan
  writes. CRAN inspect records `RPackage`. A single PyPI plan emits
  `PythonPackage` and depends on the bundle that already provides a
  requirement. A Cargo.toml with no crate tarball is refused at plan
  time; the crates.io JSON is what plan emits from.

- The checked-in zen4 overlap report is the 2026-10-24 cvmfs run: 28 open
  pull requests, 20 easystack records, 10 shared modules. Closed pull
  request 1728 is gone and 1692's head is `655310721787`.

- The European Environment for Scientific Software Installations
  (EESSI) 2025.06 component-reach test is ignored unless
  `scripts/fetch-upstream-trees.sh` has checked out the pinned trees.
  The default test run does not clone or write into the source tree.
- `scripts/fetch-upstream-trees.sh` exports one name per pinned tree
  and records the EESSI software-layer commit the ignored test reads.
- The component-reach how-to runs on a recorded slice of pinned
  easybuild-easyconfigs develop `981d2123`, licence file included.
  The easyblock-reach how-to uses the same fetch script.
- The easyconfigs `pr-overlap` example in the CLI reference is a run
  that writes 4 records, on the recorded open pull requests.
- CI runs every integration test file. Ignore-only files are named by
  the upstream-trees workflow, which passes `--include-ignored`.
  `real_tree_scale` still needs a local robot tree and is not in that
  list.

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
