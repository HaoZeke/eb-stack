# Agent driver contract for eb-stack

Use the matching public skill and execute it through the requested claim rung:

| Work | Skill |
|---|---|
| conda-forge or Spack: new EasyBuild package | `skills/new-package/SKILL.md` |
| existing `.eb`: new toolchain/application version | `skills/annual-bump/SKILL.md` |
| prove an emitted or edited recipe before building it | `skills/verify-recipe/SKILL.md` |
| upstream easybuild-easyconfigs PR / test report | `skills/upstream-pr/SKILL.md` |
| EasyBuild do/don't (incl. #26435 class) | `skills/easybuild-dos-donts/SKILL.md` |
| reviewer comments on an easyconfig | `skills/easybuild-dos-donts/SKILL.md` (review-fix) |
| an easyblock (Python module) is wrong | `skills/easyblock-fix/SKILL.md` |
| build an upstream recipe at a site without forking it | `skills/site-consume/SKILL.md` |
| EESSI-extend test / European Environment for Scientific Software Installations (EESSI) software-layer PR | `skills/eessi-extend/SKILL.md` |
| generated output needed a hand correction | `skills/tool-repair/SKILL.md` |
| regenerate the SeisSol golden overlay | `skills/golden-replay/SKILL.md` |
| inspect a foreign recipe | `skills/package-inspect/SKILL.md` |
| plan a package against a fixture robot | `skills/package-plan/SKILL.md` |
| bump an existing easyconfig | `skills/package-bump/SKILL.md` |
| write a CycloneDX SBOM from a stack lock | `skills/stack-sbom/SKILL.md` |
| report modules shared by open pull requests | `skills/pr-overlap/SKILL.md` |
| serve those commands over stdio | `skills/mcp/SKILL.md` |

Two of those are gates rather than tasks and cut across the rest:
`verify-recipe` before any build or push, `tool-repair` whenever an emitted
file had to be corrected by hand.

## Canonical procedure

1. Run `eb-stack package inspect|plan|bump`; do not use removed flat commands.
2. Treat `package.plan.json` as the canonical build manifest, `package.sbom.cdx.json` as the planned Software Bill of Materials (SBOM), and `locks/*.lock.json` as Resolvo evidence.
3. Emit one `.eb` file per product profile. Keep the default profile unsuffixed and follow neighboring GROMACS/LAMMPS conventions.
4. Run `eb-stack recipe format|lint|check` on emitted recipes. `recipe lint`
   reports E999 (not valid Python), the easyconfigs CI lint (F541, W605, W291), the
   check-contrib pycodestyle codes (E501, E225-E228, E231), the easyconfigs
   test-suite checks (`EB_PY_NO_USER_SITE`, `EB_SANITY_PATHS_SHAPE`,
   `EB_LOCAL_VAR_NAMING`, `EB_REDEFINED_PARAM`, `EB_CHECKSUM_COUNT`), and review
   conventions (`EB_FMT_*`, `EB_SHLIB_HARDCODED`, ...), each with a severity and
   a top-level `clean` flag, without relying on `eb --check-contrib`;
   it merges `eb --check-contrib` when `eb` is installed. `recipe check` needs
   at least one easyconfigs path and refuses a recipe that is not valid Python.
5. Run `eb-stack target doctor` before a build campaign.
6. Run or resume `eb-stack campaign run` on the configured EasyBuild target until the requested claim rung is established.
7. Use `campaign finding claim|resolve` for OMP coordination. Never edit campaign JSON directly or steal an owned finding.
8. For an **upstream easyconfigs PR**, after recipes are ready, use the EasyBuild
   CLI on the build host: `eb --check-contrib`, then
   `eb --from-pr <N> --robot --upload-test-report` (see `skills/upstream-pr/SKILL.md`).
   Do not invent substitutes for contribution checks or test-report upload.

## Review fixes

When an EasyBuild reviewer asks for a recipe change, read the whole file,
apply that change at every occurrence, run `eb-stack recipe lint` and
`eb-stack recipe check`, and show the diff. Do not stop after the first hunk.
EasyBuild template values (`%(name)s`, `%(version)s`, `%(builddir)s`) stay
templates. An f-string is only for a Python constant such as `SHLIB_EXT`, and
it always has a placeholder. The table, the citations, and the edit steps are
in `skills/easybuild-dos-donts/SKILL.md`. Do not declare the fix done while
lint findings remain, and do not comment on the PR.

