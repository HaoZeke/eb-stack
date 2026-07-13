---
name: eb-stack-new-package
description: Author a new EasyBuild easyconfig from a conda-forge or Spack recipe with eb-stack ingest. Mechanical parse, robot hierarchy + resolvo dep pins, check-recipe loop; residual product flags/checksums/companions are judgment. Pair with annual-bump for generation rebuilds and build/PR ops.
---

# New package from conda-forge / Spack with eb-stack

You are bringing **new software** into an EasyBuild stack (greenfield recipe),
usually by deriving from a real foreign recipe:

- conda-forge `meta.yaml` (classic) or `recipe.yaml` (rattler-build v1)
- Spack `package.py` (restricted static parse — no Python execution)

`eb-stack ingest` does the mechanical majority: identity fields, sources,
dependency *names*, static configure flags when the foreign DSL states them,
optional **generation-native** dependency versions from a robot tree. You drive
the loop and close the residuals it names. It never silently invents product
flags, landable PR layout, or fake checksums.

**What this skill closes:** foreign → parseable EasyBuild scaffold; hierarchy +
resolvo pins when `--easyconfigs` is set; `check-recipe` until the plan
*resolves*; optional companion scaffolds for missing deps; honest residual
warnings. **What it does not close:** full product configopts/patches/ctest
surface that only exists in hand-maintained PR recipes, real `eb --robot`
builds, binary verification, and the PR surface (human-only). For those ops
and the three-claim ladder, follow `skills/annual-bump/SKILL.md` §10.

**Related skill:** generation rebuild of *existing* recipes is
`skills/annual-bump/SKILL.md` (bump + solve), not this skill.

## 0. What you need first

1. **The tool.** `cargo build --release` in the eb-stack repo; `eb-stack` on
   `PATH`. Confirm `eb-stack --version` (expect ≥ 0.3.0 for ingest + resolvo).
2. **A foreign recipe path.** Real file: feedstock `recipe.yaml` / `meta.yaml`,
   or Spack `package.py` (can be frozen under `fixtures/foreign_ingest/` for
   regression). Prefer official release artifacts over floating `develop`.
3. **Target toolchain generation.** Operator-chosen (e.g. current upstream
   develop `foss-2026.1`, or site `foss-2024a`). Never invent a generation from
   the foreign recipe.
4. **Robot easyconfig tree.** Clone or install of `easybuild-easyconfigs` (and
   optional site overlay). Required for generation-native dep pins; without it
   you only get residual foreign floors / `0.0.0` work-queue stubs.

## 1. The mental model (read once)

- **Deterministic (the tool):**
  1. *Parse* foreign DSL into intermediate fields (name, version, sources,
     sha256 when present, dep names + residual pins, static `-D…` flags).
  2. *Emit* a reparseable `.eb` scaffold (easyblock guess from bases/deps,
     multi-source layout, EB title casing for known names like `eOn` /
     `QMCPACK`).
  3. *With `--easyconfigs`:* hierarchy consensus then **resolvo joint pins**
     for deps that exist under the generation (same path as
     `bump --easyconfigs`). Toolchain virtuals (`blas`/`mpi`/…) and conda
     packaging noise (`pip`/`setuptools`) are skipped, not invented as modules.
- **Judgment (yours):** product `configopts` / variants, real tarball sha256
  when foreign only has a git tag, multi-source `extract_cmd` layout, greenfield
  companions the robot lacks, cross-generation hand pins, patches, sanity paths,
  `toolchainopts`, moduleclass polish.

Warnings on stderr are the work queue. Residual foreign pins are labelled;
robot pins name `hierarchy consensus` / `resolvo joint`.

## 2. Ingest one package (the core command)

### 2.1 Scaffold only (no robot)

```
eb-stack ingest \
  --source path/to/meta.yaml   # or recipe.yaml / package.py \
  --format auto \              # or conda-forge | spack
  --toolchain-name foss \
  --toolchain-version 2026.1 \
  --out out/Name-Ver-foss-2026.1.eb
```

Use when you want identity fields only, or the robot is offline. Dep versions
are residual — **not** production pins.

### 2.2 Scaffold + generation-native deps (preferred)

