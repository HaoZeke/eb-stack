# Agent driver contract for eb-stack

Use the matching public skill and execute it through the requested claim rung:

| Work | Skill |
|---|---|
| conda-forge or Spack → new EasyBuild package | `skills/new-package/SKILL.md` |
| existing `.eb` → new toolchain/application version | `skills/annual-bump/SKILL.md` |
| upstream easybuild-easyconfigs PR / test report | `skills/upstream-pr/SKILL.md` |
| EasyBuild do/don't (incl. #26435 class) | `skills/easybuild-dos-donts/SKILL.md` |
| EESSI-extend test / EESSI software-layer PR | `skills/eessi-extend/SKILL.md` |

## Canonical procedure

1. Run `eb-stack package inspect|plan|bump`; do not use removed flat commands.
2. Treat `package.plan.json` as the canonical build manifest, `package.sbom.cdx.json` as the planned SBOM, and `locks/*.lock.json` as Resolvo evidence.
3. Emit one `.eb` file per product profile. Keep the default profile unsuffixed and follow neighboring GROMACS/LAMMPS conventions.
4. Run `eb-stack recipe format|lint|check` on emitted recipes.
5. Run `eb-stack target doctor` before a build campaign.
6. Run or resume `eb-stack campaign run` on the configured EasyBuild target until the requested claim rung is established.
7. Use `campaign finding claim|resolve` for OMP coordination. Never edit campaign JSON directly or steal an owned finding.
8. For an **upstream easyconfigs PR**, after recipes are ready, use the EasyBuild
   CLI on the build host: `eb --check-contrib`, then
   `eb --from-pr <N> --robot --upload-test-report` (see `skills/upstream-pr/SKILL.md`).
   Do not invent substitutes for contribution checks or test-report upload.

## Non-negotiables

- Dependency versions, hierarchy choices, site pins, and fallbacks belong inside Resolvo. A package-specific `--dep` override becomes a locked solver pin.
- SAT compatibility is not build evidence. Hermes owns classified target/recipe/policy repair and repeats the campaign through verification.
- Fix code and recipes under test. Never remove assertions, dependencies, checksums, sanity checks, or tests to clear a failure.
- Run EasyBuild installs on the configured target, scheduler, and runtime. Keep `EASYBUILD_TMPDIR` on durable storage.
- Report `resolves`, `builds`, and `binary-verified` separately. A plan establishes only `resolves`; a build without declared verification does not establish `binary-verified`. `eessi-verified` is a further rung owned by `skills/eessi-extend/SKILL.md`; a build on a normal target never establishes it.
- The public issue and PR surface belongs to the human operator. Prepare paste-ready material and evidence without opening or mutating remote issues or PRs.

## Version-one CLI

```text
package  inspect | plan | bump
recipe   check | lint | format
stack    solve | sbom
target   list | doctor
campaign run | status | finding claim | finding resolve
mcp
```

The MCP catalog mirrors these workflows with `eb_`-prefixed typed tools.

## Tests

Run Rust builds and tests on the repository’s configured build machine when local compilation is prohibited. The complete gate is:

```sh
cargo test --locked --all-targets
```

CI mirrors that gate as parallel jobs covering known bumps, packaging fixtures,
foreign adapters, package bundles, catalog/closure/source discovery, profile
emission, target routing, campaign state, CLI, and MCP. Optional
`real_tree_scale` needs a local EasyBuild robot tree (`EB_EASYCONFIGS`) and is
not required on GitHub runners. No affected suite may remain red in a
completion claim.
