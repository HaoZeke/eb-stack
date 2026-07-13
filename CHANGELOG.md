# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.0] - 2026-07-13

### Added

- `eb-stack ingest`: convert conda-forge (`meta.yaml` / `recipe.yaml`) and
  Spack (`package.py`, restricted parse) recipes into parseable EasyBuild
  scaffolds (name, version, source identity, dependency names; residual
  warnings for toolchain/build logic and EB generation-native dep versions).
- Frozen fixtures under `fixtures/foreign_ingest/` and integration tests
  (`tests/foreign_ingest.rs`) for library + CLI ingest paths.
- In-site Rust API docs path (sphinx-rustdocgen) and public packaging hygiene
  from the 0.2.x docs/CI workline.

### Changed

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
