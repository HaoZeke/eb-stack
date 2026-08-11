---
name: eb-stack-verify-recipe
description: Prove an emitted or edited easyconfig is right before spending a build on it, by checking every claim against the real artifact: source checksum recomputed from the download, every patch dry-run against the unpacked tree, extension versions read out of the source, configure options confirmed to exist, and dependency bounds taken from the project's own build system. Use after package bump or plan, after hand-editing a recipe, before pushing to a site pipeline or an upstream PR, and whenever a recipe claims something the tool inferred rather than read.
---

# Verify a recipe without building it

A generated recipe is a hypothesis. The build is the expensive way to test it,
and on a software partition a wrong hypothesis costs hours before it says so.
Every check here runs on a laptop in under a minute and fails loudly.

Run these after `package bump`, after `package plan`, and after any hand edit.
`recipe lint` and `recipe check` do not cover any of them: those answer "is this
well-formed and does it resolve", not "is what it says true".

## The five checks

### 1. The source checksum against the real artifact

Recompute it. Do not carry a checksum because a sibling recipe, another
ecosystem, or an earlier version of the same recipe had one.

```sh
curl -sSL -o /tmp/pkg.tar.gz https://example.invalid/pkg-1.2.3.tar.gz
sha256sum /tmp/pkg.tar.gz          # must equal the checksums entry
```

A seeded checksum from conda-forge or Spack is a different artifact class than
an EasyBuild release-asset source often enough that `recipe check
--verify-sources` exists for it. That flag classifies the URL; this step proves
the bytes.

**Also check every nested copy.** An `exts_list` entry whose `source_tmpl`
resolves to the main tarball carries its own `checksums`, and a version bump
that rewrites only the top-level list leaves the extension checking the old
hash. It fails at the end of the build, after the main package is built:

```sh
rg -n 'tar\.gz|tar\.xz|\.zip' recipe.eb | rg -v '^\s*#'   # every artifact named
```

Every artifact name in the file should carry the version the recipe declares.

### 2. Every patch, dry-run against the unpacked source

A patch written for one version applies to another by luck. Test it:

```sh
tar -xf /tmp/pkg.tar.gz -C /tmp/src
cd /tmp/src/pkg-1.2.3
for p in $EC_DIR/*.patch; do
  echo "--- $p"
  patch -p1 --dry-run --force < "$p" | tail -3
done
```

Read the output rather than the exit status. `succeeded ... with fuzz 1` and an
offset are fine. `No file to patch` means the patch level or the start
directory is wrong, not that the patch is obsolete: an extension patch usually
applies from the tarball root even though the extension's `start_dir` is deeper.

A patch that no longer applies is evidence the fix went upstream. Say which,
and check the release notes before dropping it.

### 3. Extension and component versions, read from the source

An extension version in a recipe is a claim about a file in the tarball. Read
the file:

```sh
rg -n '_major|_minor|_micro|_suffix|^version' \
   /tmp/src/pkg-1.2.3/python_packaging/*/src/*/version.py
```

If a recipe patches a version string (a `pyproject.toml` that disagrees with
the package's own `version.py`), confirm both sides still disagree the same
way, or the patch is now writing the wrong value.

### 4. Configure options, confirmed to exist

Before writing a `configopts` flag, confirm the build system defines it in the
version being built. Options move, get renamed, and live only in forks:

```sh
rg -rn 'option\(|gmx_option_multichoice\(|GMX_USE_' /tmp/src/pkg-1.2.3/CMakeLists.txt \
   /tmp/src/pkg-1.2.3/cmake/*.cmake | rg -i 'the_option_you_want'
```

Two failure modes this catches. A flag that does not exist is silently ignored
by CMake, so the build succeeds and the feature is absent. And a feature that
exists only in a fork cannot be enabled from the upstream tarball at all, which
is a packaging decision, not a flag.

### 5. Dependency bounds, from the project rather than from a sibling recipe

An easyconfig pins dependencies exactly and states no minimum, so a version in
a neighbouring recipe is evidence of what a generation shipped, never of what
the package needs. When a retarget has to move a dependency, read the real
bound:

```sh
rg -n 'find_package\([A-Za-z0-9_]+ [0-9]' CMakeLists.txt
rg -n 'requires-python|dependencies' pyproject.toml
eb-stack package inspect --source path/to/spack/package.py --format spack \
  --toolchain-version 2025b --out-dir /tmp/foreign   # constraints with provenance
```

State the bound and where it came from. "The target generation ships an older
version" is not evidence that the older version is enough.

## Then the cheap mechanical gates

```sh
eb-stack recipe lint  <recipe.eb>
eb-stack recipe check --recipe <recipe.eb> --easyconfigs <robot-tree> [--easyconfigs <another>]
```

Two traps in `recipe check`. Mixing robot trees mixes hierarchies: if two trees
carry the same toolchain at different `GCCcore` generations, the resolver picks
one and reports dependencies from the other as missing. Check against the tree
the build will actually use, and add a second tree only for dependencies that
genuinely come from it. And `--strict` reports statements the parser could not
model, which is useful, but a skipped statement is not by itself a recipe
defect.

## Hand-maintained patch files

A patch file edited by hand needs its hunk arithmetic checked. `patch` accepts
wrong start lines and fixes them by context, but rejects a wrong line *count*
as a malformed patch, and the dry-run in check 2 is what catches it. If the
patch is generated rather than edited, regenerate it instead of editing.

Strip trailing whitespace from added lines. It lands in the generated file and
some build systems carry it into a variable value.

## What this establishes

`resolves` plus `artifact-checked`: the recipe is internally consistent and
every claim it makes about the source is true. It does not establish `builds`.
Say which rung you are on; do not report a verified recipe as a tested one.

## Related

- `skills/annual-bump/SKILL.md` — the bump that produced the recipe
- `skills/tool-repair/SKILL.md` — what to do when a check fails because the emitter is wrong
- `skills/upstream-pr/SKILL.md` — the evidence a maintainer expects to see
- `skills/easybuild-dos-donts/SKILL.md` — recipe-shape rules
