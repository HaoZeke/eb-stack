---
name: eb-stack-upstream-pr
description: Shape eb-stack output into an easybuild-easyconfigs PR that maintainers accept. Use when drafting, reviewing, or repairing an upstream EasyBuild contribution, when a PR gets maintainer pushback, before handing recipes to the human-owned PR surface, or when producing a test report. Distilled from rejected eOn #26435, accepted-shape rework #26480, and QMCPACK #26437 SUCCESS.
---

# Upstream easybuild-easyconfigs PRs

Every rule here traces to a real maintainer decision or a campaign failure
that burned hours. The rejected eOn PR (#26435) and its accepted-shape rework
(#26480) are the canonical packaging pair. QMCPACK (#26437) is the canonical
test-report pair: FAILED reports from inventing workarounds, then SUCCESS
from the official EasyBuild CLI.

## EasyBuild CLI is mandatory (non-negotiable)

For upstream easyconfigs work, the **EasyBuild `eb` command** is the interface
to the project. Do not invent parallel paths.

| Task | Required command | Forbidden substitutes |
|------|------------------|----------------------|
| Style + checksum gate | `eb --check-contrib path/to/*.eb` | hand-waving style, only running eb-stack lint |
| Prove GitHub integration | `eb --check-github --github-user=<user>` | assuming the token works |
| Build recipes from a PR | `eb --from-pr <N> --robot ...` | building only local copies while claiming "from PR" |
| Post a test report | `eb --from-pr <N> --upload-test-report --github-user=<user>` | `gh pr comment`, hand-written SUCCESS, gist API glue, paste of log tails |
| Open a PR (human) | `eb --new-pr` | random GitHub UI titles that break conventions |
| Inject checksums | `eb --inject-checksums` on the EasyBuild host | inventing hashes |

**Test reports:** the only maintainer-shaped evidence is the comment EasyBuild
itself posts (`Test report by @user` + gist). Log proof:

```text
Adding comment to easybuild-easyconfigs issue #<N>: 'Test report by @...
== Test report uploaded to https://gist.github.com/...
```

If that line is missing, the report was not uploaded. Do not claim SUCCESS
publicly by any other channel.

**PR surface ownership (precise):**

- Human owns title, body, review replies, and any `gh`/`gh api` mutation of
  issues or PRs.
- **Exception:** `eb --upload-test-report` may post the standard test-report
  comment and gist. That is the EB contribution workflow, not agent freestyle.
- Still forbidden: editing title/body via API, "fixing" wording, or posting
  prose that is not the EB test-report format.

Canonical contribution docs: https://docs.easybuild.io/contributing/

## Canonical sequence (do this order)

On the **configured EasyBuild host** (for this site: `rg.terra`, not the
laptop; not a random host without the foss generation installed):

```sh
# 0) Identity and token
eb --check-github --github-user=HaoZeke

# 1) Style + SHA256 on every easyconfig in the PR
eb --check-contrib CapnProto-….eb quill-….eb … eOn-….eb

# 2) Prefer an existing install prefix for the target generation
export EASYBUILD_PREFIX=$HOME/scratch/eb-foss-2026.1   # example: the prefix prior SUCCESS gists used
export MODULEPATH=$EASYBUILD_PREFIX/modules/all
# source site Lmod; set EASYBUILD_SOURCEPATH/BUILDPATH/TMPDIR on durable storage

# 3) Build from the PR head and upload the report
eb --from-pr <N> --robot --ignore-osdeps --force \
   --upload-test-report --github-user=HaoZeke
```

Notes on flags used in this site's campaigns:

- `--ignore-osdeps`: Arch (and similar) OS package names never match deb/rpm
  strings EasyBuild checks; confirm real headers/libs first.
- `--force`: rebuild PR software so the report is not "already installed"
  only; use when a prior FAILED run left modules behind.
- `--robot`: resolve the full dep graph from the robot tree + overlay.
- Do **not** start a from-scratch foss generation unless the prefix is empty
  and Rocky 9 is intentional (hours, not minutes).

eb-stack still owns planning (`package plan`, `recipe check`, fixtures). The
**upstream** gate after recipes exist is always `eb`, not a reimplementation.

## Non-negotiables (each one has ended a PR)

1. **One toolchain generation per PR.** Never mix generations in a
   dependency list. `('PyTorch', '2.9.1', '', ('foss', '2024a'))` inside a
   foss-2026.1 recipe drew "This is mixing two different toolchain
   generations, it shouldn't be done" and closed #26435. If the robot tree
   lacks a dependency on the target generation, port that dependency on its
   own generation-consistent recipe or trim the product until the closure is
   single-generation. Compiler-level libraries sit on the `GCCcore-X.Y.Z`
   matching the target generation; that placement stays single-generation.
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
6. **Do not open, edit title/body, or freestyle-comment on the PR** via `gh`
   or the API. Prepare paste-ready text. The only automated PR comment path
   is `eb --upload-test-report` (see above).

## The accepted shape (#26480 and every sampled merged PR)

- **Small file count.** Sampled merged new-software PRs carry 1-4 files;
  #26480 carries five. One software plus its new-to-develop companions is the
  ceiling; unrelated packages go in separate PRs.
- **Title format is mechanical:** `{moduleclass}[toolchain/version] Name
  vX.Y.Z`. Multiple classes or toolchains join with commas:
  `{chem,lib}[foss/2026.1,GCCcore/15.2.0] eOn v2.17.1 and deps`.
- **The body is nearly empty and human.** Default template from
  `eb --new-pr`: the created-using line, optional one short factual line
  ("Backported from 2025b"), mandatory AI disclosure. That is the whole
  body. **Test reports, not prose, prove the recipes build.**

  **Banned in PR title, body, and review replies (agent-drafted or posted):**
  - Architecture essays, feature marketing, "this PR adds a robust…" filler
  - Bullet novels of design rationale, claim ladders, or internal host paths
  - LLM tells: "I'd be happy to", "happy to help", "in summary", "in
    conclusion", "comprehensive", "leverages", "streamlines", "ensures",
    "seamless", emoji, fake enthusiasm, apologetic padding
  - Process narration ("agent fixed", "campaign established", vault IDs,
    internal hostnames, "as an AI")
  - Invented reviewer-facing status that is not a real SUCCESS gist link

  If a maintainer needs background, put it in chat or a vault note — not in
  the public PR. When asked to draft wording, match HaoZeke-short past PRs:
  dry, sparse, no sales pitch. Prefer leaving the body alone over expanding
  it.
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

### pretestopts / testopts are shell (campaign #26437)

EasyBuild concatenates `pretestopts` + `test_cmd` + `testopts` and runs them
**through a shell**. Regex metacharacters in `-E` patterns must be quoted or
the shell eats them.

```python
# BROKEN — shell pipe; test step fails before PRRTE/slots matter
testopts = (
    "-L deterministic -j %(parallel)s --output-on-failure "
    "-E unit_test_message-r12|unit_test_new_drivers_mpi-r16"
)

# FIXED — quotes survive into the shell command
testopts = (
    "-L deterministic -j %(parallel)s --output-on-failure "
    "-E 'unit_test_message-r12|unit_test_new_drivers_mpi-r16'"
)
```

Verify with the `full command` line in the EasyBuild log before diagnosing
ctest failures. Prefer short lines that still keep the quotes over E501
"fixes" that strip them.

OpenMPI 5 / PRRTE under parallel ctest: multi-rank jobs hit "not enough
slots" when `ctest -j N` launches concurrent `mpiexec`. Tree precedent
(netCDF, OTF-CPT):

```python
pretestopts = 'export PRTE_MCA_rmaps_default_mapping_policy=:oversubscribe && '
```

Do not gut the deterministic suite to hide slot exhaustion. Oversubscribe and
correct quoting are both required when both apply.

## PR lifecycle (after the human opens it)

1. `eb --new-pr` creates the PR; a bot posts a diff against the closest
   existing easyconfig. That diff is the first review artifact: a clean,
   small diff against a merged sibling recipe is the strongest acceptance
   signal.
2. Contributor evidence: `eb --check-contrib` then
   `eb --from-pr <N> --robot --upload-test-report --github-user=<user>`
   (plus `--ignore-osdeps` / `--force` as site needs). A PR without a
   SUCCESS test report does not move.
3. Community bot testing: `@boegelbot please test @ jsc-zen3` (maintainers
   often trigger it themselves).
4. A maintainer reproduces SUCCESS, replies "Going in, thanks @author!" and
   merges. The PR itself runs no CI; test reports decide acceptance.

## Producing the test report (where the hours actually go)

### Decision zero: stack reuse vs empty prefix

| Path | When | Wall time |
|------|------|-----------|
| **Reuse site prefix** (eOn-style) | A prior SUCCESS gist or known `EASYBUILD_PREFIX` already has the foss/GCCcore generation | minutes for the PR packages only |
| **Rocky 9 empty prefix** | Host cannot bootstrap (e.g. glibc 2.43 + binutils 2.45 gprofng), or no generation installed | hours (M4 → GCCcore → full foss → PR packages) |

Find the prefix from a prior author test-report gist (`buildpath` /
`installpath`). `eb --show-config` prints the *default* configuration, not
what past runs used. An empty default installpath says nothing about what
exists elsewhere on the host.

Campaign proof:

- #26480 eOn: reuse `~/scratch/eb-foss-2026.1` → SUCCESS 5/5 in ~9 min via
  `eb --from-pr 26480 --upload-test-report`.
- #26437 QMCPACK: empty Rocky bootstrap wasted a day; same prefix + fixed
  recipes → SUCCESS 2/2 in ~12 min via `eb --from-pr 26437 --upload-test-report`.

### Host and container traps

- **Bleeding-edge hosts break the bootstrap, not the app build.** glibc
  >= 2.43 breaks gprofng in binutils <= 2.45 (`CALL_UTIL` + `_Generic`
  macros). Upstream HEAD already parenthesizes `CALL_UTIL`; do not open a
  novel binutils PR. Use an installed generation or Rocky 9
  (`skills/new-package/container/rocky9`).
- **Container runs need** `--cap-add=NET_RAW` (Perl Net-Ping) plus
  `OMPI_/PRTE_ALLOW_RUN_AS_ROOT*` and
  `EASYBUILD_ALLOW_USE_AS_ROOT_AND_ACCEPT_CONSEQUENCES` from
  `examples/targets/local-podman.toml`. Keyring for upload must be installed
  **inside the same container** that runs `eb` (image venv does not keep
  one-shot `pip install`s).
- **OS dependency checks know deb/rpm names only.** On Arch, verify real
  files, then `--ignore-osdeps`.
- **Uploads use python keyring only** (no env token fallback).
  `eb --check-github --github-user=<user>` before the long build.
- **The PR head moves.** Re-fetch before fixture freezes and before
  `--from-pr`. A report for superseded recipe versions is wasted work.

### After a FAILED public report

FAILED gists stay on the PR. Fix recipes, re-run the **same**
`eb --from-pr <N> --upload-test-report` path, and supersede with SUCCESS.
Do not delete comments or invent a manual "actually it works" post.

## How eb-stack output maps onto this

- `package plan` / `package bump` emit the conventional `.eb`; `recipe
  format`, `recipe lint`, and `recipe check` must pass against the robot
  tree plus the draft overlay before any PR text is drafted.
- Then run **`eb --check-contrib`** on the EasyBuild host. eb-stack lint is
  not a substitute for contribution checks maintainers run.
- The claim ladder constrains the PR wording: `resolves` alone justifies only a
  draft PR; write "built and sanity-checked on <target>" only when a
  campaign established `builds`/`binary-verified` there, and name the
  toolchain generation it ran on. #26435 claimed a 2026.1 recipe set from a
  2024a build evidence base; reviewers noticed immediately.
- Freeze the exact PR file set as a fixture tree (see
  `fixtures/eon_core_rgpot/`, `fixtures/qmcpack_foss_2026_1/`) with tests
  asserting the non-negotiables: single-generation dependency closure, no
  staging parameters, required configopts, shell-safe testopts, and
  contribution-critical flags. Regressions against a live upstream PR are
  then caught before the human pushes an update.

## Internal campaign notes (vault)

- eOn #26480: `Software/eOn/easybuild-pr-26480-core-rgpot-2.17.2-campaign.org`
- QMCPACK #26437: `Software/eb-stack/easybuild-pr-26437-qmcpack-test-report-campaign.org`
- Host ladder: `Software/eb-stack/easybuild-test-report-host-lessons-2026-07-18.org`
