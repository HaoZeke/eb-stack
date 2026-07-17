# Changelog

All notable changes to this unreleased project are documented here.

## Unreleased

### Added

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
