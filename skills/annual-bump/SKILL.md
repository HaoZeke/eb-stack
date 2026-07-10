---
name: eb-stack-annual-bump
description: Move an EasyBuild software stack onto a new toolchain generation (the annual rebuild) with eb-stack. A complete operational runbook - any capable agent or engineer can execute it end to end. The tool does the mechanical majority and fails loudly; you handle a small, bounded set of judgment calls it names for you.
---

# Annual toolchain-generation bump with eb-stack

You are moving a set of EasyBuild easyconfigs from one toolchain generation to the
next (for example `foss-2023b` -> `foss-2024a`). `eb-stack` rewrites each recipe
mechanically and resolves every dependency version itself; you drive the loop and
resolve the handful of cases it cannot decide. Expect it to reproduce a real
maintainer update mechanically for roughly 60% of packages and to tell you
exactly what is left on the rest. It never silently emits a wrong version.

## 0. What you need first

1. **The tool.** Build once: in the eb-stack repo, `cargo build --release`; put
   `target/release/eb-stack` on `PATH`. Confirm `eb-stack --version`.
2. **The easyconfig universe.** A directory tree of `.eb` files to draw
   dependency versions from - an EasyBuild install's `easyconfigs/` tree, or a
   clone of `easybuild-easyconfigs`. If you also have a site overlay of custom
   recipes, note its path; you can pass both (overlay wins on conflict).
3. **The build list.** The set of recipes to rebuild for the new generation
   (toolchains first, then libraries, then applications). Often the previous
   generation's list, retargeted.

## 1. The mental model (read once)

- **Deterministic (the tool does it, and refuses to guess):** rewrite
  `toolchain`; resolve every `dependencies` and `builddependencies` version from
  the universe across the target generation's sub-toolchain hierarchy
  (`GCCcore`/`GCC`/`gfbf`/`gompi`/... down to `SYSTEM`) using the generation's
  consensus version; preserve templates, `local_*`, `SYSTEM`, versionsuffix and
  pin qualifiers, `exts_list`, and every unchanged line verbatim.
- **Judgment (yours):** a genuinely new dependency the maintainer adds for the
  new version; the source tarball checksum on a version bump; whether a
  version-bump's patches still apply; a maintainer's one-off decision to freeze
  or downgrade a single dependency. The tool cannot invent these and will not
  pretend to - it warns or exits non-zero instead.

If a run exits non-zero or prints a warning, that is a decision point for you,
never a silent failure.

## 2. Bump one recipe (the core command)

```
eb-stack bump \
  --source path/to/App-<ver>-<oldtc>.eb \
  --toolchain-name foss --toolchain-version 2024a \
  --easyconfigs path/to/easyconfigs \
  --out out/App-<ver>-foss-2024a.eb
```

- Dependency versions are resolved automatically from `--easyconfigs`. You do not
  hand-feed them. To override one, add `--dep Name=version` (repeatable).
- Multiple universes with precedence (upstream plus a site overlay): pass
  `--easyconfigs` more than once; a later path wins on conflict.

### Worked example (copy-paste reliable)

```
eb-stack bump \
  --source GROMACS-2024.4-foss-2023b.eb \
  --toolchain-name foss --toolchain-version 2024a \
  --easyconfigs /path/to/easyconfigs \
  --out GROMACS-2024.4-foss-2024a.eb
```

This emits the new recipe with `toolchain` set to `foss-2024a`, and CMake,
Python, SciPy-bundle, networkx, mpi4py, and scikit-build-core each resolved to
their `foss-2024a` generation versions - byte-identical to what the maintainer
shipped, except for any dependency the maintainer added by hand (which the tool
correctly does not invent; see 4.1).

## 3. Version bump vs toolchain bump

- **Toolchain bump** (same app version, new generation): fully mechanical. The
  command above is all you need.
- **Version bump** (the application version also changes): add `--version
  <newver>` and the new source checksum `--source-checksum <sha256>`. Without the
  checksum the tool renames the source tarball key to the new version and WARNS
  that the checksum is stale - resolve it (see 4.2) before shipping.

## 4. The residual decision tree (what to do when the tool hands one back)

The tool reproduces the mechanical part; these four cases are yours. Each is
signalled by a warning or a diff against the previous-generation recipe.

1. **A genuinely new dependency.** The new application version needs a dependency
   absent from the source recipe. Symptom: diff vs upstream shows the maintainer
   added a dep the tool did not. Action: add it from the upstream project's
   requirements for that version; resolve its version with a second
   `eb-stack bump`/`solve` pass or by hand.
