# Changelog

All notable changes to this project are documented here.

## 0.7.1 - 2026-11-08

### Changed

- The crates.io package carries the sources, the TOML tables they include,
  the README, LICENSE, CHANGELOG and CITATION. The test corpus, fixtures,
  manual, skills, examples and delivery bundles stay in the repository;
  the 0.7.0 archive was 1589 files and 13.8 MiB, most of it tests.

### Docs

- The manual and README describe the 0.7.x GitHub release as carrying
  every command, and crates.io alone as still 0.3.0; the Org pages that
  print a version are on the release checklist; CONTRIBUTING names the
  publish command.

## 0.7.0 - 2026-11-08

The behaviours the `develop` line carried and main lacked, ported one test
at a time (120 tests by name, all green on main now).

### Added

- `package bump --foreign RECIPE` (MCP `foreign`): a Spack `package.py` or
  conda-forge recipe whose dependencies the bump inspects for names the
  source `.eb` never declared, mapped through the package-config aliases.
- Package config: the alias table form rejects unknown keys; empty alias
  and virtual names or values, and a patch `sha256` that is not a SHA-256,
  fail the layer; `exclude_from_solve` matches the post-alias provider; a
  child profile may be listed before its parent in the same layer; a
  requirement updates every row that maps to its name and keeps an
  existing alias.
- Package catalog: an exact version wins over an unversioned sibling,
  names and versions compare trimmed, a blank `source` is a missing-source
  error, replacing `source` resets an inherited `format` and drops
  inherited checksums, switching to `easybuild-bump` drops foreign
  checksums, and `source_checksums` must be SHA-256 digests.
- LuaRocks: `~>` is a series range, `~=` a not-equal, `build_dependencies`
  are build edges, a `git+` source is a git remote with its tag, `hash`
  counts only as a 64-hex SHA-256, `external_dependencies` are residuals,
  and the plan's easyblock is `LuaRocks`. Raku: `ver<0.10+>` is `>=0.10`
  and the `ver`, `auth`, `api` and `from` adverbs are not the name.
- A git checkout source (a commit, or a remote with no download URL)
  emits a `git_config` dict.
- A dependency that spells the module, `('OpenMPI', '5.0.3-GCC-13.3.0')`,
  is an exact pin on that build, from a SYSTEM recipe too; it is not a hole.
- Stack pins and exclusions apply to every toolchain-qualified key of a
  name; a locked pin on `Perl` constrains the GCCcore and SYSTEM builds.
- The bump emits the plan's default profile; a layer's `[package] version`
  reaches the recipe while `--version` wins; a dependency stated in both
  lists is rewritten when either disagrees with the lock; a vanished
  dependency is removed wherever its tuple sits on the line; a sibling's
  `patches` and `checksums` are adopted with the emitted source digest
  kept and the plan's patch set rebuilt; `local_host_arch`, `local_order`
  and `local_equations` derive `local_binary` and parameterised
  `configopts`.
- PyPI and PEP 518 requirements gated by an environment marker are opaque
  conditions, solver-excluded with a `pypi-marker` residual; build
  backends and overlay build tools are named only when the plan's own
  toolchain hierarchy provides them; a language bundle renders its
  patches, their checksums and configopts; the request's package index
  reaches the plan in prepare.
- Closure: holes carry their versionsuffix into discovery, the companion
  key and the cycle path, so `-MPI` and `-CUDA` builds are two companions;
  an unversioned catalog wildcard defers to a unique versioned sibling; a
  PyPI or CRAN root the robot already provides closes without companions.
- SBOM: only a 64-hex digest is a SHA-256 hash, the root hash is the first
  source that has one, overlay extensions are library components, and the
  GitHub archive URL is derived only from a github.com remote in any
  spelling.
- CRAN sanity dirs sit under `lib/R/library/<name>`; a Spack
  `type=("build", "link")` dependency is a runtime edge.

### Changed

- A suffix or module spelling of a candidate answers only an exact
  requirement. Against a range it compared as a pre-release of its
  version, and `<5` admitted a `5.0.0-GCCcore-13.3.0` build.
- A GitHub archive whose repository differs from the package name spells
  the account and repository in `source_urls`; `GITHUB_SOURCE` would 404.
- An uppercase declared patch digest verifies, a non-UTF-8 patch copies
  into the bundle, overlay claims key on the content digest, and a
  `dummy`-spelled source toolchain bumped onto `system` keeps its exact
  dependency pins.
- A runtime extra that collapses onto a bundle a build-only extra already
  locked keeps the bundle off the build-only list.

### Fixed

- An MCP `eb_package_bump` call without a `foreign` array no longer fails
  with "argument foreign requires at least one path".

## 0.6.2 - 2026-11-08

### Changed

- `rfc9162`, `rfc9162-tiles` and `rfc9162-log` come from crates.io at
  exactly 0.8.1 instead of a git pin, so `cargo package` resolves them;
  0.8.1 reads a partial tile a writer has widened through the widest one,
  which a reader at a checkpoint behind the tail needs.

### Fixed

- The stack-gap-lint delivery (report, bundle, patches) lives under
  `deliveries/stack-gap-lint/`. At `artifacts/` it sat where cargo-dist
  collects the release artifacts, and the 0.6.1 host step failed trying to
  upload `artifacts/patches` as a release asset.

## 0.6.1 - 2026-11-08

### Fixed

- The Apple-silicon release build runs on `macos-15`. The `macos-14` job
  dist asked for was cancelled the second it was created, on every
  attempt, so the 0.5.3 and 0.6.0 releases never reached the host step.

## 0.6.0 - 2026-11-08

### Added

