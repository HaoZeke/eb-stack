# eb-stack

Parse EasyBuild `*.eb` files, co-select a software stack with **resolvo**
(CDCL SAT), rewrite a recipe onto the next toolchain generation (`bump`),
and emit a stack lock, build list, and optional planned CycloneDX SBOM.

It is assistive tooling for a toolchain-generation rebuild, not an EasyBuild
replacement and not a build runner. On a measured sample of real maintainer
`foss-2023b` → `foss-2024a` pairs it reproduces the next-generation recipe
exactly (or exactly modulo a hand-added dependency) for about half the
packages, and never silently emits a wrong dependency version on the rest.

## Install

```bash
# build on a remote builder when local compiles thrash the machine
cargo test --locked
cargo build --release
# binary: target/release/eb-stack
```

## Tutorial path: one zero-hand-fed bump

```bash
./target/release/eb-stack bump \
  --source tests/repro_fixtures/gromacs/GROMACS-2024.4-foss-2023b.eb \
  --toolchain-name foss \
  --toolchain-version 2024a \
  --easyconfigs tests/repro_fixtures/universe_foss_2024a \
  --out-dir /tmp/gromacs-2024a
```

Every dependency version comes from the universe; no `--dep` flags. The only
intentional gap versus the real maintainer `foss-2024a` recipe is a hand-added
`pybind11` line the tool correctly does not invent. Full walkthrough:
`docs/orgmode/tutorial.org`.

## Annual rebuild (operator / agent)

For a whole generation move — many recipes, residual decisions, claim ladder,
PR discipline — use:

| Audience | Document |
|----------|----------|
| Human operator | [`docs/orgmode/howto/run-annual-bump.org`](docs/orgmode/howto/run-annual-bump.org) |
| Agent driver | [`skills/annual-bump/SKILL.md`](skills/annual-bump/SKILL.md) |
| Repo contract | [`AGENTS.md`](AGENTS.md) |

Both the skill and the howto describe the same loop: `bump` → residual table →
`check-recipe` → `solve` → human-reviewed PR. MCP surface: `eb-stack mcp`
(`eb_check_recipe` / `eb_bump` / `eb_solve`).

## Solve a multi-package stack

```bash
./target/release/eb-stack solve \
  --easyconfigs fixtures/gromacs_2025_to_next/easyconfigs \
  --policy fixtures/gromacs_2025_to_next/policies/prefer_newer.json \
  --baseline-easyconfigs fixtures/gromacs_2025_to_next/easyconfigs \
  --lock-out stack.lock.json \
  --sbom-out stack.cdx.json \
  --build-list-out build.list \
  --stack-diff-out stack.diff.md
```

When the baseline tree contains **multiple generations** of the same toolchain
family as the policy target, `solve` picks the baseline generation as follows:

1. If `--baseline-toolchain-version VERSION` is set, that generation is used
   (must exist in the baseline tree for the policy toolchain name).
2. Otherwise, the **nearest lower** generation than the policy target is used
   (EasyBuild-style version order, e.g. target `2025b` with `2024b` and
   `2025a` present → baseline `2025a`).

Optional outputs:

- `--build-list-out`: selected easyconfigs in dependency order (one path per
  line) for sequential install pipelines.
- `--stack-diff-out`: markdown package-level diff vs the baseline (unchanged /
  added / removed / version-bumped), pasteable into a PR.

`prefer_newer` co-selects GROMACS 2025.0 with OpenBLAS 0.3.27, OpenMPI 5.0.3,
FFTW 3.3.10.

## Tests and CI

GitHub Actions (`.github/workflows/ci_test.yml`) runs on every push and PR:

| Job | What it covers |
|-----|----------------|
| `cargo test --lib` | Unit tests |
| known-bump regression | `--test reproduce_real_prs --test bump_emit`: frozen maintainer pairs (GROMACS, ScaFaCoS, MDTraj, Fiona, PuLP, numba) library + CLI |
| packaging fixtures | eOn 2.16.0 and QMCPACK 4.3.0 landable recipe sets (`fixtures/eon_foss_2026_1`, `fixtures/qmcpack_foss_2026_1`, `fixtures/eon_packaging`) |
| solve / reports | build-list and stack-diff emission |
| CLI smoke | release `eb-stack bump` on the GROMACS tutorial path |

```bash
cargo test --locked --lib
cargo test --locked --test reproduce_real_prs --test bump_emit
cargo test --locked --test eon_foss_2026_1 --test qmcpack_foss_2026_1 --test eon_packaging
```

Robot-overlay check-recipe cases skip when no easyconfigs tree is present
(`EB_EASYCONFIGS` or `~/.venvs/easybuild/easybuild/easyconfigs`). Resolve and
packaging_gate always run against the checked-in fixtures.

## Easyconfig parsing

`.eb` files are a restricted Python DSL. The crate evaluates that subset
(including `local_*` names, the `SYSTEM` toolchain constant, multi-element
dependency tuples, `builddependencies`, and `exts_list`) and resolves
EasyBuild-style `%(…)s` templates derived from name/version/toolchain.

Hard-case samples and EasyBuild-captured goldens live under
`fixtures/parser_hardcases/`. Refresh goldens (requires `~/.venvs/easybuild`):

```bash
source ~/.venvs/easybuild/bin/activate
python scripts/resolve_easyconfig_eb.py \
  fixtures/parser_hardcases/easyconfigs/*.eb \
  -o fixtures/parser_hardcases/resolved/
```

`cargo test` does not invoke EasyBuild; it asserts the crate parser against the
checked-in resolved JSON.

## Docs

Org sources under `docs/orgmode/`; build with:

```bash
pixi run -e docs docbld
# HTML -> docs/build/index.html
```

## License

MIT.
