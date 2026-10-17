---
name: eb-stack-easybuild-dos-donts
description: Do/don't list for EasyBuild easyconfigs and contributions. Distilled from rejected PR #26435, accepted rework #26480, QMCPACK #26437, merged review threads on string formatting and templates (#25561, #23349, #23528, #24056, #24886 and others), and maintainer review practice. Use before drafting recipes, before opening a PR, when applying review feedback, and when a bot or agent is about to invent flags to make it build.
---

# EasyBuild dos and don'ts

Mechanical enforcement lives in `eb-stack recipe check` / `recipe lint`.
Hard errors are the two #26435 classes (`EB_MAINT_CROSS_GEN`,
`EB_MAINT_SHELL_MONSTER`, `EB_MAINT_PATCHELF_RPATH`); warnings are the
#26480 review classes (`EB_MAINT_THIN_BUILD`, `EB_MAINT_TESTS_OFF`,
`EB_MAINT_DEP_TOOLCHAIN_PIN`). `recipe lint` also reports what upstream CI,
the easyconfigs test suite and reviewers check (E999, F541, W605, W291, E501,
E2xx, `EB_*`; see `fixtures/LINT_FIXTURES.md`), each finding an error or a
warning, plus a `clean` flag that is false while any error or any `EB_FMT_*`
formatting finding remains. It does
not rely on `eb --check-contrib`, but merges it when `eb` is on `PATH`. This skill is the human and
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

### Strings, templates and f-strings

An easyconfig is Python, run with `exec` when it is parsed. f-strings and
constants such as `SHLIB_EXT` are evaluated at parse time. `%(name)s`-style
templates stay as text and EasyBuild fills them in later, per extension,
per component, per `multi_deps` iteration, and with build-time values such as
`%(builddir)s` and `%(installdir)s`. A template and a Python variable are not
interchangeable, even when they print the same thing today.