## Non-negotiables

- Dependency versions, hierarchy choices, site pins, and fallbacks belong inside Resolvo. A package-specific `--dep` override becomes a locked solver pin.
- Boolean satisfiability (SAT) compatibility is not build evidence. Hermes owns classified target/recipe/policy repair and repeats the campaign through verification.
- Fix code and recipes under test. Never remove assertions, dependencies, checksums, sanity checks, or tests to clear a failure.
- Run EasyBuild installs on the configured target, scheduler, and runtime. Keep `EASYBUILD_TMPDIR` on durable storage.
- Report `resolves`, `builds`, and `binary-verified` separately. A plan establishes only `resolves`; a build without declared verification does not establish `binary-verified`. `eessi-verified` is a further rung owned by `skills/eessi-extend/SKILL.md`; a build on a normal target never establishes it.
- The public issue and PR surface belongs to the human operator. Prepare paste-ready material and evidence without opening or mutating remote issues or PRs.

## Version-one CLI

```text
package  inspect | plan | bump
recipe   easyblock | templates | hierarchy | check | lint | format
stack    solve | sbom | order | easystack | sections | from-commit | arch | files | generation | cuda-cc | known-failures
target   list | doctor
campaign run | status | finding claim | finding resolve
repro    summary
easyblock-reach
pr-overlap
rebuild-set
component-reach
mcp
```

The Model Context Protocol (MCP) catalog mirrors these commands,
including pr-overlap, rebuild-set, component-reach, easyblock-reach,
and repro summary, with `eb_`-prefixed tools. Recipe file tools
(`eb_recipe_read`, `eb_recipe_list`, `eb_recipe_edit`,
`eb_recipe_apply_suggestion`, `eb_recipe_diff`, `eb_recipe_write`,
`eb_recipe_grep`, and `eb_dep_versions`), easyblock dry-run, module
lint, build log, and build test are MCP-only. `eb_build_test` runs
only when `EB_STACK_ALLOW_BUILD=1`.

## Build, test, lint, and docs

Rust 1.88 is the minimum. CI uses current stable. Run builds and tests on
the repository's configured build machine when local compilation is prohibited.

```sh
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo package --locked
```

`cargo test --locked --lib` is the library slice. The jobs in
`.github/workflows/ci_test.yml` split the same suite into `--test <name>`
runs: known bumps, packaging fixtures, foreign adapters, package bundles,
catalog/closure/source discovery, profile emission, target routing, campaign
state, CLI, and MCP. `real_tree_scale` needs a local EasyBuild robot tree
(`EB_EASYCONFIGS`) and is not required on GitHub runners.
`scripts/fetch-upstream-trees.sh` creates a checkout of the pinned
easybuilders trees and writes `env.sh` for that variable and for the ignored
real-tree tests. The `Upstream trees` workflow runs
`cargo test --include-ignored` against that cache every week, on a push
that changes the pinned tree list in that script, and by hand.

`uvx prek run -a` runs the hooks in `prek.toml` (trailing whitespace, YAML
and TOML, codespell, large files). That is the `Pre-checks` workflow.

Docs, with pixi and Emacs:

```sh
pixi run -e docs docbld
scripts/check-doc-links.sh docs/build
```

`pixi run -e docs docbld` exports Org to RST, generates the Rust API pages,
and runs `scripts/sphinx-build-docs.sh`, which calls
`sphinx-build -W --keep-going`. Without pixi, point `EB_STACK_DOCS_PYTHON`
at a virtualenv that imports `sphinxcontrib_rust`, run
`emacs -Q --batch --load export.el` from `docs/`, run
`scripts/gen-rust-api.sh`, then run `scripts/sphinx-build-docs.sh`.

`python3 scripts/gen-llms-txt.py --check` fails when `llms.txt` drifts from
`docs/orgmode`. The same check is `cargo test --locked --test llms_txt`.

`tests/property` and `tests/generation` are pytest modules. They read
`EB_STACK_BIN` and `EB_EASYCONFIGS` and skip when either is unset.

No affected suite may remain red in a completion claim.
