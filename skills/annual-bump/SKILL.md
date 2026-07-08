---
name: eb-stack-annual-bump
description: Use to move an EasyBuild software stack onto a new toolchain generation (the annual rebuild) with eb-stack. Drives the deterministic bump and solve CLI over a set of recipes, and names the narrow decisions left to the driving agent. Any capable agent can execute it.
---

# Annual toolchain-generation bump with eb-stack

Use this to produce the next generation of a set of EasyBuild easyconfigs (for
example `foss-2023b` -> `foss-2024a`) mechanically, so the driving agent only
handles a small, bounded residual. The deterministic work is `eb-stack`; the
agent orchestrates and resolves the residual.

`eb-stack` is a Rust CLI. Build it once (`cargo build --release`); the commands
below assume `eb-stack` is on PATH.

## What is deterministic (eb-stack does it, exits non-zero on ambiguity)

For one recipe, `bump` rewrites a source easyconfig onto a target toolchain
generation and resolves every dependency version itself from an easyconfig
universe:

```
eb-stack bump \
  --source path/to/App-<ver>-<oldtc>.eb \
  --toolchain-name foss --toolchain-version 2024a \
  --easyconfigs path/to/easyconfigs \
  --out out/App-<ver>-foss-2024a.eb
```

- `toolchain` is rewritten to the target generation.
- Each `dependencies` and `builddependencies` version is resolved from the
  universe across the target generation's sub-toolchain hierarchy
  (`GCCcore`/`GCC`/`gfbf`/`gompi`/... down to `SYSTEM`), not by exact toolchain
  string. No dependency versions are supplied by hand.
- Template fields (`%(version)s` and friends), `local_*` variables, `SYSTEM`,
  multi-element dependency tuples (versionsuffix, per-dependency toolchain), and
  `exts_list` are parsed and preserved.
- Everything not changed is preserved verbatim.

For a version bump (the application version changes), also pass the new source
checksum so the emitted recipe is buildable:

```
eb-stack bump ... --version <newver> --source-checksum <sha256>
```

If the version changes and no `--source-checksum` is given, `bump` renames the
source tarball key to the new version and WARNS that the checksum is stale.

To co-select a jointly consistent stack and emit the build list a pipeline
consumes:

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
declared `root_priority`; the stack diff is a reviewable summary of what changed
against the baseline. (CycloneDX SBOM output is opt-in via `--sbom-out` and is
not part of the core workflow.)

## What the agent resolves (the bounded residual)

`bump` reproduces a real maintainer update byte-for-byte EXCEPT for judgment the
tool cannot make. When you drive the annual bump, handle these:

1. Genuinely-new dependencies a maintainer would add for the new version (a dep
   that is not in the source recipe at all). The tool cannot invent these; add
   them from the upstream project's requirements.
2. The source checksum on a version bump when you did not pass one: obtain it
   from the upstream release or from a sibling recipe of the same new version,
   or run EasyBuild's own checksum injection, then re-run with `--source-checksum`.
3. Patch-set changes on a version bump: the old version's patches may not apply
   to the new version. Review and swap to the new version's patch set.
4. A dependency whose target-generation version does not exist in the universe
   yet: build or pull that dependency first, or record it as a blocker.

## Bulletproofing contract

- `bump` exits non-zero (with a typed error) when it cannot resolve a field
  mechanically, so the agent is invoked only for a real residual decision, never
  to guess. Treat any non-zero exit or emitted warning as a decision point.
- Verify each emitted recipe: re-parse it (EasyBuild's own parser is the ground
  truth), and diff against the previous-generation recipe to confirm only the
  intended fields changed.
- Do not push generated recipes to a build pipeline without human review of the
  diff.

## Driver

This skill names no model. Any capable agent (or a human) can execute it; the
tool guarantees correctness on the mechanical steps, so the driver only needs to
handle the four residual cases above.

## Loop over a build list

The annual bump is this per recipe, over the list of recipes to rebuild:

1. `bump` the recipe onto the new generation (auto-resolved deps).
2. If it exits non-zero or warns, resolve the residual and re-run.
3. Re-parse and diff the result; keep the diff for review.
4. When the set is bumped, `solve` the whole set for joint consistency and emit
   the build list.
