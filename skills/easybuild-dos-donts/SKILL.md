---
name: eb-stack-easybuild-dos-donts
description: Do/don't list for EasyBuild easyconfigs and contributions. Distilled from rejected PR #26435, accepted rework #26480, QMCPACK #26437, and maintainer review practice. Use before drafting recipes, before opening a PR, and when a bot or agent is about to invent flags to make it build.
---

# EasyBuild dos and don'ts

Mechanical enforcement lives in `eb-stack recipe check` / `recipe lint`.
Hard errors are the two #26435 classes (`EB_MAINT_CROSS_GEN`,
`EB_MAINT_SHELL_MONSTER`, `EB_MAINT_PATCHELF_RPATH`); warnings are the
#26480 review classes (`EB_MAINT_THIN_BUILD`, `EB_MAINT_TESTS_OFF`,
`EB_MAINT_DEP_TOOLCHAIN_PIN`). This skill is the human and agent contract
around those gates plus contribution practice.

## DO

### Recipe content

1. **One toolchain generation per PR and per recipe closure.**
   Companions on `GCCcore-X.Y.Z` that belongs to the target generation are fine.
   High-level deps (`foss` / `gfbf` / `gompi` / ...) must share that generation.
2. **Prefer plain dependency names on foss apps.** Robot walks subtoolchains.
   Write `('quill', '11.1.0')`, not a hard-coded GCCcore 4-tuple on foss (#26480).
3. **Put staged software in its own easyconfig.** Companion recipes beat inline
   `cargo cinstall` in `preconfigopts`.
4. **Use tree idioms:** `github_account` + `GITHUB_SOURCE`, templates, full
   sanity checks, `moduleclass` last.
5. **Comment every deviation** from the default easyblock path with why, and
   cite tree precedent when one exists.
6. **Run unit tests when they exist.** Compile them (`with_tests=true` /
   `BUILD_TESTS=ON`) *and* run them (`runtest`). Prefer a minor upstream
   release over an EB patch that fakes green (#26480: "We typically do prefer
   to run unit tests (if they exist) to validate the sanity of the
   installation").
7. **Keep recipes readable.** Short configopts; prefer Meson/CMake options over
   shell pipelines.
8. **Build fat.** Enable every optional feature whose dependencies exist in
   the generation; author first-time companion easyconfigs for missing deps
   instead of switching features off. Features that are mutually exclusive or
   structurally blocked (BLAS-backed dep in a GCCcore recipe) go to a
   `versionsuffix` variant, with the off-flag justified in a recipe comment
   (#26480: "we typically install packages as 'fat' as possible").
9. **Trim the product, not the package.** A smaller *product scope* (fewer
   recipes per PR) beats cross-generation pins or multi-page staging; within
   one recipe, fat beats thin.

### Contribution workflow (EasyBuild CLI is the interface)

10. `eb --check-contrib` on every easyconfig before PR text.
11. `eb --check-github --github-user=<user>` before long builds / uploads.
12. `eb --from-pr <N> --robot ... --upload-test-report` for evidence.
13. `eb -D` / `eb -x` as pre-flight before hours of robot time.
14. `eb --inject-checksums` on the EasyBuild host (never invent hashes).
15. `eb --new-pr` with title `{moduleclass}[toolchain/version] Name vX.Y.Z`.
16. Reuse an installed foss/GCCcore prefix when a prior SUCCESS gist has one.

### Evidence and claims

17. Separate claims: resolves (eb-stack) != builds != binary-verified != SUCCESS report.
18. Disclose AI in the PR body per EasyBuild policy.
19. Freeze accepted/rejected PR shapes as fixtures and re-run maintainer checks.

## DON'T

### The #26435 class (mechanical hard errors)

1. **Don't mix toolchain generations** in one recipe (e.g. PyTorch on
   foss-2024a inside foss-2026.1). Reviewer: "This is mixing two different
   toolchain generations, it shouldn't be done."
2. **Don't stage companion builds in preconfigopts/postinstallcmds** with
   multi-page `+=` shell, cargo cinstall into builddir stage, or
   `patchelf --force-rpath`. Reviewer: "Sorry, but we can never accept this.
   It's incomprehensible and uncommented."
3. **Don't invent RPATH with patchelf** to pass sanity; use
   `check_readelf_rpath = False` when cargo-c installs lack RPATH (#26480).

### Build-eval anti-patterns (agent/bot failure modes)

4. **Don't hardcode random flags to silence a failure** without a measured,
   commented reason and tree precedent.
5. **Don't reimplement EasyBuild** (hand SUCCESS comments, invented style
   checkers) as a substitute for `eb --upload-test-report` / `eb --check-contrib`.
6. **Don't claim a 2026.1 recipe set from 2024a build evidence.**
7. **Don't open or freestyle-edit PR title/body via API.** Human owns the
   surface; only `eb --upload-test-report` posts the standard report.
8. **Don't re-add packages that develop already has** at the target generation.
9. **Don't put architecture essays or internal host paths** in the public PR.
10. **Don't ship easyconfig patches for bugs in software you control.**
11. **Don't bootstrap foss from scratch on bleeding-edge glibc** when Rocky 9
    or an existing prefix will do.
12. **Don't treat `eb --show-config` defaults** as the prefix used by past
    SUCCESS gists.

## Quick pre-PR checklist

```sh
eb-stack recipe lint path/to/*.eb
eb-stack recipe check --recipe path/to/App.eb --easyconfigs "$ROBOT" --easyconfigs "$OVERLAY"
eb --check-contrib path/to/*.eb
eb --check-github --github-user="$GH_USER"
eb App.eb --robot --dry-run-short
# after human opens PR:
eb --from-pr <N> --robot --upload-test-report --github-user="$GH_USER"
```

## Related

- `skills/upstream-pr/SKILL.md` — full contribution sequence
- `fixtures/maintainer_reject_26435/` — frozen reject surfaces (hard errors)
- `fixtures/maintainer_fat_26480/` — frozen fat-build review surfaces (warnings)
- `src/eb_maintainer.rs` — codes `EB_MAINT_*`
