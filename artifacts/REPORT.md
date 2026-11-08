# U3–U5 delivery report

Base: `da07561a0c430957c2d014f31855bbe973064197`

Feature tip: `682ea65` (`cursor/stack-gap-lint-5553`)

## Baseline coverage

The requested baseline already contained the core U3 and U4 commands:

- `stack provided` already rejected names from an installed listing or
  baseline lock and rejected per-line `--hooks`/`--hook`.
- `stack gap` already read live module trees and dumps, accepted both
  open-pull-request inputs, and applied the ns-006 route priority.

Following the brief's skip rule, this series does not duplicate those
implementations. It closes the remaining differences: default-text line
numbers for every U3 error, the U4 dump helper and exact one-row-per-route
fixture, and all of U5.

## Commits

1. `25f6010 fix(stack): report build-list line numbers`
   - Reuses `shard::parse_lines` for source line identity and
     `jenkins::parse_build_list` for filenames/flags.
   - Adds one-based line numbers to `provided` and `unparsed` text findings,
     matching the existing hook/toolchain diagnostics.
2. `57757bd feat(stack): capture live module inventories`
   - Adds `scripts/dump-module-trees.sh`, which captures the site, older
     EESSI, and target EESSI trees in separately purged shells.
   - Adds an executable fixture test and a seven-name fixture with exactly
     one row in each route TSV.
3. `1342ae9 feat(stack): write module version changes`
   - Preserves versions from `module -t avail`, spider-style text, and Lmod
     trees.
   - Writes `version-change.tsv`: `in-stack` becomes `moved`, `excluded`
     becomes `site-only`, and every other missing route is `gone` until it
     lands.
4. `02aec42 docs(stack): refresh module gap index`
   - Regenerates the checked-in long-form documentation index.
5. `3ada26f docs(stack): use configured upstream trees`
   - Makes the pull-request inventory walkthrough use the configured pinned
     checkout variables.
6. `55b5a73 ci(stack): run module dump helper test`
   - Adds the helper test to the explicit integration-test CI slice.
7. `682ea65 docs(stack): fix shard API literal`
   - Fixes a pre-existing malformed generated RST literal so the required
     Sphinx warnings-as-errors build is clean.

## Minimal working examples

### U3: build-list lint

Command:

```sh
eb-stack stack provided \
  --build-list tests/provided_fixtures/site.list \
  --installed tests/provided_fixtures/installed.txt \
  --baseline-lock tests/provided_fixtures/baseline.lock.json
```

Before (`da07561a`, exit 1), baseline and unparsed findings had no source
line:

```text
provided GROMACS-2026.0-foss-2026.1.eb name=GROMACS list=tests/provided_fixtures/site.list
unparsed not-a-version.eb list=tests/provided_fixtures/site.list
hooks FFTW-3.3.10-gompi-2026.1.eb name=FFTW list=tests/provided_fixtures/site.list line=4 sets=--hooks=/sw/hooks/site.py; which replaces EESSI's hooks
```

After (exit 1), every failing line is located:

```text
provided GROMACS-2026.0-foss-2026.1.eb name=GROMACS list=tests/provided_fixtures/site.list line=3
unparsed not-a-version.eb list=tests/provided_fixtures/site.list line=7
hooks FFTW-3.3.10-gompi-2026.1.eb name=FFTW list=tests/provided_fixtures/site.list line=4 sets=--hooks=/sw/hooks/site.py; which replaces EESSI's hooks
```

### U4: live dumps and one row per route

Portable helper test:

```sh
cargo test --locked --test module_tree_dump_script
```

Before:

```text
error: no test target named `module_tree_dump_script`
exit=101
```

After:

```text
running 1 test
test helper_dumps_site_older_and_target_module_trees ... ok
test result: ok. 1 passed; 0 failed
```

The site command documented for a real initialized Lmod shell is:

```sh
scripts/dump-module-trees.sh \
  /tmp/module-dumps - EESSI/2025.06 EESSI/2026.06
```

The exact route fixture:

```sh
eb-stack stack gap \
  --site tests/gap_routes/one-per-route.txt \
  --installed tests/gap_routes/installed.txt \
  --easyconfigs tests/gap_routes/easyconfigs \
  --generation 2026.1 \
  --exclude-file tests/gap_routes/site-only.tsv \
  --filter-deps zlib \
  --open-layer tests/gap_routes/open-layer.json \
  --open-easyconfigs tests/gap_routes/open-easyconfigs.json \
  --out-dir /tmp/gap-one-per-route
```

Output:

```text
site=7 in-stack=1 filtered=1 open-layer=1 recipe=1 open-easyconfigs=1 no-recipe=1 excluded=1 out=/tmp/gap-one-per-route
```

### U5: version-change table

Run the live-tree fixture from `docs/orgmode/howto/module-gap.org`, using
`/tmp/gap-mwe` as `--out-dir`, then inspect
`/tmp/gap-mwe/version-change.tsv`.

Before:

```text
site=7 in-stack=2 filtered=1 open-layer=1 recipe=0 open-easyconfigs=0 no-recipe=1 excluded=2 out=/tmp/gap-mwe
version-change.tsv missing
```

After:

```text
name	old_version	eessi_version	status
CUDA	12.6.0	12.6.0	moved
FFTW	3.3.10	3.3.10	moved
LAMMPS	2023.0	-	gone
LAMMPS	2024.0	-	gone
MATLAB	2024b	-	site-only
QuantumESPRESSO	7.3	-	gone
binutils	2.42	-	gone
code-server	4.96.2	-	site-only
```

## Verification

All final gates passed with Rust stable 1.99.1 (the checkout's default Rust
1.83 is below the declared Rust 1.88 minimum):

```sh
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo package --locked
uvx prek run -a
pixi run -e docs docbld
scripts/check-doc-links.sh docs/build
python3 scripts/gen-llms-txt.py --check
```

The Sphinx build completed with `build succeeded.` under `-W --keep-going`.
All eight prek hooks passed.

## Reproduction

```sh
git clone /path/to/eb-stack eb-stack-u3-u5
cd eb-stack-u3-u5
git fetch artifacts/eb-stack-u3-u5.bundle \
  cursor/stack-gap-lint-5553:u3-u5
git switch u3-u5
```

Alternatively, apply `artifacts/patches/*.patch` in numeric order to
`da07561a`.

## Left undone

No requested code or fixture work remains. The cloud runner has no site Lmod
service, so the helper was verified with a deterministic `module` fixture
rather than a production site tree. No upstream EasyBuild or EESSI repository
was mutated.