- The tile log carries a sorted-run index of its records in a second,
  signed index log inside it at `<tiles>/index`: `ledger head` signs it under
  `<origin>/index` with the same key once a run exists, `ledger find`
  proves the record in the tile checkpoint and the run manifest in the
  index checkpoint, `audit` checks the index checkpoint's prefix, and a
  rebuild moves both logs aside. The data tile log still holds exactly the
  ledger's lines, so `ledger verify` and the signed heads are unchanged.
  The index comes from `rfc9162-log` in the rfc9162 workspace.

## 0.5.3 - 2026-11-08

### Changed

- The same code as 0.5.2. `dist host` refused the v0.5.2 release because
  dependabot had bumped the action versions inside the generated
  `release.yml`; `dist-workspace.toml` now carries `allow-dirty = ["ci"]`.
  The link check excludes every `api.github.com` URL, not only `/repos/`.

## 0.5.2 - 2026-11-08

### Changed

- The same code as 0.5.1. The v0.5.1 tag sits on a commit whose
  `Cargo.lock` and recorded version strings still said 0.5.0, so its
  release does not build with `--locked`; 0.5.2 is the release of that
  fix with every lockstep file at one version.

## 0.5.1 - 2026-11-08

### Fixed

- `ledger head` on an empty log signs the size-0 head without a
  checkpoint when no `--origin` is given, and `ledger verify` of such a
  head skips the tile-log audit. The checkpoint origin derives from the
  first record's stack and year, so an empty log has none to derive;
  the first head after a record checkpoints as before. A site can
  anchor a fresh ledger on day zero without naming an origin that the
  first record may contradict.

## 0.5.0 - 2026-11-08

### Changed

- `stack admit` follows GPFS on which inode limit governs: the soft limit
  below it, the hard limit while the grace period runs (`inodes_grace` in
  the quota document, a remaining time), the soft limit again once the
  grace has expired. The decision reports `grace` and `soft_exceeded`.
- Rust 1.90 is the minimum. `ordered-float` 5.5.1 sets `rust-version`
  1.90. `uuid` 1.29.0 sets 1.89.0. The CI minimum job builds every
  target with that compiler.
- `rfc9162` comes from crates.io at exactly 0.6.1 instead of a git
  revision of `leidarljos/deedar`. The published crate has the same
  source as revision `a749a63`, so leaf hashes, roots, and signed heads
  do not change.

### Added