S1. **Keep EasyBuild templates.** If a line uses `%(name)s`, `%(version)s`,
    `%(namelower)s`, `%(version_major_minor)s`, `%(pyshortver)s`, ... leave them.
    Never rewrite them to `{name}` / `{version}`. The templates are what keep
    the file consistent with the rest of the repository and what
    `--try-software-version` follows
    (https://docs.easybuild.io/writing-easyconfig-files/#easyconfig_param_templates).
S2. **Use a template when one exists, an f-string only when none does.**
    smoors, #25561: "we should use the easybuild templates when available, and
    only fall back to f-strings if not available."
    (https://github.com/easybuilders/easybuild-easyconfigs/pull/25561#issuecomment-4204999497).
    `f'{name.lower()}'` becomes `'%(namelower)s'`, and `version.split('.')[0]`
    becomes `%(version_major)s`. The full list is `eb --avail-easyconfig-templates`.
    `%(arch)s` is a template; `ARCH` is a constant, so use `{ARCH}` in an
    f-string, not `%(ARCH)s`.
S3. **Use f-strings for constants and local variables, not `%` or `+`.**
    Micket, #23349: "might as well modernize the rest of these string
    interpolations with f-strings as well, to set a new nicer standard going
    forward."
    (https://github.com/easybuilders/easybuild-easyconfigs/pull/23349#pullrequestreview-3012493873).
    Templates can sit inside the f-string:

    ```python
    # bad
    'files': ['lib/libfoo.%s' % SHLIB_EXT, 'lib/%(name)s.' + SHLIB_EXT]
    # good: only the % / + part changes, the templates stay
    'files': [f'lib/libfoo.{SHLIB_EXT}', f'lib/%(name)s.{SHLIB_EXT}']
    ```

    `f'lib/%(name)s.{SHLIB_EXT}'` is already correct: keep it exactly as it
    is. Do not turn its `%(name)s` into `{name}` and do not drop the `f`.

    When you drop the `%` operator, un-escape doubled templates. With `%`,
    `'BEAST.v%%(version)s.Linux.%s.tgz' % local_archs[ARCH]` needs `%%`; as an
    f-string it is `f'BEAST.v%(version)s.Linux.{local_archs[ARCH]}.tgz'`
    (Crivella, #21194,
    https://github.com/easybuilders/easybuild-easyconfigs/pull/21194#discussion_r2438870296).
    This is a reviewer preference, not a CI check, but eb-stack lint counts it
    on the lines in scope: an in-scope `EB_FMT_PERCENT` finding keeps `clean`
    false. Convert the lines the review is about (when it asks for f-strings
    in general, that is every plain `'...' % X` / `'...' + SHLIB_EXT` it
    covers) and the lines you touch anyway. Leave untouched entries that use
    `%%(...)s` escapes as they are (lint reports them as `info`); if you do
    convert one, every `%%(` becomes `%(` and the path must stay the same.
    Each finding's message shows the exact replacement. Never collapse a
    sanity entry into another one while converting it.
S4. **Only add `f` when there is a `{...}` to fill in.** `f"--with-x=no"` is
    pyflakes F541, and easyconfigs CI fails on it (`flake8 --select F,W605,W291`
    on `*.eb`, enabled in #20173).
S5. **Put `f` on every piece of a split string.** Adjacent literals are joined
    by Python, but each piece needs its own `f` prefix. If one is missing, the
    module ships a literal `{SHLIB_EXT}` and no CI check notices (eb-stack lint
    reports it as `EB_FMT_MISSING_F`):

    ```python
    # bad: second piece has no f
    f'cp lib/libfoo%(version_major)s.{SHLIB_EXT} '
    '%(installdir)s/lib/libfoo.{SHLIB_EXT}',
    # good
    f'cp lib/libfoo%(version_major)s.{SHLIB_EXT} '
    f'%(installdir)s/lib/libfoo.{SHLIB_EXT}',
    ```
S6. **Don't reuse the outer quote inside `{...}`.** `f'{'_'.join(v)}'` parses
    only on Python 3.12+. EasyBuild supports older Pythons, and CI lints with
    Python 3.8 without reporting this. Write `f'{"_".join(v)}'` (smoors, #23528,
    https://github.com/easybuilders/easybuild-easyconfigs/pull/23528#discussion_r2431197952).
S7. **Inside `exts_list` and `components`, use `%(name)s` / `%(version)s`.**
    There they resolve to the extension's or component's own values. A Python
    `{name}` / `{version}` is always the parent's. For build-time paths, always
    use `%(builddir)s` / `%(installdir)s`, because no Python variable holds them.
S8. **Plain `{name}` / `{version}` in a new top-level line is tolerated, not
    preferred.** Merged easyconfigs contain lines such as
    `f'{name}-{version}-{local_commit}.tar.xz'`, including fixes by maintainers.
    Don't introduce it when a template fits, and never swap an existing template
    for it (S1). Note that EB already re-parses the text for
    `--try-software-version`, so `{version}` does follow the new version.
S9. **Use `SHLIB_EXT`, never `.so`,** for shared libraries in sanity checks,
    commands and paths: `f'lib/%(name)s.{SHLIB_EXT}'` (Crivella, #24056:
    "Generalize dynamic library extension",
    https://github.com/easybuilders/easybuild-easyconfigs/pull/24056#discussion_r2444693034).
S10. **Run `python -s`** in `sanity_check_commands`, `postinstallcmds` and
     similar for non-Python easyblocks: `"python -s -c 'import itk'"`. The test
     suite fails new files without it. boegel, #24886: "`-s` option should be
     used (to ignore user `site-packages`)"
     (https://github.com/easybuilders/easybuild-easyconfigs/pull/24886#discussion_r2622037471).
S11. **Name local variables `local_*` or `_*`, not both.** Micket, #24587: "we
     specifically allow for local variables that either start with _ or with
     local, so it's a bit cake-on-cake to do both here"
     (https://github.com/easybuilders/easybuild-easyconfigs/pull/24587#discussion_r2535805898).

### Applying review feedback

R1. **Change what the comment is about and nothing else.** A comment on the
    `%` around `SHLIB_EXT` is not a request to rewrite the templates on the same
    line. Lines the lint reports clean that the review does not mention stay
    as they are.
R1a. **A reviewer's code suggestion is a style hint, not a literal patch.** It
    shows the direction (for example "use an f-string here"). Apply it to the
    lines it is about, in the form the rules above require. A suggestion may
    use a form a rule forbids (`{name}` for `%(name)s`, `%` formatting, a
    missing bracket); keep the rule and meet the intent.
R2. **Check a suggestion before you commit it.** #21194's suggestion was missing
    a closing `]`, and the author committed it verbatim. Re-run the checks below
    after every edit.
R3. **Meeting the reviewer's intent another way is fine.** Explain it in the
    reply.
R4. **Changing nothing is a valid result.** When lint is clean and no line
    matches what the reviewer asked for (it was already fixed, or the comment
    is about another file), leave the file unchanged and say so. Do not edit
    correct lines to have something to show.
R5. **Find where each comment applies before editing.** An inline comment
    applies to the line it is anchored on: the last lines of the diff hunk
    shown under it (the `+` lines at the bottom, or the line range given). An
    empty ```` ```suggestion ```` block means "delete these lines". It is not
    about `easyblock` or any other line higher up in the hunk. A PR-level
    comment ("use f-strings throughout", "please use templates") applies to
    the whole file: fix every matching line, then lint with
    `review_scope: "file"`. Pass the anchored line numbers as `review_lines`.
R6. **A question about a removal asks you to keep the item.** "Not sure if we
    want to remove X", "why was X dropped?" or "is X still needed?" on a line
    the PR removed means: put X back, unchanged. Only remove something when
    the reviewer says to.
R7. **Leave working string-building alone.** `configopts += '...'` and
    multi-line strings split across adjacent literals are normal easyconfig
    style. Do not merge, reflow or re-wrap a string the review does not
    mention. Use parentheses, not backslashes, if you must split a line.

### Contribution workflow (EasyBuild CLI is the interface)

10. `eb --check-contrib` on every easyconfig before PR text. It runs pycodestyle
    (max 120, only W291/E741 ignored, so E226 and other default-ignored codes
    count) and the SHA256 checksum checks. It does **not** run pyflakes, so it
    misses F541. Its W299 also exempts the description, while CI's W291 does
    not. eb-stack lint covers both without needing `eb`. Run the CI lint too:
    `flake8 --select F,W605,W291 --filename '*.eb'` (with easyconfigs setup.cfg).
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
   foss-2024a inside foss-2026.1). The #26435 review rejected exactly this.
2. **Don't stage companion builds in preconfigopts/postinstallcmds** with
   multi-page `+=` shell, cargo cinstall into builddir stage, or
   `patchelf --force-rpath`. The #26435 review rejected this as unreadable
   and uncommented.
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
| `f'libexec/foo.{SHLIB_EXT}'` | `'libexec/foo.%s' % SHLIB_EXT` | `SHLIB_EXT` is a Python constant. Old-style `'%s' % X` is EB_FMT_PERCENT. |
| `f'lib/libfoo.{SHLIB_EXT}'` | `'lib/libfoo.' + SHLIB_EXT` | Concatenating a constant is EB_FMT_CONCAT. |
| `f'cp {local_file} target &&'` | `"cp %s target &&" % local_file` | `local_file` is a local Python name. |
| `'cd %(builddir)s/tmp && '` | `f'cd %(builddir)s/tmp && '` | `%(builddir)s` is not a Python placeholder. The `f` prefix is F541. |
| `'%(name)s/%(version)s'` (kept) | `f'{name}/{version}'` replacing it | Swapping an existing template for a Python value is EB_FMT_TEMPLATE_REMOVED (lint with a base file). Plain new `{name}` at top level is tolerated. |
| `'%(namelower)s'` | `f'{name.lower()}'` | A template exists for the derived value: EB_FMT_TEMPLATE_VAR. |
| `f'lib/%(name)s.{SHLIB_EXT}'` | either form alone | Correct as written; keep it. Template text stays `%(name)s`, the constant is the only `{placeholder}`. Lint does not flag it. |

`name.lower()` inside an f-string is `%(namelower)s`. `nameletter.lower()` is
`%(nameletterlower)s`. A numeric `n % 2` is not formatting. A literal `%s`
inside a string with no `%` operator (an `extract_cmd` snippet, for example)
is not formatting either.

`eb --check-contrib` does not run pyflakes, so it will not report F541. eb-stack
lint does. Clean (`"clean": true`) means no error-severity finding and no
`EB_FMT_*` finding (EB_FMT_PERCENT, EB_FMT_CONCAT, EB_FMT_MISSING_F,
EB_FMT_TEMPLATE_VAR, EB_FMT_TEMPLATE_REMOVED): leftover `%` formatting is not
done on the lines in scope. A finding with `"info": true` is outside the
scope (an untouched `%%`-escaped entry, or a line unchanged from the `base`
you passed) and does not count. Pass `review_lines` with the line numbers the
review is about so those always count. `error_free` reports errors only.
These are errors: `%%(...)s` inside an f-string (EB_FSTRING_ESCAPED_TEMPLATE),
a sanity_check_paths entry lost since the file was first seen
(EB_SANITY_ENTRY_REMOVED), `easyblock` removed or changed
(EB_EASYBLOCK_CHANGED), and a dependency with version `''`
(EB_DEP_EMPTY_VERSION: look the version up with `eb_dep_versions`, never
guess). Removing the whole sanity_check_paths is only a warning
(EB_SANITY_PATHS_REMOVED), because a review may ask for exactly that. The
other warnings (EB_SHLIB_HARDCODED, EB_DEP_DUPLICATE) need a change when the
review asks for one. A comment anchored on that line, or a PR-level comment
about the same thing, counts as asking, even when the line is commented out.

## Review-fix workflow

Use this when a reviewer asks for a change on an existing easyconfig. Read the
comments, then change the file. Do not stop at the first diff hunk.

1. Read the whole file (`eb_recipe_read`, or the file on disk). Reviewer
   comments name one line; the same pattern is often elsewhere.
2. Search every occurrence of the requested pattern before editing
   (`eb_recipe_grep`, or grep on disk). `eb_recipe_edit` replaces an exact string. If `old` matches more than once,
   the tool leaves the file unchanged until `replace_all` is true, or until
   `old` includes enough surrounding text to be unique.
3. Apply every requested change, and only those (R1, R1a). If the same
   pattern the reviewer named appears on several lines, fix each of them. If
   nothing matches and lint is clean, the result is no change (R4).
4. Run `eb-stack recipe lint` (MCP `eb_recipe_lint`) and `eb-stack recipe check`
   (MCP `eb_recipe_check`) with at least one easyconfigs path (a robot
   directory; a `.eb` path stands for its easyconfigs directory). Lint must
   report `clean: true`. The MCP server compares each file against the
   version it first saw in the session, so lost sanity entries, a changed
   easyblock and replaced templates are caught without a `base`. A file that is not valid Python is E999 and
   `recipe check` refuses it. When `eb` is installed, lint
   also merges `eb --check-contrib`. A missing `eb` does not clear local
   findings. On the EasyBuild host, still run `eb --check-contrib` before PR
   text. eb-stack lint is not a substitute for that host check.
5. Show the diff. Do not declare the review fix done while lint reports
   `clean: false` for a line you edited or a line in `review_lines`. If an
   error says a sanity entry was lost but the new entry is the same path
   written another way (`{name}` for `%(name)s`), say so in the summary
   instead of reverting the reviewer's change; never pass the current file
   as `base` to silence it.
   Do not comment on the PR. The human owns that surface.

## Quick pre-PR checklist

```sh
eb-stack recipe lint path/to/*.eb
eb-stack recipe check --recipe path/to/App.eb --easyconfigs "$ROBOT" --easyconfigs "$OVERLAY"
eb --check-contrib path/to/*.eb
flake8 --select F,W605,W291 --filename '*.eb' path/to/   # easyconfigs CI lint (F541, W605, W291)
grep -nE "python (-c|-m) " path/to/*.eb | grep -v "python -s"   # CI test: needs python -s
grep -nE "% *SHLIB_EXT|\+ *SHLIB_EXT|\.so['\" ]" path/to/*.eb  # reviewer style: f'...{SHLIB_EXT}'
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
- `fixtures/lint_*/` — one fixture per string/template/CI rule (bad, good,
  expected.json, README with precedent); `fixtures/LINT_FIXTURES.md` has the
  rule table, and `tests/lint_fixtures.rs` keeps eb-stack lint in line with it
- `src/eb_maintainer.rs` — codes `EB_MAINT_*`