```
ROBOT=$HOME/.venvs/easybuild/easybuild/easyconfigs
# or a clone of easybuild-easyconfigs/easybuild/easyconfigs

eb-stack ingest \
  --source path/to/package.py \
  --format spack \
  --toolchain-name foss \
  --toolchain-version 2024a \
  --easyconfigs "$ROBOT" \
  --keep-old-deps \
  --out-dir drafts/easyconfigs
```

- `--easyconfigs` is **repeatable** (later path wins on identity conflict). Put
  the upstream robot first, then a site/draft overlay when you have companions.
- `--keep-old-deps`: keep residual foreign floors when the robot has no
  candidate (loud warnings). Default without it: drop unresolved `0.0.0` lines
  after robot resolve (or fail paths depending on residual policy).
- `--out-dir` writes letter/name/conventional basename layout
  (`q/QMCPACK/QMCPACK-4.3.0-foss-2024a.eb`).

### Worked examples (copy-paste; fixtures in-repo)

```
REPO=<path-to-eb-stack>
ROBOT=$HOME/.venvs/easybuild/easybuild/easyconfigs

# conda-forge eOn (rattler recipe.yaml)
eb-stack ingest \
  --source $REPO/fixtures/foreign_ingest/conda_eon/recipe.yaml \
  --toolchain-name foss --toolchain-version 2024a \
  --easyconfigs "$ROBOT" --keep-old-deps \
  --out /tmp/eOn-2.16.0-foss-2024a.eb

# Spack QMCPACK
eb-stack ingest \
  --source $REPO/fixtures/foreign_ingest/spack_qmcpack/package.py \
  --format spack \
  --toolchain-name foss --toolchain-version 2024a \
  --easyconfigs "$ROBOT" --keep-old-deps \
  --out /tmp/QMCPACK-4.3.0-foss-2024a.eb
```

Expect: `name` title-cased (`eOn`, `QMCPACK`), correct version, `MesonNinja` or
`CMakeNinja`, robot pins for CMake/Python/Boost/HDF5/… when present in the
tree, warnings for product residuals.

## 3. Residual decision tree (judgment only)

Each case is signalled by a WARNING or by `check-recipe` missing-dep hints.

1. **Product configopts / variants.** Symptom: scaffold has no or only static
   `-D…` flags; foreign recipe uses f-strings, selectors, or Spack `when=`.
   Action: author product flags from the project's build docs or from a
   landable PR fixture — never invent silently. Optional:
   `check-recipe --require-configopt=FLAG` once flags exist.
2. **Missing / zero checksum.** Symptom: 64-zero placeholder or "no sha256".
   Action: take sha256 from the release asset (or EasyBuild
   `--inject-checksums` after fetch). Prefer release tarballs over
   `archive/refs/tags`.
3. **Missing robot deps / companions.** Symptom: `check-recipe` missing with
   "available at other generations: …" or no candidate. Action: bump or author
   companions; or
   `check-recipe --scaffold-missing DIR` for letter-layout stubs, then fill
   sources/checksums for real greenfield.
4. **Foreign pin vs generation consensus.** Symptom: residual foreign floor
   kept under `--keep-old-deps`, or hierarchy pin differs from foreign range.
   Action: accept hierarchy/robot pin unless you know a freeze reason; then
   hand-edit or later `bump --dep Name=ver`.
5. **Toolchain virtuals / conda macros.** Symptom: skipped `blas`/`mpi`/
   `compiler(...)` warnings. Action: none for the scaffold — foss/GCCcore
   already provide them. Do not re-add as fake modules.
6. **Multi-source extract layout.** Symptom: multi-source list without
   `extract_cmd` for subprojects. Action: hand-author extract placement when
   meson wraps / subprojects need a specific path (see eOn PR fixtures).

## 4. What the tool guarantees

- Identity fields track the foreign input (no unexpanded `${{…}}` after
  restricted template expand).
- Dependency versions with `--easyconfigs` are **generation-native** when the
  robot has a hierarchy candidate — not free global newest, not silent invent.
- Resolvo joint-checks hierarchy pins; on deep-tree unsat it **falls back** to
  hierarchy consensus with a warning (does not invent pins).
- Re-parse under `eb_parse` / EasyBuild-shaped DSL succeeds for the scaffold.
- Residuals are **warned**, never silent product claims.

## 5. Verify each emitted recipe

```
eb-stack check-recipe \
  --recipe out/Name-Ver-foss-YYYY.eb \
  --easyconfigs "$ROBOT" \
  --easyconfigs drafts/easyconfigs   # if companions live here
```