- `EB_STACK_EB` names the `eb` that `recipe lint` runs for
  `--check-contrib`, and `EB_STACK_EB=none` skips that check with the
  report saying so. A host whose `eb` judges a synthetic recipe that the
  local checks accept (EasyBuild 5.4.1 refuses the lint fixtures with
  "mandatory parameters not provided in pyheader: toolchain, homepage,
  description") made the lint tests and the SBOM page drift check fail
  there; the default CI, with no `eb`, never saw it.
- `stack shard --lift-cpu-prerequisites` (opt-in) writes CPU-only
  prerequisites to `cpu-prerequisites.list` (CPU prefix) while shared
  accelerator packages stay in `prelude.list`. Classification matches
  EasyBuild 5.4.2 `dependency_names()` / EESSI's install-path hook: build
  and runtime dependencies after `--filter-deps`, plus accelerator
  toolchains — not application package-name lists. CPU-only build-list
  roots and their missing CPU closure join the CPU list; a missing
  accelerator prerequisite of a CPU package is refused by resolved
  graph identity. Default behaviour is unchanged. A lifted recipe from
  a `--from-commit` tree keeps that line's source options for hashing
  and materialization.
- `stack shard --easyconfigs-repo DIR` reads the commit of a
  `--from-commit` line from an easybuild-easyconfigs git repository: one
  `git ls-tree` per commit, and one `git cat-file --batch` for the
  easyconfigs that can decide the plan. A blobless clone serves it, and
  nothing is written to disk. `--commit-cache` serves only a commit the
  repository lacks.
- `stack shard --materialize --build-robot-path PATH` writes
  `sources/v1/<commit>/`, the files the planned closures read from each
  commit at their commit paths with each named easyconfig at the top, a
  SHA-256 manifest `sources/v1/<commit>.json`, and `prelude.local.list` and
  `shard-NN.local.list`, whose `--from-commit` lines read that directory
  through `--robot-paths` instead of fetching the commit. A line that
  cannot run that way is copied unchanged and listed with the reason.
  `prelude.list`, `shard-NN.list`, and their lines are unchanged.

### Fixed

- `eb_easyblock_dry_run` and `eb_build_test` with `robot` pass those
  directories and then the robot paths EasyBuild is configured with, as
  `eb --show-config` reports them, to `--robot-paths`. The trailing `:`
  they passed stands for EasyBuild's built-in default only, so robot paths
  set in `EASYBUILD_ROBOT_PATHS` or a configuration file were dropped.
- `stack shard` resolves a `--from-commit` or `--from-pr` line as EasyBuild
  5.4 does: the easyconfig the line names comes from the fetched tree, and
  a dependency comes from the robot path when it has a file of that name
  and from the fetched tree otherwise. The fetched tree used to win for
  dependencies too. A dependency whose fetched easyconfig differs from the
  robot path's is reported as a warning and under `shadowed` in
  `plan.json`.

## 0.4.0 - 2026-11-05

### Changed

- `scripts/gen-llms-txt.py` runs `scripts/llms_leakguard.py` before it
  writes `llms.txt` or `llms-full.txt`. A link to `AGENTS.md`,
  `CLAUDE.md`, or `GEMINI.md` fails that check unless the URL is in
  `scripts/leakguard.toml`. The manual index link
  `https://github.com/HaoZeke/eb-stack/blob/main/AGENTS.md` is allowlisted.
  `uvx prek run -a` runs the same check.

### Fixed

- `stack shard` resolves a build-list line with `--from-commit` or
  `--from-pr` against that commit's or pull request's easyconfigs first,
  then the robot path, the order EasyBuild uses. A dependency such a line
  shares with another shard goes to the prelude, so two shards do not
  build it at once. `petsc4py-3.25.0-foss-2026.1.eb --from-commit=...`
  and the `PETSc-3.25.0-foss-2026.1.eb` line it needs plan into one
  shard, PETSc first. Split over two shards, the second build stops on
  PETSc's install lock. A prelude line for a module only a
  fetched tree provides carries that line's `--from-commit`,
  `--from-pr` and `--include-easyblocks-from-*` options. A fetched line
  that still cannot be resolved stays opaque; `plan.json` gives the cause
  under `opaque_reason` and the top-level `opaque`, and the command
  prints a `warning:` line for it and `opaque=<n>` in its summary.
  A `--commit-cache` download that fails before an HTTP status exits 1
  and writes nothing; the error names the cache, the commit, the URL,
  and the transport error. An HTTP status, including 404, stays opaque.
- `stack shard` plans a build-list line it cannot resolve as an opaque
  line when EasyBuild can still build it: the line carries `--from-pr`
  or `--from-commit`, or its easyconfig is on the robot path but does
  not parse (a site recipe that computes its fields in Python). The
  line packs as its own group with the default weight and no closure,
  is written to its shard unchanged, and `plan.json` gives the reason
  under `opaque`. Any other unresolved line is still an error and
  nothing is written. A list with 22 `--from-commit` lines and seven
  site recipes failed whole before.
- `examples/easyconfigs-pr-overlap/fetch-prs.sh` and
  `examples/eessi-pr-overlap/fetch-prs.sh` request each page of open pull
  requests and of changed files, 100 at a time. The open list does not
  stop at 500. A changed-file list that stops at GitHub's cap of 3000 is
  kept: the pull request is still written, with `file_list_truncated`
  set, and the record notes `truncated at GitHub's limit`.
- `stack order` exits 1 when runtime requirements of one name at one
  toolchain have no version that satisfies every one of them. The error
  names each requester and the version it resolved. Eigen 3.4.1 required
  by LAMMPS and Eigen 5.0.0 required by SciPy-bundle, both at
  `foss-2026.1`, is the closure `stack solve` already rejects. The
  command writes no order file. It does not repin a recipe: choosing
  which exact pin to drop would describe a build the easyconfig does
  not ask for. Two builds of one name stay in the order when EasyBuild
  installs them side by side: another toolchain, a system bootstrap
  pair (binutils 2.40 and 2.42), or a build-only tool (CMake 3.31.11
  and 4.2.1). Those are the same package keys `stack solve` uses.

### Added

- `stack footprints`, `stack shard --inode-quota`, and `stack admit` plan
  a build campaign around a scratch inode quota instead of discovering
  it at admission time. `stack footprints` writes `footprints.json`, the
  peak build-tree inode count per software name: the peak of the new
  optional ledger field `build.inodes_peak` over the records of each
  `--ledger`, then each `--table` line (`Name<TAB>inodes`; the larger
  when both name a package), then a built-in class table (CP2K 350000;
  LAMMPS, PyTorch, SeisSol, NVHPC, intel 150000) over a default of 60000.
  `stack shard` writes each shard's `inode_footprint` (its largest module
  footprint plus `--inode-line-overhead`, 1000 inodes per line) and
  `max_concurrent_shards` into `plan.json`; with `--inode-quota N
  [--inode-footprints footprints.json] [--inode-margin 100000]` the cap is
  the most shards, largest first, that fit under the quota minus the
  margin, and a shard that does not fit on its own fails the run. The
  shard lists do not change. `stack admit --quota-json FILE|- --footprint
  N --job ID --reservations DIR [--margin N] [--live-jobs ID,ID]` admits a
  job when the inodes the quota still allows, minus the reservations of
  the live jobs, cover its footprint plus the margin; on admit it writes
  `DIR/<job>.json`, drops a reservation whose job is not live, never
  counts the job's own earlier reservation, prints one JSON line, and
  exits 0 or 78. The controller lists the live jobs; eb-stack calls no
  scheduler. `eb_stack_footprints` and `eb_stack_admit` are the same
  commands over MCP, and `eb_stack_shard` takes the inode arguments.
- `ledger tiles` and `ledger find`, and a tile log beside every ledger.
  `ledger append` writes each canonical line into a C2SP tlog-tiles log
  at `<log>.tiles` (or `--tiles DIR`) through the `rfc9162-tiles` crate,
  with key sidecars over `module=`, `pkg=`, `target=`, `action=`,
  `route=`, `actor=`, `id=`, and `job=` from `build.job`. `ledger find
  --key pkg=PETSc --key route=kvm` prints the records carrying every
  key, each proven in the tile log's checkpoint. `ledger head`, and
  `ledger append --key`, sign that checkpoint under the origin
  `eb-stack-ledger/<stack>/<year>` (or `--origin`) with the ledger's
  Ed25519 key. `ledger verify` also checks that the tile log extends
  the signed head with the same lines and that its checkpoint is signed
  by the same key, so a changed tile, a rewritten tree state, or a tile
  log of another ledger is status 1. `ledger tiles` writes the tile log
  for an existing ledger or brings it up to date; a tile log that
  disagrees with the ledger is moved to `<dir>.stale-<time>` and
  rebuilt. `route` is a new optional record field: `native`, `kvm`, or
  `qemu`. `eb_ledger_tiles` and `eb_ledger_find` are the same commands
  over MCP, and the other ledger tools take `tiles` and `origin`.

