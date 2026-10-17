---
name: eb-stack-easybuild-dos-donts
description: Do/don't list for EasyBuild easyconfigs and contributions. Distilled from rejected PR #26435, accepted rework #26480, QMCPACK #26437, and maintainer review practice. Use before drafting recipes, before opening a PR, and when a bot or agent is about to invent flags to make it build.
---

# EasyBuild dos and don'ts

Mechanical enforcement lives in `eb-stack recipe check` / `recipe lint`.
Hard errors are the two #26435 classes (`EB_MAINT_CROSS_GEN`,
`EB_MAINT_SHELL_MONSTER`, `EB_MAINT_PATCHELF_RPATH`); warnings are the
#26480 review classes (`EB_MAINT_THIN_BUILD`, `EB_MAINT_TESTS_OFF`,
`EB_MAINT_DEP_TOOLCHAIN_PIN`). `recipe lint` also reports the style
reviewers apply: E501, W299, F541, UP031, and EB101, and it merges
`eb --check-contrib` when `eb` is on `PATH`. This skill is the human and
agent contract around those gates plus contribution practice.

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

## Templates versus f-strings

EasyBuild completes string templates from the easyconfig (`name`, `version`,
`builddir`, and the rest of `--avail-easyconfig-templates`). Constants such
as `SHLIB_EXT`, `SOURCELOWER_TAR_GZ`, and `GITHUB_SOURCE` are Python names
from `--avail-easyconfig-constants`, not `%(SHLIB_EXT)s` templates.

Cite the current docs, not a paraphrase of an old recipe:

- Dynamic values, including why `%(version)s` has to stay a template for
  `--try-software-version`:
  <https://docs.easybuild.io/writing-easyconfig-files/#easyconfig_param_templates>
- The template table:
  <https://docs.easybuild.io/version-specific/easyconfig-templates/>
- Line length 120 and `eb --check-contrib` (pycodestyle, ignoring W291 and
  E741, plus EasyBuild's W299 trailing-whitespace check):
  <https://docs.easybuild.io/code-style/>
- Contribution checks, including SHA256 checksums:
  <https://docs.easybuild.io/contributing/>

Write values EasyBuild resolves as templates inside an ordinary string:
`%(name)s`, `%(version)s`, `%(namelower)s`, `%(builddir)s`, `%(installdir)s`.
An f-string is only for a Python constant or a local name, and it always has
a `{placeholder}`.

| Write this | Instead of | Why |
|---|---|---|
| `f'libexec/7z.{SHLIB_EXT}'` | `'libexec/7z.%s' % SHLIB_EXT` | `SHLIB_EXT` is a Python constant. Old-style `'%s' % X` is UP031. |
| `f'cp {local_makefile} makefile.linux &&'` | `"cp %s makefile.linux &&" % local_makefile` | `local_makefile` is a local Python name. |
| `'export TMPDIR=%(builddir)s/tmp && mkdir -p $TMPDIR && '` | `f'export TMPDIR=%(builddir)s/tmp && mkdir -p $TMPDIR && '` | `%(builddir)s` is not a Python placeholder. The `f` prefix is F541. |
| `'%(name)s/%(version)s'` | `f'{name}/{version}'` | `{name}` and `{version}` bypass templates (EB101), so `--try-software-version` cannot retarget them. |
| `f'lib/%(name)s.{SHLIB_EXT}'` | either form alone | Template text stays `%(name)s`. The constant is the only `{placeholder}`. |

`name.lower()` inside an f-string is `%(namelower)s`. `nameletter.lower()` is
`%(nameletterlower)s`. A numeric `n % 2` is not formatting. A literal `%s`
inside a string with no `%` operator (an `extract_cmd` snippet, for example)
is not formatting either.

`eb --check-contrib` does not run pyflakes, so it will not report F541. eb-stack
lint does. Clean means no E501, W299, F541, UP031, or EB101.

## Review-fix workflow

Use this when a reviewer asks for a change on an existing easyconfig. Read the
comments, then change the file. Do not stop at the first diff hunk.

1. Read the whole file (`eb_recipe_read`, or the file on disk). Reviewer
   comments name one line; the same pattern is often elsewhere.
2. Search every occurrence of the requested pattern before editing.
   `eb_recipe_edit` replaces an exact string. If `old` matches more than once,
   the tool leaves the file unchanged until `replace_all` is true, or until
   `old` includes enough surrounding text to be unique.
3. Apply every requested change. On easyconfigs PR #27186 the pre-review
   `p7zip-17.05-GCCcore-15.2.0.eb` (the proof summary called the version 17.06;
   the file in the PR is 17.05) had both
   `"cp %s makefile.linux &&" % local_makefile` and
   `'libexec/7z.%s' % SHLIB_EXT`. Both become f-strings. On #27187,
   `libgd-2.3.3` `pretestopts = f'export TMPDIR=%(builddir)s/tmp && ...'`
   drops the `f` prefix, and `f'lib/%(name)s.{SHLIB_EXT}'` stays as it is.
4. Run `eb-stack recipe lint` (MCP `eb_recipe_lint`) and `eb-stack recipe check`
   (MCP `eb_recipe_check`) with at least one easyconfigs path. Lint must be
   clean of E501, W299, F541, UP031, and EB101. When `eb` is installed, lint
   also merges `eb --check-contrib`. A missing `eb` does not clear local
   findings. On the EasyBuild host, still run `eb --check-contrib` before PR
   text. eb-stack lint is not a substitute for that host check.
5. Show the diff. Do not declare the review fix done while findings remain.
   Do not comment on the PR. The human owns that surface.

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
- `skills/verify-recipe/SKILL.md` — proving a recipe's claims against the real source
- `skills/site-consume/SKILL.md` — building an upstream recipe at a site without forking it
- `skills/eessi-extend/SKILL.md` — EESSI-extend testing and software-layer PRs (separate track)
- `fixtures/maintainer_reject_26435/` — frozen reject surfaces (hard errors)
- `fixtures/maintainer_fat_26480/` — frozen fat-build review surfaces (warnings)
- `src/eb_maintainer.rs` — codes `EB_MAINT_*`
