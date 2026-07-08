# eb-stack

Private tooling: parse EasyBuild `*.eb` files, co-select a software stack with
**resolvo** (CDCL SAT, `solver.engine = resolvo_cdcl_sat`), emit a stack lock and
a planned CycloneDX SBOM.

## Example: GROMACS foss-2025a → foss-2025b

```bash
# build on a remote builder; do not thrash a laptop with cargo
cargo test
cargo build --release

./target/release/eb-stack solve \
  --easyconfigs fixtures/gromacs_2025_to_next/easyconfigs \
  --policy fixtures/gromacs_2025_to_next/policies/prefer_newer.json \
  --baseline-easyconfigs fixtures/gromacs_2025_to_next/easyconfigs \
  --lock-out stack.lock.json \
  --sbom-out stack.cdx.json \
  --build-list-out build.list \
  --stack-diff-out stack.diff.md
```

When the baseline tree contains **multiple generations** of the same toolchain family as
the policy target, `solve` picks the baseline generation as follows:

1. If `--baseline-toolchain-version VERSION` is set, that generation is used (must exist
   in the baseline tree for the policy toolchain name).
2. Otherwise, the **nearest lower** generation than the policy target is used (EasyBuild-style
   version order, e.g. target `2025b` with `2024b` and `2025a` present → baseline `2025a`).

Optional outputs (omit to keep prior lock+SBOM-only behavior):

- `--build-list-out`: plain-text selected easyconfigs in dependency order (one path
  per line) for sequential install pipelines.
- `--stack-diff-out`: markdown package-level diff vs the baseline (unchanged /
  added / removed / version-bumped with easyconfig paths), pasteable into a PR.

The same flags work on `solve-json` (baseline via `--baseline` lock JSON).

`prefer_newer` co-selects GROMACS 2025.0 with OpenBLAS 0.3.27, OpenMPI 5.0.3,
FFTW 3.3.10. Design notes (if any) live in a separate notes vault, not this repo.

## Easyconfig parsing

`.eb` files are a restricted Python DSL. The crate evaluates that subset (including
`local_*` names, the `SYSTEM` toolchain constant, multi-element dependency tuples,
`builddependencies`, and `exts_list`) and resolves EasyBuild-style `%(…)s` templates
derived from name/version/toolchain. Solver code still consumes `Candidate` /
`DepReq`; full resolution is available as `ResolvedEasyconfig`.

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
