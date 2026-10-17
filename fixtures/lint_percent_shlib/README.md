# lint_percent_shlib: EB_FMT_PERCENT

Severity: warning. Enforced by: reviewer style (not CI).

Files: real bad / derived good. Clair3-1.0.8-foss-2023a.eb as merged in #22738 (8ee07d8d3a), line 51 `'lib/libclair3.%s' % SHLIB_EXT`; good.eb converts that one line to `f'lib/libclair3.{SHLIB_EXT}'`. PR: https://github.com/easybuilders/easybuild-easyconfigs/pull/22738

Expected findings in bad.eb (good.eb has none for this rule):

- line 51: EB_FMT_PERCENT: %-formatting of a string literal; use an f-string instead (e.g. f'lib/libfoo.{SHLIB_EXT}') and keep %(...)s templates as they are

Precedent:

- Crivella: "Would keep consistent usage of f-strings  without re-introducing old % formatting."  
  https://github.com/easybuilders/easybuild-easyconfigs/pull/24391#discussion_r2472841336
- Micket: "actually, might as well modernize the rest of these string interpolations with f-strings as well, to set a new nicer standard going forward."  
  https://github.com/easybuilders/easybuild-easyconfigs/pull/23349#pullrequestreview-3012493873

Counter-precedent, kept on purpose: the bad line is Crivella's own suggestion from 2025-10-16 (https://github.com/easybuilders/easybuild-easyconfigs/pull/22738#discussion_r2435263238), two weeks before he asked for f-strings in #24391, and Thyre suggested `'...libGLU.%s' % SHLIB_EXT` in #24311 (https://github.com/easybuilders/easybuild-easyconfigs/pull/24311#discussion_r2466066965). So this is a warning about a style trend, not a CI rule.

Validated against the real CI flake8, pycodestyle, and the framework style check; tests/lint_fixtures.rs checks eb-stack against expected.json.