2. **Stale source checksum (version bumps).** Symptom: a "checksum is stale"
   warning. Action: get the real sha256 from the upstream release, or from a
   sibling recipe of the same new version, or run EasyBuild's own checksum
   injection; re-run with `--source-checksum`.
3. **Patch set on a version bump.** Symptom: the source's `patches` reference the
   old version. Action: review whether each patch still applies to the new
   version; swap to the new version's patch set.
4. **A single-dependency freeze or downgrade the maintainer chose.** Symptom: one
   dependency version differs and it is not a mechanical mismatch (the tool
   resolved the generation-standard version, the maintainer deliberately pinned a
   different one). Action: accept the tool's version unless you know the upstream
   reason to override; add `--dep Name=version` if you must pin it.

Everything else in a diff - reordered blocks, quote-style, blank lines,
description rewrapping, a compiler swap like Qt5 -> Qt6 - is maintainer cosmetic
or structural change, not a version error. The tool's recipe is correct and
buildable; match the maintainer's formatting only if your review requires it.

## 5. What the tool guarantees (so you can trust the output)

- It resolves the **generation-native** version of each dependency, not the
  globally newest, and never a version older than the source's.
- It **never silently keeps a stale dependency**: an unresolved dependency is a
  loud warning and a non-zero exit (unless you pass `--keep-old-deps`).
- It respects **pins**: versionsuffix-qualified and `SYSTEM`-toolchain
  dependencies are preserved, not bumped.
- It parses **real easyconfigs** (templates, `local_*`, `SYSTEM`, multi-element
  tuples, `exts_list`), validated against EasyBuild's own parser, and skips a
  file it cannot parse rather than aborting the whole run.

## 6. Verify each emitted recipe

- Re-parse it (EasyBuild's own parser is the ground truth): a syntax check plus a
  read-back of name/version/toolchain/deps.
- Diff it against the previous-generation recipe and confirm only the intended
  fields changed.
- Sanity-check dependency existence for the target generation:
  `eb-stack check-recipe` resolves one recipe and verifies its deps exist in the
  robot tree(s). A missing dep is reported with the generations where the
  package DOES exist ("available at other generations: ...") — that hint is
  your work queue, not decoration. The same gate also lint-checks the
  `checksums` list against EasyBuild's positional convention (sources first,
  then patches); a "packaging" finding means reorder the list, never bypass.

## 7. Solve the whole set and emit the build list

Once the recipes are bumped, produce a jointly consistent stack and the artifacts
a pipeline consumes:

```
eb-stack solve \
  --easyconfigs path/to/easyconfigs \
  --policy policy.json \
  --baseline-easyconfigs path/to/previous-generation \
  --lock-out stack.lock.json \
  --build-list-out build-list.txt \
  --stack-diff-out stack-diff.md
```

`solve` returns the globally newest jointly-consistent stack under the policy's
declared `root_priority`; `stack-diff.md` is a reviewable summary against the
baseline. (CycloneDX SBOM output is opt-in via `--sbom-out`, not part of the core
loop.)

## 8. The loop, end to end

For each recipe in the build list:
1. `bump` it onto the new generation (deps auto-resolved).
2. If it exits non-zero or warns, resolve the residual (section 4) and re-run.
3. Verify (section 6); keep the diff for review.
4. When the set is bumped, `solve` for joint consistency and emit the build list.
5. Open the change as a reviewable PR into your easyconfigs repo; a human (and
   your build pipeline) reviews before install. Do not push generated recipes to
   a build without review — and follow the PR discipline in §10.5 to the letter
   (one PR per recipe set, duplicate check first, PR surface is human-only).

## 9. Driver

This runbook names no model. Any capable agent, or a human, can execute it; the
tool guarantees correctness on the mechanical steps and refuses to guess the
rest, so the driver only handles the four bounded residual cases. If you use an
in-house or hosted LLM as the driver, it needs only API access - no EasyBuild
semantics live in the prompt.

## Quick reference

| Task | Command |
|------|---------|
| Bump one recipe (auto deps) | `eb-stack bump --source X.eb --toolchain-name foss --toolchain-version 2024a --easyconfigs DIR --out Y.eb` |
| Override one dep | add `--dep Name=version` |
| Version bump | add `--version V --source-checksum SHA` |
| Overlay universe | pass `--easyconfigs` twice (later wins) |
| Keep unresolved deps (opt-in) | add `--keep-old-deps` |
| Check a recipe's deps exist | `eb-stack check-recipe --recipe X.eb --easyconfigs DIR` (repeat `--easyconfigs` for overlays; `--require-configopt=FLAG` asserts the config surface) |
| Solve stack + build list | `eb-stack solve --easyconfigs DIR --policy P.json --lock-out L.json --build-list-out B.txt --stack-diff-out D.md` |