- Exit 0, 0 missing → claim **resolves** only (see annual-bump §10.4).
- Packaging findings (`checksums` order): fix the list (sources then patches).
- Re-parse / diff against foreign source identity: name, version, primary URL.
- Do **not** claim *builds* or *binary-verified* until `eb --robot` and load
  tests (annual-bump §10).

## 6. Companions and missing graph

When the new package needs software the robot lacks:

```
eb-stack check-recipe \
  --recipe out/App.eb \
  --easyconfigs "$ROBOT" \
  --scaffold-missing drafts/easyconfigs
```

Then either author real recipes for those companions (sources + checksums +
deps) or ingest *their* foreign recipes the same way. Overlay order for
check-recipe: robot first, drafts second so companions win.

## 7. The loop, end to end

1. Obtain foreign recipe + choose target generation + robot path.
2. `ingest` with `--easyconfigs` (and `--keep-old-deps` while iterating).
3. Read WARNINGs; close residuals §3 that block a coherent plan.
4. `check-recipe` until resolves (or document remaining missing with generation
   hints as the work queue).
5. Author product flags / checksums / extract_cmd as needed for landability.
6. Build/verify only on a build machine via scheduler (annual-bump §10.1–10.2).
7. PR surface is human-only (annual-bump §10.5). One PR per recipe set;
   target **current development generation** for new upstream software.

## 8. Driver

Same as annual-bump: skill names no model. Any capable agent or human executes
it; mechanical steps refuse to guess. Site runbook holds host-specific
incantations.

## Quick reference

| Task | Command |
|------|---------|
| Ingest conda-forge | `eb-stack ingest --source meta.yaml\|recipe.yaml --toolchain-name foss --toolchain-version GEN --out out.eb` |
| Ingest Spack | `eb-stack ingest --source package.py --format spack --toolchain-name foss --toolchain-version GEN --out out.eb` |
| + robot pins | add `--easyconfigs ROBOT [--easyconfigs OVERLAY] --keep-old-deps` |
| Letter layout | `--out-dir drafts/easyconfigs` instead of `--out` |
| Check plan | `eb-stack check-recipe --recipe X.eb --easyconfigs ROBOT` |
| Scaffold missing deps | add `--scaffold-missing DIR` |
| Assert product flags | add `--require-configopt=FLAG` (repeatable) |
| MCP tools | `eb-stack mcp` (`eb_check_recipe` / `eb_bump` / `eb_solve`; ingest is CLI today) |

## Reality check (measured fixtures)

| Foreign fixture | Mechanical claim | Not claimed |
|-----------------|------------------|-------------|
| `fixtures/foreign_ingest/conda_eon/` | eOn 2.16.0, MesonNinja, multi-source, robot pins for common stack deps | full product `-Dwith_*`, extract_cmd, 2026.1 landable set |
| `fixtures/foreign_ingest/spack_qmcpack/` | QMCPACK 4.3.0, CMakeNinja, tag archive, robot Boost/HDF5/Python/… | real release sha256, ctest/Nexus, MPI product surface |
| Landable PR sets | `fixtures/eon_foss_2026_1/`, `fixtures/qmcpack_foss_2026_1/` | hand-authored product; use annual-bump §11 check-recipe recipes |

Regression:

```
cargo test --locked --test foreign_ingest
cargo test --locked --lib foreign
```

## 9. Operational contract (shared with annual-bump)

Do **not** re-implement host/build/PR rules here. Follow
`skills/annual-bump/SKILL.md`:

- §10.1 eb runtime contract  
- §10.2 scheduler discipline  
- §10.3 checksums / missing-dep hints  
- §10.4 three-claim ladder (*resolves* / *builds* / *binary-verified*)  
- §10.5 PR discipline  
- §10.6 build-to-completion loop  

## 10. Routing: which skill?

| Situation | Skill |
|-----------|--------|
| New software from conda-forge / Spack (or greenfield scaffold) | **This skill** (`new-package`) |
| Existing `.eb` → new toolchain generation | **annual-bump** |
| Stack lock / build list / stack diff for a set | annual-bump §7 (`solve`) |
| After ingest, generation rebuild of companions | annual-bump `bump` |

Operator-facing annual guide: `docs/orgmode/howto/run-annual-bump.org`.
Ingest CLI reference: `docs/orgmode/reference/cli.org` (*ingest*).
