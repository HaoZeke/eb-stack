# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Podman Rocky 9 build backend** for greenfield *builds*:
  `skills/new-package/container/rocky9/` (`Containerfile`, `eb-in-podman`).
  `render-full-drive --build-backend podman-rocky9` (default) runs
  `eb --robot` in-container; host OS workarounds stay optional under
  `overlays/` only for `--build-backend host`.


- **`eb-stack check-style` / `format-style`**: mechanical pycodestyle E501
  (max 120 columns) lint and rewrite for easyconfig string assignments
  (`key = '…'` / `key += "…"` → parenthesized adjacent literals) and `#`
  comments. Residual queue gains `kind: "style"` items pointing at
  format-style — line wrapping is **not** residual judgment. Skills and
  mechanical sequence updated so Hermes/local-ai never hand-split E501.
- Agent skill for **new packages** from conda-forge / Spack:
  `skills/new-package/SKILL.md` (paired with `skills/annual-bump/` for
  generation rebuilds). `AGENTS.md` routes by work type.
- Operator howto for greenfield packages:
  `docs/orgmode/howto/new-package.org` (linked from README and the docs map).
- Landable eOn foss-2026.1 fixture companion
  `CapnProto-1.4.0-GCCcore-15.2.0.eb` so robot-style resolve of CapnProto
  matches the PR surface (serve feature) without inventing versions in
  product code.
- **`ingest --residual-queue`** (default: `{stem}.residuals.json` beside the
  scaffold): machine-readable residual work queue for agents (dep versions,
  product_config / moduleclass / sanity / checksum gaps). Claim ladder in
  the JSON always marks *resolves*/*builds* as not established by ingest.
- MCP tool **`eb_ingest`**: same path as CLI ingest + residual queue.

### Changed

- **new-package skill**: campaign narrative is container-first and host-agnostic; per-distro host hacks demoted to `overlays/README.md`.

- **full-drive PATH**: use absolute EasyBuild `eb` vs `eb-stack`; venv bin first so a release-dir `eb`→`eb-stack` symlink cannot steal `--robot`.

- **new-package skill §7 full-drive default**: local-ai agent (Hermes/herdr on
  `EasyBuild host`) owns residual judgment **and** `eb --robot` *builds* for PR-ready
  campaigns; residual-only only when human scopes it. Stopping after `eb -Dr`
  without install is not done.
- **Arch OS-dep policy in full-drive**: rendered `full-drive.sh` auto-passes `--ignore-osdeps` when `/usr/include/infiniband/verbs.h` exists (Arch `rdma-core`); teaches campaign agents not to chase Debian package names. Optional Debian Docker fallback under `skills/new-package/docker/eb-arch-host-fallback/`.
- **`render-full-drive`**: templates
  (`skills/new-package/templates/full-drive.sh.tmpl`,
  `hermes-full-drive.md.tmpl`) + renderer that emits per-campaign
  `WORK/residuals/full-drive.sh` and `hermes-full-drive.md` from
  `--work` / `--robot` / repeated `--recipe` [`--oracle`/`--stem`].
  Example: `examples/render-eon-qmcpack.sh`.
- **`check-recipe` hierarchy membership for unpinned deps**: a candidate on
  an out-of-generation GCCcore (e.g. CapnProto only on 14.3.0) no longer
  false-passes a newer foss recipe; explicit cross-gen pins still match.
  Closer to EasyBuild robot behaviour without inventing companion recipes.
- Ingest WARNING comments wrap to ≤116 columns (mechanical style noise).
- **Mechanical-first residual policy** in skills: maximize CLI steps
  (`ingest`, inject-checksums, check-contrib, check-recipe, `eb -Dr`);
  local-ai agent (Hermes preferred, OMP allowed) only for judgment
  residuals — never hardcode product `configopts` into ingest.
- **Host split** for SURF ops: EasyBuild authoring, residual agents, and
  `eb --robot` *builds* on **`EasyBuild host`**; residual agents in a **herdr**
  pane (not ad-hoc `ssh … hermes/omp -p`). **`cargo builder` is cargo-only**
  for this repo’s Rust compile — not the EasyBuild install host.
- README public framing for **0.3.x**: ready vs must-not-claim table;
  three-claim ladder (*resolves* / *builds* / *binary-verified*); ingest
  scaffold ≠ landable PR; human-owned PR surface.
- Skills + `AGENTS.md` aligned with residual-queue JSON, hierarchy-aware
  `check-recipe`, MCP `eb_ingest`, and herdr residual agents on `EasyBuild host`.

## [0.3.0] - 2026-07-13

### Added

- `eb-stack ingest`: convert conda-forge (`meta.yaml` / `recipe.yaml`) and
  Spack (`package.py`, restricted parse) recipes into parseable EasyBuild
  scaffolds (name, version, sources, static configopts, dependency names).
  With `--easyconfigs`, fills generation-native dep versions via hierarchy
  consensus + resolvo joint pins (same path as `bump`).
- Frozen fixtures under `fixtures/foreign_ingest/` and integration tests
  (`tests/foreign_ingest.rs`) for library + CLI ingest paths.
- In-site Rust API docs path (sphinx-rustdocgen) and public packaging hygiene
  from the 0.2.x docs/CI workline.

### Changed

- `bump --easyconfigs` auto-resolve is **two-stage**: hierarchy consensus
  floors, then resolvo CDCL SAT with those versions as exact pins (joint
  feasibility). No longer hierarchy-only independent lookup.
- Planned SBOM emission uses the official `cyclonedx-bom` crate (CycloneDX
  1.5 JSON: `serialNumber`, tools, pre-build lifecycle; runtime `dependsOn`;
  build edges as `eb_stack:buildDependsOn`). Lock-only SBOM no longer invents
  all-to-all co-stack edges.
- Project version surfaces (`Cargo.toml`, `pixi.toml`, `CITATION.cff`, docs
  `release`, binary `--version`) aligned at **0.3.0**.

## [0.2.0] - 2026-07-10

### Added

- Hierarchy-aware `bump --easyconfigs` with loud fail on unresolved deps.
- `check-recipe` with nearest-generation missing-dep hints and positional
  checksum packaging gate.
- `solve` lock, optional CycloneDX SBOM, build list, and stack-diff markdown.
- MCP tool surface (`eb-stack mcp`: `eb_check_recipe`, `eb_bump`, `eb_solve`).
- Annual-bump skill and agent driver contract (`skills/annual-bump/`,
  `AGENTS.md`).
- Frozen eOn and QMCPACK foss-2026.1 packaging fixtures.
- GitHub Actions CI for unit tests, known-bump regression, packaging fixtures,
  solve/stack-diff, and CLI smoke.
- Operator guide for the annual toolchain-generation bump
  (`docs/orgmode/howto/run-annual-bump.org`).
- CLI auto-bump regression tests for frozen maintainer pairs under
  `tests/repro_fixtures/`.

### Changed

- Hierarchy derivation from the robot tree for GCC-family generations;
  fixture hierarchy remains the escape hatch for non-GCC toolchains.

## [0.1.0] - 2026-06-01

### Added

- Initial parse / resolvo co-select / planned SBOM path and GROMACS fixtures.