## Reality check

On a real `easybuild-easyconfigs` sample, `eb-stack` mechanically reproduced the
maintainer's next-generation recipe (exactly, or exactly-modulo-a-hand-added-dep)
for about 60% of packages; the rest differ only by maintainer judgment
(cosmetic/structural rewrites, hand-added deps or patches, one-off pins) that no
mechanical tool can or should invent. Treat it as: it does the mechanical
majority correctly and never silently wrong, and it hands you a short, named list
of judgment calls. That is what makes the annual bump tractable for one
agent-plus-human instead of a person-month of hand edits.

## 10. Operational contract (every line below cost a real build cycle once)

These are not tips; treat them as the execution contract. All were paid for
during the 2026-07 eOn/QMCPACK/OpenMPI campaigns.

### 10.1 The eb runtime contract (per machine, before ANY build)

`eb` needs exactly five things — and "needs EESSI" is never one of them:

1. **Python ≥3.9 on PATH** (venv activate, or `uv python install 3.12` +
   `uv venv` + `uv pip install easybuild` on hosts with ancient system
   python). Symptom when missing: "No compatible 'python' command".
2. **A modules tool with the `lmod` BINARY on PATH** — the `module` shell
   function is not enough, and profile.d init scripts silently no-op in
   non-interactive shells. Source the real init
   (`/usr/share/lmod/lmod/init/bash`, `/opt/ohpc/admin/lmod/lmod/init/bash`)
   *before* `set -u`, then prepend `$(dirname $LMOD_CMD)` to PATH; export
   the login shell's MODULEPATH explicitly in batch scripts (batch shells
   often get an empty one).
3. **Robot paths** (`--robot`/`EASYBUILD_ROBOT_PATHS`; overlay dir first
   when your drafts must win).
4. **A starting compiler** — the system gcc suffices for a full bootstrap;
   OpenHPC-style compute nodes may ship *no* compiler (load `gnu12`-class
   module) and may lack glibc headers entirely (then SYSTEM-level builds
   are impossible there: reuse the site toolchain modules instead).
5. **Clean paths**: set `EASYBUILD_TMPDIR` under scratch so failure logs
   survive node-local `/tmp`; clear stale `<installpath>/software/.locks/`
   or pass `--ignore-locks`; on non-deb/rpm hosts pass `--ignore-osdeps`
   (osdependencies use dpkg/rpm names that can never match).

Debug order when eb fails before building anything: python → lmod binary →
MODULEPATH → robot paths → locks → osdeps. Never "needs EESSI".

### 10.2 Scheduler discipline