- `stack shard --commit-cache DIR` reads the trees of `--from-commit` and
  `--from-pr` lines from DIR and downloads the missing ones: a commit from
  `codeload.github.com/easybuilders/easybuild-easyconfigs` (its `.eb`
  files only), a pull request's changed easyconfigs at its head from
  `api.github.com`. `--commit-tree COMMIT=DIR` and `--pr-tree N=DIR` name
  trees already on disk. `eb_stack_shard` takes `commit_cache`,
  `commit_tree` and `pr_tree`.
- `ledger append`, `ledger head`, `ledger show`, and `ledger verify` keep
  a per-stack software ledger: one JSON Lines file of install, rebuild,
  retire, move, access, removal, finding, and correction records. Each
  line is canonical JSON (keys sorted, no whitespace, integers only) with
  its `index` and `id` filled in, and its leaf is the RFC 9162 leaf hash
  of the line, from the `rfc9162` crate. Heads are signed with Ed25519
  under the domain `eb-stack-ledger-v1`; the key is a PEM path passed on
  the command line and is refused inside the log's directory.
  `ledger verify --record N --head HEAD` checks an inclusion proof and
  `ledger verify --from HEAD1 --to HEAD2` a consistency proof, so a
  removed record or a rewritten log is status 1. `eb_ledger_append`,
  `eb_ledger_head`, `eb_ledger_show`, and `eb_ledger_verify` are the same
  commands over MCP.

- `stack order --installed` stops the build order at modules already on
  the machine. A file is a module listing and a directory is an Lmod
  tree, read the same way as `stack gap --installed`. The version is the
  full module version. A listed module is left out of the order, and so
  are the dependencies that were installed with it. `--filter-deps`
  still drops dependencies first. The modules that remain are chosen
  with the synthetic GCC-family rule `recipe hierarchy --export`
  prints. Stdout reports `installed=` as how many modules the walk
  stopped at. `eb_stack_order` takes the same `installed` list.
- The Model Context Protocol manual lists one tool for every CLI
  command. `docs/orgmode/reference/mcp.org` names each tool, including
  the recipe and easyblock tools that have no command. A test fails
  when a subcommand's tool is absent from that page.
- `stack shadow` reports a site-layer recipe that shadows a module the
  EESSI base stack already provides. `--base` is an Lmod tree or a module
  listing, loaded with the same inventory reader as `stack gap`. The same
  software version is `same`, another version of that name is `different`,
  and a base name with no recorded version is `unversioned`. A name the
  base does not have is not reported. The command exits 1 when any recipe
  shadows, or when a recipe does not parse. `eb_stack_shadow` is the same
  check.
- `stack provided` includes the one-based build-list line on every
  `provided` and `unparsed` finding in its text report, as it already did for
  hook, toolchain, family-lock, and licensed findings.
- `scripts/dump-module-trees.sh` captures the live site, older EESSI, and
  target EESSI `module -t avail` inventories in isolated, purged shells for
  `stack gap`.
  It sources the Lmod bash init when the shell has no `module` function.
  It exits with the module name and the Lmod error when a purge or load fails.
  It then writes no inventory for that tree.
- `stack gap` writes `version-change.tsv`, one row per version in the site
  and older-EESSI inventories. A module in the target is `moved` and names
  its EESSI version; an excluded module is `site-only`; every other missing
  route is `gone` until it lands.
- `stack solve` warns on standard error when a foss solve's baseline easystack names an LLVM-family recipe (`lfoss`, `lompi`, `lfbf`, or `llvm-compilers`) and the stack diff marks that package added.
  The foss universe cannot see that family, so the added row is the site layer by mistake.
  The solve still exits 0.
  Keep one lock per toolchain family, or pass `--baseline-also-toolchain` for that family.
  `llvm-compilers-20.1.8` and `llvm-compilers-21.1.8` are the versions `data/eessi-generations.toml` lists for generations 2025b and 2026.1.
- `component-reach` and `rebuild-set` record a reach class: `cross-user`, `network-facing`, `local-only`, `not-reachable`, or `unclassified` when no built-in name and no `--reachability` file names the package.
  `component-reach` uses the built-in names only.
  `cross-user` and `network-facing` justify a backport.
  Component-reach schema is 2.
  Rebuild-set schema is 5.
  The component-reach summary prints `reach=`.
  `rebuild-set.md` ends with the class of each patched easyconfig.
  `--reachability` on `rebuild-set` applies to that map as well as to `backport.md`.
- `stack sbom --backport-list` writes that list for a stack lock.
  `--cve` resolves an advisory through Open Source Vulnerabilities (OSV).
  `--osv-cache` reads recorded responses and, without `--cve`, every id in `queries.json`.
  `--reachability` is the same `name class` file as `rebuild-set`.
  Blast radius is the module plus every locked module that depends on it through a runtime or build edge.
  `local-only`, `not-reachable`, and `unclassified` are recorded and left off the list.
  A `.md` path also writes the sibling `.json`.
  `eb_stack_sbom` takes the same optional arguments and reports `listed`, `recorded`, and `stack` when the list is requested.
- `stack shard` splits one Jenkins build list into shards that can
  build at the same time. A line's set is its closure over
  `--easyconfigs` minus the modules in `--installed-modules-dir` Lmod
  trees and `--installed` lists. Lines whose own easyconfig another
  line needs share a shard; any other module two or more shards need
  goes to `prelude.list`. Groups are packed into at most `--shards`
  shards by `--weights` seconds, or one per module. `shard-NN.list`
  keeps the input lines verbatim, dependencies first, and `plan.json`
  records the closures, the shard weights and a SHA-256 of the inputs.
  A line that cannot be resolved exits 1 and writes nothing.
  `eb_stack_shard` is the same command over MCP.
