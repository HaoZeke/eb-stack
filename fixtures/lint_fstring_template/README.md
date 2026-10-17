# lint_fstring_template: EB_FMT_TEMPLATE_VAR

Severity: warning. Enforced by: reviewer style (not CI).

Files: real. SvABA-1.2.0-20240815-GCC-12.3.0.eb (#25561): bad = c7d995c68c, good = 75c425a0bd (smoors' fix). PR: https://github.com/easybuilders/easybuild-easyconfigs/pull/25561

Expected findings in bad.eb (good.eb has none for this rule):

- line 18: EB_FMT_TEMPLATE_VAR: f-string computes `name.lower()`; use the EasyBuild template %(namelower)s instead (templates first, f-strings only for values without a template)
- line 42: EB_FMT_TEMPLATE_VAR: f-string computes `name.lower()`; use the EasyBuild template %(namelower)s instead (templates first, f-strings only for values without a template)
- line 44: EB_FMT_TEMPLATE_VAR: f-string computes `name.lower()`; use the EasyBuild template %(namelower)s instead (templates first, f-strings only for values without a template)
- line 45: EB_FMT_TEMPLATE_VAR: f-string computes `name.lower()`; use the EasyBuild template %(namelower)s instead (templates first, f-strings only for values without a template)
- line 46: EB_FMT_TEMPLATE_VAR: f-string computes `name.lower()`; use the EasyBuild template %(namelower)s instead (templates first, f-strings only for values without a template)
- line 51: EB_FMT_TEMPLATE_VAR: f-string computes `name.lower()`; use the EasyBuild template %(namelower)s instead (templates first, f-strings only for values without a template)
- line 54: EB_FMT_TEMPLATE_VAR: f-string computes `name.lower()`; use the EasyBuild template %(namelower)s instead (templates first, f-strings only for values without a template)

Precedent:

- smoors: "we should use the easybuild templates when available, and only fall back to f-strings if not available."  
  https://github.com/easybuilders/easybuild-easyconfigs/pull/25561#issuecomment-4204999497
- PhoenixEmik: "Maybe just use `%(namelower)s` instead of `f'{name.lower()}'`?"  
  https://github.com/easybuilders/easybuild-easyconfigs/pull/25561#issuecomment-4079087246

Only derived values are flagged (name.lower(), version splits/slices, and {name}/{version} inside exts_list/components). smoors' own fix kept f'{name}-{version}-{local_commit}.tar.xz', so plain {name}/{version} at top level is not flagged here; EB_FMT_TEMPLATE_REMOVED covers replacing existing templates.

Validated against the real CI flake8, pycodestyle, and the framework style check; tests/lint_fixtures.rs checks eb-stack against expected.json.
