# eb-stack recipe lint fixtures

Each `lint_<rule>/` has `bad.eb`, `good.eb`, `expected.json` (rules, severity, enforced_by, base, findings with
rule/line/message) and a README with the source commits and precedent (review comment, docs, CI config or
framework source, each with its URL). `lint_template_removed/` also has `base.eb` (diff mode: the version the PR
started from).

Pass criterion: running the linter on bad.eb gives exactly the listed (rule, line) pairs for the fixture's
rule(s), and on good.eb gives none for them. Other rules may fire on either file (real merged easyconfigs are not
perfect). `tests/lint_fixtures.rs` enforces this for every fixture, and that `lint_negative/negative.eb`
(synthetic must-not-flag cases: `${PATH}`, `{{`, `.so.1`, `.sock`, `%(build_dir)s`, `%(arch)s`,
`python -E -s -m`, templates inside f-strings, comprehensions in sanity paths) gives no findings at all.

The expected findings were validated against the real checkers: the CI flake8 (Python 3.8,
`--select F,W605,W291 --filename '*.eb'` with the easyconfigs setup.cfg), the framework
`check_easyconfigs_style` (the style part of `eb --check-contrib`), and an executable Python spec of the EB_*
rules that `src/eb_style_rules.rs` ports. Those scripts are not shipped here.

## What the real checkers enforce (verified, not assumed)

| Checker | What it runs |
|---|---|
| easyconfigs CI (`.github/workflows/linting.yml`) | `flake8 --select F,W605,W291 --filename '*.eb'` on Python 3.8 with setup.cfg (max-line-length 120, EB constants as builtins). F codes since #20173 (2024-03-21). E999 is NOT selected, so a syntax error that only Python 3.8 sees is not reported. |
| `eb --check-contrib` | pycodestyle, max line 120, ignore = (W291, E741) only, custom W299 (trailing whitespace except comments and description/examples/citing). Replacing the ignore list turns on codes pycodestyle normally skips (E226, E121...). Plus SHA256 checksum checks. No pyflakes, so no F541. |
| easyconfigs test suite | `python -s` rule for new files, sanity_check_paths shape, redefined parameters, local_var_naming_check='error', checksums, binutils for GCCcore, no `$root`, ... |
| eb-stack `eb_recipe_lint` | All of the codes below plus E999 for files that are not valid Python, without relying on `eb --check-contrib` (which never reports F541). `clean` is false while any error-severity finding remains. |

## Rule spec

