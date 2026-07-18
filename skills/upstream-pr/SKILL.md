---
name: eb-stack-upstream-pr
description: Shape eb-stack output into an easybuild-easyconfigs PR that maintainers accept. Use when drafting, reviewing, or repairing an upstream EasyBuild contribution, when a PR gets maintainer pushback, or before handing recipes to the human-owned PR surface. Distilled from real merged and rejected PRs, including the rejected eOn 26435 and its accepted rework 26480.
---

# Upstream easybuild-easyconfigs PRs

Every rule here traces to a real maintainer decision. The rejected eOn PR
(easybuild-easyconfigs #26435) and its accepted-shape rework (#26480) are the
canonical pair: the same software, first refused outright, then redone into a
five-file set a maintainer can read in one sitting.

## Non-negotiables (each one has ended a PR)

1. **One toolchain generation per PR.** Never mix generations in a
   dependency list. `('PyTorch', '2.9.1', '', ('foss', '2024a'))` inside a
   foss-2026.1 recipe drew "This is mixing two different toolchain
   generations, it shouldn't be done" and closed #26435. If the robot tree
   lacks a dependency on the target generation, port that dependency on its
   own generation-consistent recipe or trim the product until the closure is
   single-generation. Compiler-level libraries sit on the matching
   `GCCcore-X.Y.Z`; a GCCcore level matching the target generation stays single-generation.
2. **A recipe a maintainer cannot read is a recipe they will not merge.**
   Multi-page `preconfigopts` shell pipelines, staged sub-builds,
   `postinstallcmds` that rewrite rpaths and pkg-config prefixes: "Sorry, but
   we can never accept this. It's incomprehensible and uncommented." If the
   build needs a staged component, make that component its own easyconfig
   (readcon-core became a standalone `cargo cinstall` recipe) instead of
   inlining its build.
3. **Comment every deviation from the default easyblock path** with the reason,
   and cite tree precedent when one exists. The accepted readcon-core recipe
   carries "same pattern as librsvg-2.61.0-GCCcore-14.3.0.eb with cargo-c" —
   maintainers verify against precedent, not against your reasoning.
4. **Disclose AI involvement** per https://docs.easybuild.io/policies/ai/:
   name the tool and model, state the extent of use, in the PR body. One
   line suffices: the accepted form in #26437 is an `## AI Disclosure`
   section such as "Build-eval facilitated by gpt-120b and eb-stack."
   Non-disclosure is discovered, not forgiven.
5. **Never re-add what develop already has.** Before adding any companion,
   check the robot tree at the target generation. A duplicate of an
   existing recipe reads as not having looked.
6. **Do not open, edit, or comment on the PR surface autonomously.** Prepare
   files and paste-ready text; the human owns the surface (see the claims
   section of the new-package skill).

## The accepted shape (#26480 and every sampled merged PR)

- **Small file count.** Sampled merged new-software PRs carry 1-4 files;
  #26480 carries five. One software plus its new-to-develop companions is the
  ceiling; unrelated packages go in separate PRs.
- **Title format is mechanical:** `{moduleclass}[toolchain/version] Name
  vX.Y.Z`. Multiple classes or toolchains join with commas:
  `{chem,lib}[foss/2026.1,GCCcore/15.2.0] eOn v2.17.1 and deps`.
- **The body is nearly empty.** `(created using eb --new-pr)`, an optional
  one-line context ("Backported from 2025b..."), the AI disclosure. Test
  reports, not prose, establish that the recipes build. A long architecture essay in the body
  is a tell that the recipe cannot speak for itself.
- **Trim the product before porting the world.** The fat eOn product needed
  torch, xtb, and a serve stack; the accepted recipe builds the core client
  with `-Dwith_rgpot=true` and lets engines load at runtime via dlopen. A
  smaller product that builds beats a complete one that does not; later
  PRs can grow it.
- **Choose the lowest sufficient toolchain.** Pure C/C++/Rust libraries with
  no toolchain-lib dependency go on `GCCcore` (CapnProto, quill,
  readcon-core, rgpot); only recipes needing MPI/BLAS/FFTW sit on full
  `foss`.

## Recipe style the linter will not fully catch

- Use tree idioms: `github_account` + `GITHUB_SOURCE`, `SOURCE_TAR_GZ`,
  `%(version)s`, `%(pyshortver)s`, `SHLIB_EXT`, `sanity_check_paths` with
  both `files` and `dirs`, `sanity_check_commands` that run the binary and
  import the module, `moduleclass` last.
- `checksums` in source order, patches after sources.
- Dependencies alphabetical within builddependencies/dependencies, tuple
  toolchain form only when it differs from the recipe toolchain.
- `binutils` is a builddependency on every GCCcore recipe.
- Cargo recipes: `unset RUSTC_WRAPPER CARGO_BUILD_RUSTC_WRAPPER` when
  wrappers must go; never `export RUSTC_WRAPPER=` (empty value still counts
  as a wrapper executable). Keep `CARGO_HOME` inside the build directory.
- Patch files start with a prose header: what breaks, why, author line.
  Keep the diff minimal and the filename
  `Name-version_what-it-fixes.patch`.

## PR lifecycle (what happens after the human opens it)

1. `eb --new-pr` creates the PR; a bot posts a diff against the closest
   existing easyconfig. That diff is the first review artifact: a clean,
   small diff against a merged sibling recipe is the strongest acceptance
   signal.
2. The contributor uploads a test report: `eb --from-pr <N> --rebuild
   --upload-test-report`. A PR without a SUCCESS test report does not move.
3. Community bot testing is requested with `@boegelbot please test @
   jsc-zen3` (maintainers often trigger it themselves).
4. A maintainer reproduces SUCCESS, replies "Going in, thanks @author!" and
   merges. The PR itself runs no CI; test reports decide acceptance.

## How eb-stack output maps onto this

- `package plan` / `package bump` emit the conventional `.eb`; `recipe
  format`, `recipe lint`, and `recipe check` must pass against the robot
  tree plus the draft overlay before any PR text is drafted.
- The claim ladder constrains the PR wording: `resolves` alone justifies only a
  draft PR; write "built and sanity-checked on <target>" only when a
  campaign established `builds`/`binary-verified` there, and name the
  toolchain generation it ran on. #26435 claimed a 2026.1 recipe set from a
  2024a build evidence base; reviewers noticed immediately.
- Freeze the exact PR file set as a fixture tree (see
  `fixtures/eon_core_rgpot/`) with tests asserting the non-negotiables:
  single-generation dependency closure, no staging parameters, required
  configopts. Regressions against a live upstream PR are then caught before
  the human pushes an update.