- `stack oom --hooks FILE` reads `PARALLELISM_LIMITS` from an EESSI
  `eb_hooks.py` in place of the hook rows of
  `data/eessi-bot-resources.toml`. The file is parsed as Python and
  never run. A key may be a module-level string constant such as
  `CPU_TARGET_A64FX`, and a repeated key keeps its last value. A
  `divide_by_factor` that is not an integer is a warning that names the
  file and line (`16 // 2.5` is `6.0`, a float). An op other than
  `divide_by_factor` or `set_maximum`, a `**` unpacking, or a key that
  is not a string exits 1 with the line. `eb_stack_oom` takes `hooks`.
- `stack oom --hooks FILE` warns on a statement that adds to a name and
  drops the sum, such as `msg+ "..."` in the EESSI `post_ready_hook`:
  the second half of its log message never prints. The warning names
  the file, the line, and the function; `msg += "..."` is not reported,
  and the parallel levels do not change. `eb_stack_oom` returns these
  as `hook_warnings`.
- `stack oom --node` prints the memory cap beside the level the hook
  leaves, for a package with an `mib_per_job`:
  `min(current, memory_mib // mib_per_job)`, the same quotient as
  `memory_bytes // (mib_per_job * 2**20)`. It is not raised to 1, so
  `memory cap 0 (...), not one N MiB job fits` says one compile needs
  more than the node has. `--mib-per-job FILE` is a visible alias of
  `--job-memory FILE`. The `memory` column, `limit_by_memory` in place of
  the hook, is unchanged.
- `stack oom --parallel N` and `--max-parallel N` model a job that sets
  EasyBuild's `--parallel` or `--max-parallel`; the bot sets neither.
  EasyBuild uses `--parallel` in place of the core count, capped at
  max-parallel and `maxparallel`. The EESSI `post_ready_hook` still
  applies its op to the node's cores (`det_parallelism()` with no
  arguments) and keeps the result only when it is lower: with
  `--parallel 12` on 16 cores libxc gets 16 // 2 = 8, and with
  `--parallel 4` it stays at 4. The `--node` lines and a warning's
  `parallel` line name the setting that decided, and say when the
  hook's result was not lower. 0 for either flag is status 2.
  `eb_stack_oom` takes `parallel` and `max_parallel`.

### Changed

- `data/build-memory.toml` records software-layer #1772 bot job 221273:
  libxc 7.0.0 with GCC 15.2.0 ran out of the 32 GiB zen4 job at
  `make -j 5`, under a libxc divider of 3 in the software-layer-scripts
  commit the job used. The zen4 figure is now a lower bound of 6.4 GiB
  per process, 32 over 5, in place of 4.0, 32 over 8. A libxc warning
  on that node reads `8 x 6.4 GiB = 51.2 GiB` and suggests
  `maxparallel = 4` in place of 7, a divider of 2.5 (6 processes,
  38.4 GiB) no longer fits, and the remedy names a reduction of 8, as
  asked in the review of software-layer-scripts #337
  (issuecomment-6599147327).

### Documentation

- The command map lists `stack gap` once: it routes each site module the
  stack lacks, names the recipe that closes it, and writes one TSV per
  route. `campaign finding claim` and `campaign finding resolve` each
  have a row. `stack oom` has a usage block for `--arch`,
  `--easystack`, `--easyconfigs`, `--installed`, `--profile`,
  `--resources`, `--node`, `--job-memory` (alias `--mib-per-job`),
  `--hooks`, `--parallel`, and `--max-parallel`.
- The manual index links `llms.txt`, `llms-full.txt`, `AGENTS.md`, the
  skills directory, and `server.json`, the same discovery files the
  README names. Client setup stays on the Model Context Protocol page.
- How to split one Jenkins build list into shards that build at the
  same time, with the shared dependencies in a prelude.
- How to plan software-layer pull requests whose builds do not overlap,
  including the second-version warning a batch records.
- How to regroup a Jenkins build list under the section headers of a
  previous list.
- How to write a stack lock as the YAML `eb --easystack` reads.
- How to read the easyblock class, the template values, and the
  toolchain hierarchy EasyBuild derives.
- How to report the newest stable upstream release of an EasyBuild
  recipe, and why a listed version was kept or dropped.
- How to read the reproduction scoreboard and hold it to a ratchet file.
- How to report a site-layer recipe that shadows a module the EESSI
  base stack already provides.
- The out-of-memory how-to checks two forms of libxc's row with
  `--hooks`: a cap of 8, `(set_maximum, 8)`, the form proposed in the
  review of software-layer-scripts #337, and a reduction of 8,
  `(divide_by_factor, 8)`, as asked in the review of #337
  (issuecomment-6599147327). On the bot's 16-core nodes the cap gives 8,
  the same as dividing by 2, and pull request 1772 still warns on the
  32 GiB zen4 node; the reduction gives 2 and fits. A cap does not scale
  with the node: it gives 8 on 64 cores and on 12. The fixtures are
  `hooks/set-maximum-8.py` and `hooks/divide-8.py`.


`stack solve` in 0.3.0 took the policy file as `--policy`. The flag is
now `--stack-policy`, the same spelling `package plan` and `package bump`
already used. `--policy` remains a hidden alias: `--help` does not list
it, the file is read the same way, and the command prints
`warning: --policy is accepted for compatibility. Use --stack-policy.`
The Model Context Protocol argument `policy` warns and is still read;
`stack_policy` is the spelling to pass. Commands added since 0.3.0
include `stack gap`, `stack oom`, `stack from-commit`, `rebuild-set`,
`pr-overlap`, and `component-reach`.

### Added

- `stack provided` fails when a build-list line names software an
  installed-module list or a baseline lock already provides. The match
  is the package name, so a different version or toolchain still fails.
  `--allow` keeps a second version. The same command fails a toolchain
  other than 2026.1 or GCCcore-15.2.0 below it, a per-line `--hooks`,
  and a name that only `--family-lock` covers. `--licensed-file` warns
  for group-restricted names. `--format github` and `--format json` are
  the CI reports. `eb_stack_provided` is the same check.
