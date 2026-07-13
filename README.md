<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/branding/eb-stack-logo-dark.svg">
    <img src="assets/branding/eb-stack-logo.svg" alt="eb-stack" width="420">
  </picture>
</p>

**Rewrite EasyBuild stacks onto the next toolchain generation.**

Parse `*.eb` files, `bump` with zero hand-fed dependency versions
(hierarchy consensus **and** resolvo joint pins), co-select a full stack with
**resolvo** (CDCL SAT), ingest conda-forge/Spack scaffolds, and emit a lock,
build list, optional planned CycloneDX 1.5 SBOM (`cyclonedx-bom`), and a
reviewable stack diff.

[![CI](https://github.com/HaoZeke/eb-stack/actions/workflows/ci_test.yml/badge.svg)](https://github.com/HaoZeke/eb-stack/actions/workflows/ci_test.yml)
[![Docs](https://github.com/HaoZeke/eb-stack/actions/workflows/ci_docs.yml/badge.svg)](https://github.com/HaoZeke/eb-stack/actions/workflows/ci_docs.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Docs site](https://img.shields.io/badge/docs-eb--stack.rgoswami.me-teal)](https://eb-stack.rgoswami.me)

On a measured sample of real maintainer `foss-2023b` → `foss-2024a` pairs it
reproduces the next-generation recipe exactly (or exactly modulo a hand-added
dependency) for about half the packages, and never silently emits a wrong
dependency version on the rest.

### What this is / is not (public 0.3.x)

| Ready (mechanical packaging) | Must **not** claim |
|------------------------------|--------------------|
| `bump` with hierarchy + resolvo pins; loud fail on unresolved deps | Ingest alone is a **landable EasyBuild PR** |
| `check-recipe`, `check-style` / `format-style` (E501), `solve`, lock, stack-diff, optional CycloneDX SBOM | ***builds*** without a real `eb --robot` install |
| `ingest` scaffolds + residual honesty (warnings, 0.0.0 placeholders) | Product `configopts` invented inside this binary |
| CI: lib, known-bump fixtures, packaging fixtures, foreign ingest | Residual judgment without an agent + live `eb` / robot tree |
| Skills + docs for annual bump and greenfield bootstrap | Agents open/edit upstream PRs by default |

**Does:** mechanical majority of annual toolchain bumps, greenfield **bootstrap**
from conda-forge/Spack, and resolve/packaging gates against a robot tree.

**Does not:** replace EasyBuild style or contribution gates, a virgin stack
install, or maintainer judgment (product flags, companions when the robot has
holes, PR text). Upstream PR surface is **human-owned**.

Three claims (never conflate):

1. ***resolves*** — `eb-stack check-recipe` / plan complete  
2. ***builds*** — `eb --robot` green (module exists)  
3. ***binary-verified*** — module loads; binary runs / links correctly  

`eb-stack` proves the first rung. EasyBuild and site ops own the rest.
Operator skills: [`skills/annual-bump/`](skills/annual-bump/SKILL.md),
[`skills/new-package/`](skills/new-package/SKILL.md).

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

Every dependency version comes from the universe (hierarchy + resolvo joint
pins); no `--dep` flags. The only intentional gap versus the real maintainer
`foss-2024a` recipe is a hand-added `pybind11` line the tool correctly does
not invent.

Full walkthrough: [tutorial](https://eb-stack.rgoswami.me/tutorial.html) ·
source: [`docs/orgmode/tutorial.org`](docs/orgmode/tutorial.org).

## Operator / agent skills

| Work | Human guide | Agent skill |
|------|-------------|-------------|
| Annual generation rebuild | [`docs/orgmode/howto/run-annual-bump.org`](docs/orgmode/howto/run-annual-bump.org) | [`skills/annual-bump/SKILL.md`](skills/annual-bump/SKILL.md) |
| **New package** (conda-forge / Spack → EB) | [`docs/orgmode/howto/new-package.org`](docs/orgmode/howto/new-package.org) · CLI *ingest* | [`skills/new-package/SKILL.md`](skills/new-package/SKILL.md) |
| Repo contract | | [`AGENTS.md`](AGENTS.md) |

MCP surface: `eb-stack mcp` (`eb_check_recipe` / `eb_bump` / `eb_solve`).
Ingest is CLI-only today (`eb-stack ingest`). Skills assume a real EasyBuild
host (site: `rg.surf` + herdr for residual agents); cargo builds for *this*
repo use the site cargo builder (`rg.terra`), not as an EasyBuild substitute.

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

Optional `--sbom-out` writes a planned CycloneDX **1.5** inventory via
`cyclonedx-bom`. Baseline generation selection (nearest lower vs explicit) is
documented in the [solve howto](docs/orgmode/howto/solve-lock.org).

## Ingest foreign recipes (conda-forge / Spack)

Scaffold a parseable EasyBuild `.eb` from a foreign recipe. Identity fields
come from the input; pass `--easyconfigs` for generation-native dep versions
(hierarchy + resolvo, same as `bump`).

**Ingest is not a landable PR.** Product `configopts`, multi-source
`extract_cmd`, real sanity paths, companion easyconfigs (robot holes), and
style gates remain residual work for a human or local-ai agent following
[`skills/new-package/SKILL.md`](skills/new-package/SKILL.md) — never hardcoded
into this tool. After residual edits: `eb --inject-checksums`,
`eb-stack format-style` (E501 ≤120, mechanical), `eb --check-contrib`,
`eb-stack check-recipe`, then `eb -Dr` / `eb --robot`
for the *builds* rung.

```bash
./target/release/eb-stack ingest \
  --source fixtures/foreign_ingest/conda_zlib/meta.yaml \
  --toolchain-name foss --toolchain-version 2024a \
  --out /tmp/zlib-from-conda.eb

./target/release/eb-stack ingest \
  --source fixtures/foreign_ingest/spack_eon/package.py \
  --format spack \
  --toolchain-name foss --toolchain-version 2024a \
  --easyconfigs fixtures/hierarchy_resolve/easyconfigs \
  --keep-old-deps \
  --out /tmp/eon-from-spack.eb
```

Landable packaging **oracles** (resolve/`check-recipe` only — not a *builds*
claim) live under `fixtures/eon_foss_2026_1/` and
`fixtures/qmcpack_foss_2026_1/` (companions such as CapnProto for eOn when
develop still lacks them).

## Version

```bash
eb-stack --version   # crate version (Cargo.toml); tag releases as vX.Y.Z
```

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
cargo test --locked --test foreign_ingest
```

Packaging fixture tests prove **parse / resolve / packaging_gate** against
frozen recipes — not a virgin `eb --robot` install.

## Citation

```text
Rohit Goswami, eb-stack (version 0.3.0), https://github.com/HaoZeke/eb-stack
```

See also [`CITATION.cff`](CITATION.cff).

## License

[MIT](LICENSE) · [Code of Conduct](CODE_OF_CONDUCT.md) · [Security](SECURITY.md) · [Contributing](CONTRIBUTING.md)
