# Security Policy

eb-stack turns conda-forge, Spack, and EasyBuild inputs into manifests, planned
CycloneDX SBOMs, Resolvo locks, and EasyBuild recipes. Planning commands parse
local input and write local artifacts. Campaign commands execute EasyBuild recipes
on a configured local or SSH target, optionally through Slurm, Podman, or Docker.

## Supported versions

The project has no public release. Security fixes apply to the current `main`
branch until versioned releases are published.

## Trust boundaries

- Treat every foreign recipe, easyconfig, patch, and source archive as
  untrusted recipe input. EasyBuild recipes are executable Python and package
  builds run upstream build scripts.
- Review a generated bundle before running a campaign. Use a dedicated build
  account or scheduler allocation, least-privilege SSH credentials, durable
  temporary storage, and a target without unrelated secrets.
- A container limits ABI contamination but is not automatically a security
  boundary. Avoid privileged containers and mount only the source, robot,
  bundle, build, install, and temporary roots required by the target.
- Keep credentials out of target TOML. Use the SSH agent, host configuration,
  scheduler credentials, or runtime credential facilities supplied by the
  deployment site.
- A planned SBOM records the selected package intent. It does not prove that a
  build is safe or that the installed filesystem contains only those
  components; use the campaign claim ladder and site provenance controls.

## Reporting a vulnerability

Email the maintainer at [rgoswami@ieee.org](mailto:rgoswami@ieee.org) with a
description, reproducer, affected command, and expected impact. Do not open a
public issue for security matters or include credentials and private target
configuration in a report.

Relevant reports include unintended command execution during parsing, path
traversal when writing bundles or campaign state, target-routing escapes,
credential disclosure, unsafe container mounts, and supply-chain problems in
release artifacts.

Treat `Cargo.lock` and the release binary provenance as part of the security
surface.