- `stack gap` routes a missing name through an open software-layer pull
  request (`--open-layer`) and an open easyconfigs pull request at the
  generation (`--open-easyconfigs`), and writes one TSV per route.
  A toolchain whose version is the generation (`lfoss-2026.1`) counts as
  a recipe on that generation, as does an extra toolchain from
  `data/eessi-generations.toml`.
- `stack gap --site`, `--older`, and `--installed` read an Lmod module
  tree or `module -t avail` / `module spider -t` text. `--older` is an
  EESSI version loaded beside the site. `--installed` repeats for a CPU
  tree and an accelerator compute-capability tree. `--eessi-version`
  keeps `--open-layer` easystacks of that version. The open-pull-request
  match stays the filename the pull request adds.
- `stack oom` warns when a `PARALLELISM_LIMITS` `divide_by_factor` is
  not an integer. `eb_hooks.py` computes `max(1, parallel // factor)`,
  and a non-integer factor makes `//` return a float (`16 // 2.5` is
  `6.0`; `16 // 2.0` is `8.0`). The estimate uses the numeric floor.
  The same command models `set_maximum` as `min(parallel, n)`, a
  CPU-target key overriding `*`, and the A64FX divide by 4.
- `limit_by_memory(parallel, mib_per_job)` is
  `max(1, min(parallel, memory_mib // mib_per_job))`. `memory_mib` is
  the smallest cgroup v2 `memory.max` on the process's cgroup path, in
  MiB, or `MemTotal` when no cgroup v2 limit is set.
  `data/job-memory.toml` holds `mib_per_job` per package. A row with no
  number is unmeasured. libxc 7.0 is unmeasured; its peak has not been
  measured. `--node CORES:MEMORY_MIB[:TARGET]` prints the hook
  parallelism beside that rule. `--job-memory` replaces the table.
- `scripts/compile_rss.py` is a compiler launcher for
  `CMAKE_C_COMPILER_LAUNCHER` and for a `CC` wrapper. It records
  `getrusage(RUSAGE_CHILDREN).ru_maxrss` per compile.
  `--summarize` picks the worst compile and suggests `mib_per_job`
  with one quarter of headroom. The suggestion is arithmetic on the
  log, not a measurement of a named package.

### Changed

- `stack gap` classifies `--filter-deps` names as `filtered`. They were
  `excluded`, in the same bucket as a `--exclude-file` line.
- The markdown stack diff labels a module the baseline has and this solve
  did not select as `baseline not in this layer`. The machine-readable
  kind stays `removed`.
- Rust 1.88 is the minimum. `time` 0.3.55, pulled in by cyclonedx-bom,
  sets `rust-version` 1.88.0. The CI minimum job builds every target
  with that compiler.
- `scripts/fetch-upstream-trees.sh` exports `EB_EASYCONFIGS` as the robot
  directory `easybuild/easyconfigs`. The clone root is
  `EB_EASYCONFIGS_DEVELOP`.

### Fixed

- A root with no easyconfig on the requested generation is a missing
  recipe, including after the GCCcore, gompi, and foss hierarchy is
  searched. The error lists the toolchains and versions that have an
  easyconfig and names the nearest file to pass as `--toolchain`.
- A CVE whose only OSV range is GIT is mapped onto release versions.
  `component-reach` uses the advisory `versions` list when it names the
  releases, and otherwise resolves upstream tags with a cached blobless
  fetch. An offline lookup keeps the range unparsed.
- A package layer's `source_checksums` (one SHA-256 per plan source) is
  applied to the plan and used as the bump checksum when `--source-checksum`
  is absent. The command-line digest still wins. SeisSol's layer records the
  recursive git archive digest and marks Eigen build-only; a bump moves an
  existing runtime Eigen tuple into `builddependencies`.
  `-DCMAKE_BUILD_TYPE=Release` stays in the SeisSol config options.
- Verification commands and the doctor `eb --version` probe set
  `EASYBUILD_TMPDIR` and the target's EasyBuild environment, the same workload
  prefix a build uses. Compiler-wrapper scrubbing is unchanged.
- A CUDA + usempi package listed in overlay policy (`gromacs`) gets
  `mpi_numprocs` pinned on bump (default 2) instead of inheriting `$parallel`
  as NUMPROC, and `pretestopts` that count visible GPUs and disable direct GPU
  communication only on the library-MPI iteration. `recipe lint` warns
  `EB_MAINT_MPI_TEST_RANKS` when the rank pin is missing.
- `examples/eessi-pr-overlap/fetch-prs.sh` rewrites `raw/` and `prs/` from
  scratch on every run, so a pull request that closed or stopped touching
  easystacks does not persist from a stale run; it fetches every pull request
  head in one `git fetch --stdin` (with automatic gc off and two retries)
  instead of a shallow fetch per pull request that could fail with git exit
  128; it reaches the count line and exits 0 when no head easystack names a
  `from-commit` or `include-easyblocks-from-commit` pin; and it clears its
  three shellcheck findings (SC2178/SC2128 from reusing the `files` array as a
  string, SC2012 from `ls | wc -l`). It matches the easyconfigs script.
- `examples/eessi-pr-overlap/fetch-prs.sh` reads every open pull request.
  It listed them with a `gh pr list` limit of 500 and dropped the rest
  without a message. It now pages through the pulls API 100 at a time, as
  the easyconfigs script does, and prints `open pull requests: N`.
