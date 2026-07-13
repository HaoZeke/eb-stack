<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/branding/eb-stack-logo-dark.svg">
    <img src="assets/branding/eb-stack-logo.svg" alt="eb-stack" width="420">
  </picture>
</p>

**Rewrite EasyBuild stacks onto the next toolchain generation.**

Parse `*.eb` files, hierarchy-aware `bump` with zero hand-fed dependency
versions, co-select a consistent stack with **resolvo** (CDCL SAT), and emit a
lock, build list, optional planned CycloneDX SBOM, and a reviewable stack diff.

[![CI](https://github.com/HaoZeke/eb-stack/actions/workflows/ci_test.yml/badge.svg)](https://github.com/HaoZeke/eb-stack/actions/workflows/ci_test.yml)
[![Docs](https://github.com/HaoZeke/eb-stack/actions/workflows/ci_docs.yml/badge.svg)](https://github.com/HaoZeke/eb-stack/actions/workflows/ci_docs.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Docs site](https://img.shields.io/badge/docs-eb--stack.rgoswami.me-teal)](https://eb-stack.rgoswami.me)

On a measured sample of real maintainer `foss-2023b` → `foss-2024a` pairs it
reproduces the next-generation recipe exactly (or exactly modulo a hand-added
dependency) for about half the packages, and never silently emits a wrong
dependency version on the rest.

## Install

```bash
git clone https://github.com/HaoZeke/eb-stack.git
cd eb-stack
cargo test --locked
cargo build --release
# binary: target/release/eb-stack
```

## 30-second quickstart

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
`pybind11` line the tool correctly does not invent.

Full walkthrough: [tutorial](https://eb-stack.rgoswami.me/tutorial.html) ·
source: [`docs/orgmode/tutorial.org`](docs/orgmode/tutorial.org).

## Annual rebuild

For a whole generation move — residual decisions, claim ladder, PR discipline:

| Audience | Document |
|----------|----------|
| Human operator | [`docs/orgmode/howto/run-annual-bump.org`](docs/orgmode/howto/run-annual-bump.org) |
| Agent driver | [`skills/annual-bump/SKILL.md`](skills/annual-bump/SKILL.md) |
| Repo contract | [`AGENTS.md`](AGENTS.md) |

MCP surface: `eb-stack mcp` (`eb_check_recipe` / `eb_bump` / `eb_solve`).

## Solve a multi-package stack

```bash
./target/release/eb-stack solve \
  --easyconfigs fixtures/gromacs_2025_to_next/easyconfigs \
  --policy fixtures/gromacs_2025_to_next/policies/prefer_newer.json \
  --baseline-easyconfigs fixtures/gromacs_2025_to_next/easyconfigs \
  --lock-out stack.lock.json \
  --build-list-out build.list \
  --stack-diff-out stack.diff.md
```

Optional `--sbom-out` writes a planned CycloneDX inventory. Baseline generation
selection (nearest lower vs explicit) is documented in the
[solve howto](docs/orgmode/howto/solve-lock.org).

## Documentation

| Kind | Where |
|------|--------|
| Site | https://eb-stack.rgoswami.me (orgmode → Sphinx + Shibuya) |
| Tutorial | one zero-hand-fed GROMACS bump |
| How-tos | annual bump, solve, emit reports, recipe flags, root priority |
| Reference | CLI, policy JSON, lock / build-list / stack-diff formats |
| Explanation | lifecycle, architecture, fidelity, parser approach |

Build locally:

```bash
pixi run -e docs docbld
# HTML → docs/build/index.html
```

## Tests and CI

| Job | Coverage |
|-----|----------|
| `cargo test --lib` | Unit tests |
| known-bump regression | Frozen maintainer pairs (GROMACS, ScaFaCoS, MDTraj, Fiona, PuLP, numba) library + CLI |
| packaging fixtures | eOn 2.16.0 and QMCPACK 4.3.0 landable recipe sets |
| solve / reports | build-list and stack-diff emission |
| CLI smoke | release `eb-stack bump` on the GROMACS tutorial path |
| docs | org export + Sphinx build + link check |

```bash
cargo test --locked --lib
cargo test --locked --test reproduce_real_prs --test bump_emit
cargo test --locked --test eon_foss_2026_1 --test qmcpack_foss_2026_1 --test eon_packaging
```

## Citation

```text
Rohit Goswami, eb-stack (version 0.2.0), https://github.com/HaoZeke/eb-stack
```

See also [`CITATION.cff`](CITATION.cff).

## License

[MIT](LICENSE) · [Code of Conduct](CODE_OF_CONDUCT.md) · [Security](SECURITY.md) · [Contributing](CONTRIBUTING.md)