Heavy builds go through the batch scheduler, never a login shell and never
a shared tmux: a Slurm job owns its cgroup (shared-cgroup builds get
kernel-OOM-killed by *other* processes' memory pressure). Size `--mem` to
what the node actually has free (`scontrol show node`: RealMemory minus
AllocMem) and match `EASYBUILD_PARALLEL` to `--cpus-per-task`. Never
cancel or delay jobs you did not create.

### 10.3 Recipe correctness rules the tool now enforces

- **Checksums are positional**: all `sources` entries first, then
  `patches`, and a multi-arch dict is ONE entry. `check-recipe` fails on a
  count mismatch and on a patch-keyed checksum in a source slot — do not
  bypass the finding; reorder the list.
- **Missing deps come with a hint** ("available at other generations:
  …"): that list is your work queue — bump from the newest listed
  generation or author greenfield if the hint says no candidate exists
  anywhere.
- Prefer **official release artifacts** over `archive/refs/tags` tarballs:
  release assets ship stable checksums and any dev-version metadata the
  build machinery needs; tag archives are re-compressed at GitHub's whim
  and lack git metadata.

### 10.4 The reporting ladder (three different claims — never conflate)

1. **Resolves** — `check-recipe` exit 0, 0 missing: the *plan* is
   complete. Say "resolves", never "works".
2. **Builds** — `eb --robot` green: module file exists. For PR-bound
   recipes verify on TWO legs when possible: a delta build against a
   prebuilt stack (fast) and a virgin-robot full build (what upstream CI
   does).
3. **Binary-verified** — the module loads and the binary runs
   (`env -i <bin> --version` proves RPATH completeness) and links the
   stack's libraries (`ldd`).

### 10.5 PR discipline (hard rules)

- **One PR per recipe set.** Before opening anything, list existing open
  PRs for the same software (`gh pr list --author … --state open`) — a
  duplicate PR is a community-facing incident.
- **The PR surface belongs to the human.** Branch pushes to your own fork
  are plumbing and always fine; opening/editing/commenting on PRs happens
  only on an explicit, current instruction — an old authorization does not
  carry forward.
- New software targets the **current development generation** (what
  upstream `develop` toolchain definitions say), not the generation you
  happen to have prebuilt. Verify with the tree, not memory.
- PR text can be (re)generated with the SURF-internal model (§11.5) so the
  AI provenance is attestable as internal.

## 11. Reproducing the eOn and QMCPACK PR fixtures

Frozen recipe sets live in-repo. Use them for operator/agent runs so check-recipe
does not depend on a mutable fork checkout.

### 11.1 Paths

| Package | Fixture | Generation | Role |
|---------|---------|------------|------|
| eOn 2.16.0 | `fixtures/eon_packaging/` | foss-2024a | Site / feedstock parity only |
| eOn 2.16.0 | `fixtures/eon_foss_2026_1/` | **foss-2026.1** | **Landable** upstream PR set |
| QMCPACK 4.3.0 | `fixtures/qmcpack_foss_2026_1/` | foss-2026.1 | PR #26437 |

Robot universe (dependency versions for non-companion packages):

```
ROBOT=$HOME/.venvs/easybuild/easybuild/easyconfigs
# or a clone of easybuild-easyconfigs/easybuild/easyconfigs
```

### 11.2 eOn foss-2026.1 — check-recipe (landable)

```
REPO=<path-to-eb-stack>
DRAFTS=$REPO/fixtures/eon_foss_2026_1/easyconfigs
ROBOT=$HOME/.venvs/easybuild/easybuild/easyconfigs

eb-stack check-recipe \
  --recipe $DRAFTS/e/eOn/eOn-2.16.0-foss-2026.1.eb \
  --easyconfigs "$ROBOT" \
  --easyconfigs "$DRAFTS" \
  --require-configopt=-Dwith_metatomic=true \
  --require-configopt=-Dwith_xtb=true \
  --require-configopt=-Dwith_serve=true \
  --require-configopt=-Dwith_rgpot=true
```

Expect: exit 0, `check-recipe OK`, 0 missing deps. Overlay order: robot first,
drafts second (companions win).

**Residuals (do not invent):**

- Recipe pins `xtb` to `gfbf-2024a` and `PyTorch` to `foss-2024a` until 2026.1
  recipes exist upstream.
- Companion greenfield build/runtime (metatensor stack, quill) if robot still
  lacks them — sources/checksums already in fixtures; build validation is EB/robot.

### 11.3 eOn foss-2024a — site parity only

```
DRAFTS=$REPO/fixtures/eon_packaging/easyconfigs
eb-stack check-recipe \
  --recipe $DRAFTS/e/eOn/eOn-2.16.0-foss-2024a.eb \
  --easyconfigs "$ROBOT" \
  --easyconfigs "$DRAFTS" \
  --require-configopt=-Dwith_metatomic=true \
  --require-configopt=-Dwith_xtb=true \
  --require-configopt=-Dwith_serve=true \
  --require-configopt=-Dwith_rgpot=true
```

Do **not** use this tree as the upstream-develop PR target for new software;
prefer foss-2026.1 (§11.2).

### 11.4 QMCPACK foss-2026.1 — check-recipe

```
DRAFTS=$REPO/fixtures/qmcpack_foss_2026_1/easyconfigs
eb-stack check-recipe \
  --recipe $DRAFTS/q/QMCPACK/QMCPACK-4.3.0-foss-2026.1.eb \
  --easyconfigs "$ROBOT" \
  --require-configopt=-DQMC_MPI=ON \
  --require-configopt=-DQMC_OMP=ON \
  --require-configopt=-DQMC_MIXED_PRECISION=OFF
```

No companion overlay required: HDF5/Boost/libxml2/Python come from the robot.
Expect: exit 0, 0 missing.

**Residuals:** performance ctests need external `QMC_DATA` (recipe excludes them
via `testopts -E performance`). Full `eb` install/build is outside this skill's
mechanical bar — use rg.terra / Jenkins when required.

### 11.5 Agent driver (SURF Willma / OMP)

For SURF-only AI work, drive this skill with the SURF model path, not commercial
frontier models. With OMP: role `eb-stack` → `surf-ai-hub/openai/gpt-oss-120b`.
Run the exact commands above; do not reimplement EasyBuild semantics in the prompt.

Automated regression: `cargo test --test eon_foss_2026_1 --test qmcpack_foss_2026_1 --test eon_packaging`.