- `stack arch --probe-sources` and `eb_stack_arch` with `probe_sources`
  probe only the architecture families `--arch` (`arch`) names. They
  probed x86_64, aarch64 and riscv64 whatever `--arch` said, so
  `--arch x86_64` failed on aarch64 and riscv64 wheels, and a recipe
  that does not parse on an unrequested family aborted the command with
  `source probe:`. With no `--arch` all three are still probed. With one
  family there is nothing to compare with, so a source it cannot fetch
  is `unreachable` rather than `arch-missing`, and still exits 1.
  Two families compare only with each other: a miss is `arch-missing`
  when the other named family fetched that source slot, and
  `unreachable` when neither did.
- `classify_build_failure` classes `No space left on device` and
  `Disk quota exceeded` as `resource`. A full disk in an EasyBuild log was
  `compile` before.
- A version bump moves the file name in a positional checksum's trailing
  comment to the new version.
- A version bump moves a `local_*` commit that is the old release tag's
  commit to the new tag's commit (RStudio-Server's `local_git_rev` named the
  previous release).
- A bump writes a package-config layer's build requirement into
  `builddependencies`, and splits a layer's long `config_options` into
  `configopts +=` lines. `build.extra_config_options` adds options after the
  recipe's own `configopts` expression.
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

- The manual defines closure, generation, baseline, reachability, and
  the claim ladder in the glossary, and the pages that use those words
  point at it.
- `stack gap --help` names fixture files in this repository. A missing
  `--site` file exits 1 and names the path.
- `package plan --help` names both `--package-index` shapes. A file that
  contains a `Package:` field is the CRAN `PACKAGES` index. A file with
  no `Package:` field is a `name==version` list.
- The install notes require Rust 1.88, write the binary with
  `install -Dm755` to `~/.local/bin/eb-stack`, and say to put
  `~/.local/bin` on `PATH`. The published 0.3.0 binary lacks `stack gap`,
  `stack oom`, `stack from-commit`, `rebuild-set`, `pr-overlap`, and
  `component-reach`. A build of this tree prints `eb-stack 0.4.0`.
- The site-layer walk in `docs/orgmode/howto/emit-reports.org` runs
  `stack solve` for EESSI 2026.06 against the trees
  `scripts/fetch-upstream-trees.sh` checks out. There is no
  `GROMACS-*-foss-2026.1.eb` in that pin, so the command exits 1 and
  names the easyconfigs that do exist. `baseline not in this layer` in
  the diff is the rest of the baseline a one-root solve did not select.
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

- `rebuild-set --findings` writes `backport.md` beside the ranked cost
  tables. A module is on that list when its scanner package or module name
  is `cross-user` or `network-facing`. The built-in names are Jupyter
  server, JupyterLab, notebook, cURL, urllib3, aiohttp, tornado,
  cryptography, and git. `--reachability` (`reachability` on
  `eb_rebuild_set`) is a `name class` file that replaces one of those
  names or adds one; `local-only` and `not-reachable` are recorded and
  left off the list. Any other name stays `unclassified`. A Python
  interpreter whose fix stays on the same minor is annotated: keeping the
  module path costs the in-place column, and renaming the easyconfig to
  that patch version costs the bump column. The count line includes
  `backport=N`. Findings schema version is 2. The same `backport.md`
  adds a patch scaffold for each chosen module (`<Name>-<version>_<ID>.patch`,
  the `patches` and `checksums` lines, and a fix commit URL when the export
  names one). `--findings` also accepts Trivy, Grype, and OSV-scanner JSON.
  A module `--filter-deps` drops is listed as the OS errata stream and is
  not given a scaffold. A cURL finding records the `--filter-deps=cURL`
  trade-off.
- `rebuild-set --findings` also writes `fix-cost.json`, `fix-cost.tsv`, and
  `fix-cost.md`. Each advisory is one row. A version bump is the
  reverse-dependency closure from the rebuild set, and it breaks
  reproducibility because the module path changes. An in-place patch keeps
  the name and version, adds `<Name>-<version>_<ID>.patch` to `patches`, and
  rebuilds the patched module, a bundle that includes it as an extension or
  a component, and a dependent known to link it statically. A
  builddependency-only dependent stays out of that count when the recipe
  and the install cannot say how it was linked. The recommended route uses
  the same reach classes as `backport.md`: an in-place backport for
  `cross-user` or `network-facing`, a bump when a Python fix leaves the
  minor version, and defer otherwise. The roll-up is the fraction of the
  installed stack each strategy rebuilds. The count line adds
  `fix_routes=N` and those three fractions beside `backport=N`.
- `stack solve --dot-out` and the MCP field `dot_out` write the solved
  dependency DAG. A node is `name/version` plus the toolchain, and the
  versionsuffix when the recipe has one. An edge points from a package to a
  dependency it selected and is labelled with the requirement; a
  builddependency is dashed. A `.svg` or `.png` path is rendered with
  Graphviz `dot`. Any other suffix is the DOT source.
- `stack generation` and `eb_stack_generation` check each added entry's
  toolchain generation against the generation the easystack file name names,
  the token after `eb-<X.Y.Z>-` such as `-2025b.yml`. An entry whose
  generation its EESSI version carries but that differs from the file name's
  generation is a new `filename` finding, so a `foss-2025a` entry added to a
  `-2025b.yml` file is caught even though 2025.06 carries 2025a. The count
  line now reads `outside=N mixed=N versions=N filename=N`, and the MCP tool
  returns a `filename` array. Non-generation groups such as `001-system`,
  `010-CUDA` and `cc70-CUDA` are not checked this way.
- `stack known-failures` and the MCP tool `eb_stack_known_failures` take a
  `--strict` flag (`strict: true`). It prints the same report, then exits 1
  (returns a tool error that lists the warnings) when a warning remains, so a
  pre-bot script or CI job can fail on a known failure with no remedy in
  place. Remedied hits never count, so a plan whose every hit is remedied
  exits 0. Without it the behavior and output are unchanged.