| Rule | Severity | Flags | Must not flag | Fixture |
|---|---|---|---|---|
| F541 | error (CI) | f-string with no placeholders | f-strings with `{}` | lint_f541 (synthetic) |
| W605 | error (CI, check-contrib) | invalid escape like `'\$'` in a non-raw string | raw strings, `'\\$'` | lint_w605 |
| W291 | error (CI) | trailing whitespace anywhere, including inside description | - | lint_w291 |
| E231, E226, other pycodestyle | error (check-contrib) | pycodestyle with EB options (max 120, ignore W291,E741) | E741, W291 | lint_pycodestyle (synthetic) |
| EB_FMT_PERCENT | warning | `'...%s...' % x` on a string literal (SHLIB_EXT, locals). Hint to un-escape `%%(x)s` | `%(x)s` templates with no `%` operator | lint_percent_shlib, lint_percent_local |
| EB_FMT_CONCAT | warning | `'...' + SHLIB_EXT` / `ARCH` | `f'lib/%(name)s.{SHLIB_EXT}'` (the correct form) | lint_concat_shlib |
| EB_FMT_MISSING_F | error | non-f string containing `{IDENT}` where IDENT is defined in the recipe or an EB constant (typical: second piece of a split f-string) | `${VAR}`, `{{`, unknown names, f-strings | lint_missing_fprefix |
| EB_FSTRING_QUOTE_REUSE | error | nested string in an f-string `{}` reusing the outer quote (Python >= 3.12 only) | other quote char | lint_fstring_quote_reuse |
| EB_FMT_TEMPLATE_VAR | warning | f-string computing a value a template provides: `{name.lower()}` -> %(namelower)s, `{version.split(...)}` / `{version[...]}` -> %(version_major)s etc.; `{name}`/`{version}`/`{versionsuffix}` inside exts_list/components (parent's value, templates resolve per extension) | plain top-level `{name}`/`{version}` (merged precedent tolerates it), locals, SHLIB_EXT | lint_fstring_template |
| EB_FMT_TEMPLATE_REMOVED | warning, diff mode | template present in the base file replaced by a Python expression (`{name}` for %(name)s, ...) | new lines without a template in base | lint_template_removed |
| EB_SHLIB_HARDCODED | warning | `.so` at the end of a word in a library path (`lib/`, `lib64/`) or in sanity_check_paths | `.so.1`, `.sock`, `{SHLIB_EXT}`, build-tree names such as `'bin/7z.so'` in files_to_copy | lint_shlib_hardcoded |
| EB_PY_NO_USER_SITE | error (test suite) | exact port of test_pr_python_packages regexes; skipped for PythonPackage/PythonBundle/CargoPython* | `python -s -c`, `python -E -s -m` | lint_python_s |
| EB_TEMPLATE_UNKNOWN | error | `%(x)s` not in `eb --avail-easyconfig-templates` + aliases, with a difflib suggestion | `%(arch)s`, aliases like `%(build_dir)s`, extension-filter templates (`%(ext_name)s`, `%(ext_version)s`, `%(src)s`), easyblock-added templates (`%(cudnnarch)s`, `%(python)s`, ...), keys of a Python `'...' % {...}` | lint_template_unknown |
| EB_SANITY_PATHS_SHAPE | error (test suite) | evaluated sanity_check_paths not exactly {files, dirs}, both lists, one non-empty | comprehensions/concatenation that evaluate correctly | lint_sanity_paths_shape (synthetic) |
| EB_REDEFINED_PARAM | error (test suite) | `^(\w+)\s*=` seen twice (reported on the second) | - | lint_redefined_param (synthetic) |
| EB_LOCAL_VAR_NAMING | error (test suite) | top-level name that is not a generic parameter, not an extra option of the easyblock, not `local_*`/`_*`/one letter | easyblock extra options (files_to_copy, test_cmd, ...); skip if the easyblock is unknown | lint_local_var_naming (synthetic) |
| EB_FSTRING_ESCAPED_TEMPLATE | error (eb-stack) | `%%(...)s` in the literal part of an f-string (EasyBuild leaves a literal `%(...)s`) | `%%(...)s` with the `%` operator | unit tests |
| EB_SANITY_ENTRY_REMOVED / EB_EASYBLOCK_CHANGED | error, diff mode | a sanity_check_paths entry whose path is gone from the base (formatting changes compare equal); `easyblock` removed or changed | converted entries with the same path | unit tests |
| EB_DEP_EMPTY_VERSION | error | `('Name', '')`, `'0'`, `'x'` or `'TODO'` in (build)dependencies | a real version such as `'3.0.10'` | unit tests |
| EB_DEP_DUPLICATE | warning | identical tuple twice in (build)dependencies | - | lint_dep_duplicate |
| EB_CHECKSUM_COUNT | error (check-contrib) | sources+data_sources+patches (+ easyblock `patch_*` options such as patch_ctypes_ld_library_path) != checksums (top level), framework message | no checksums at all (not this rule), dict-per-file form with the right count | lint_checksum_count |

## Templates vs f-strings: the line the precedent draws

- Keep EasyBuild templates. Never turn an existing `%(name)s`/`%(version)s`/... into `{name}`/`{version}`
  (Crivella, #24391: "`%(name)s` are EB templates ... i would keep it as such w.r.t other ECs",
  https://github.com/easybuilders/easybuild-easyconfigs/pull/24391#discussion_r2473147924).
- Use a template whenever one exists for a derived value (smoors, #25561: "we should use the easybuild templates
  when available, and only fall back to f-strings if not available.",
  https://github.com/easybuilders/easybuild-easyconfigs/pull/25561#issuecomment-4204999497).
- Use f-strings, not `%`, for SHLIB_EXT, ARCH lookups and local variables (Crivella, #24391: "Would keep consistent
  usage of f-strings  without re-introducing old % formatting.",
  https://github.com/easybuilders/easybuild-easyconfigs/pull/24391#discussion_r2472841336). Templates stay inside
  the f-string: `f'lib/lib%(name)s.{SHLIB_EXT}'`.
- Plain `{name}`/`{version}` at top level is tolerated in merged code (#24391 merged with
  `f'include/{name}/{name}.h'`; smoors' SvABA fix kept `f'{name}-{version}-{local_commit}.tar.xz'`; Micket
  suggested `f'...{version}/'` in #24587), so it is not an error for new lines. It is wrong inside
  exts_list/components and for build-time templates (%(builddir)s, %(installdir)s), which have no Python value.

No fixture uses #27186/#27187 files. No material by or mentioning the excluded reviewer.