- `stack batch` and the MCP tool `eb_stack_batch` run the `stack generation`
  second-version check over each planned pull request: a name with two
  versions in one toolchain generation, next to one an earlier batch of the
  same run adds or the installed list already carries, is a warning in
  `pr-body.md`, `batch.json` (`versions.warnings`, `versions.count`) and
  `plan.md`. `--allow-versions NAME[@GENERATION]` (repeatable, `allow_versions`
  for the MCP tool) accepts one, as it does for `stack generation`. The
  warnings do not fail the batch; a bad spec is a usage error. The check is
  skipped when the EESSI version is not in the generation table.
- `stack bot-classify` and the MCP tool `eb_stack_bot_classify` read EESSI
  bot comments, `_bot_job<id>.result` files and job output, and classify
  each bot job as a real failure (with the campaign build-finding class),
  ERROR-only noise, an empty tarball, or infra. Text no rule matches stays
  `unclassified`. Fixtures are trimmed comments from software-layer pull
  requests 1873, 1869, 1834, 1760, 1732, 1733, 1818, 1823 and 1741.
- `eb-stack stack files` finds the patch and source files an easystack's
  easyconfigs need that neither the easyconfig's directory nor the robot
  path has. Patches of the recipe, its extensions, and its components,
  patch files named only in `checksums`, and sources no URL downloads are
  looked up the way EasyBuild does, with each `from-commit` pin's tree on
  the robot path. A missing patch fails the command (software-layer #1842
  and #1048), and a component patch beside a pinned bundle is reported
  under easybuild-framework#5133 (#1398). A source with no URL is a
  `no_url` warning. `eb_stack_files` runs the same check.
- `eb-stack stack generation` refuses an easystack entry whose toolchain
  is outside the generations of its EESSI version, naming the generation
  and the version that carries it. `data/eessi-generations.toml` is the
  table: 2023.06 carries 2022b, 2023a and 2023b, 2025.06 carries 2024a,
  2025a and 2025b, 2026.06 carries 2026.1. It also refuses easystacks for
  two EESSI versions in one pull request, and a plan that adds a second
  version of a software in one generation unless `--allow-versions`
  names it. `--pr` reads `pr-overlap --pr` JSON and checks only the
  added lines. The fixtures are software-layer #1472 (pv on
  GCCcore-12.3.0 in 2025.06), #1834 (fastp for 2025.06 and 2026.06) and
  #1497 (a second GROMACS on foss-2025b). The foss-2022b and foss-2023a
  hierarchies are now built in. `eb_stack_generation` runs the same
  check.
- `eb-stack stack cuda-cc` checks the `cuda_compute_capabilities` of
  each CUDA-dependent easyconfig in an easystack's closure against the
  CUDA release it depends on, the EESSI accelerator targets in
  `data/eessi-accelerator-targets.toml`, and the easyblock mapping limits
  in `data/easyblock-cuda-capability-map.toml` (software-layer #1461).
  Any finding exits 1. `eb_stack_cuda_cc` runs the same check.
- `eb-stack stack arch --easyconfigs` parses each easyconfig once per
  architecture, so a file the plan names that fails to parse on one
  architecture is a `parse_failed` line and exits 1 (software-layer
  #1798, Go on riscv64). `--probe-sources` resolves each planned
  easyconfig's sources for x86_64, aarch64, and riscv64 and probes the
  URLs with HEAD and a ranged GET fallback, reporting `unreachable` and
  `arch-missing` sources (software-layer #1670, Kaleido). The probe is
  opt-in because it makes network requests. `eb_stack_arch` returns
  `parse_failed` and `source_findings`.
- `eb-stack stack known-failures` warns when a plan includes, or depends
  on, an easyconfig a bot run has already failed on one architecture,
  and prints the established easyconfig remedy as text.
  `data/arch-known-failures.toml` is the table, each entry citing the
  software-layer pull request it came from: waLBerla 7.2 on the SVE
  targets (#1623), Kaleido 0.2.1 on aarch64 (#1670), numba 0.62.0 test
  failures on a64fx and timeouts on zen2 and sapphirerapids (#1683),
  orjson on a64fx (#1871, #1829), uv 0.10.9 on a64fx and uv and uv-build
  on the 2025.06 haswell nodes (#1668, #1871), and the GROMACS sanity
  check that EESSI's CUDA demotion hook breaks under EasyBuild 5.4
  (#1658). A pin that carries the fix, or an easyconfig that already
  has it, is reported as remedied. `stack batch` writes the warnings
  into each `pr-body.md` and `batch.json`. `eb_stack_known_failures`
  runs the same check.
- `stack oom` and the MCP tool `eb_stack_oom` warn when a plan entry is
  likely to run out of memory in an EESSI bot job. The peak build memory
  per software and version family is in `data/build-memory.toml`, and the
  bot build nodes and the `eb_hooks.py` parallelism limits are in
  `data/eessi-bot-resources.toml`. Every row cites a real pull request.
  A warning names the target, the node, the parallel level and the
  estimate, and suggests a `maxparallel`, a hook limit or a node with
  more memory. The command exits 0. The fixtures are the libxc build
  killed on zen4 in software-layer #1772, Highway in #1692, hipBLASLt and
  rocBLAS in #1549, and PETSc in #1608.
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
  `--stack-policy`. `stack solve --policy` from 0.3.0 is a hidden
  alias of `--stack-policy`: it is omitted from `--help`, it still
  names the policy file, and it warns
  `warning: --policy is accepted for compatibility. Use --stack-policy.`
  `--toolchain-name` and `--toolchain-version` are hidden aliases of
  `--toolchain name/version` and warn the same way. The MCP argument
  `policy` warns and is still read; `stack_policy` is the spelling to
  pass. The MCP package tools require a toolchain through the schema's
  `anyOf`, and a call without one names `toolchain`.
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
