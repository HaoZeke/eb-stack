# lint_pycodestyle: E231, E226

Severity: error. Enforced by: eb --check-contrib (pycodestyle, ignore list overridden to W291,E741 only).

Files: synthetic. libRmath good.eb from #24056; bad.eb drops the space after ':' in toolchain (E231) and writes `2*1` (E226).

Expected findings in bad.eb (good.eb has none for this rule):

- line 16: E231: missing whitespace after ':'
- line 37: E226: missing whitespace around arithmetic operator

Precedent:

- easybuild-framework easyconfig/style.py check_easyconfigs_style sets options.ignore = ('W291', 'E741'), max_line_length 120  
  https://github.com/easybuilders/easybuild-framework/blob/develop/easybuild/framework/easyconfig/style.py

E226 is in pycodestyle's default ignore list, so plain `pycodestyle`/`flake8` will not report it; check-contrib does because it replaces the ignore list. eb_recipe_lint (E501 only) misses all of these. Neither code is selected by the CI flake8 step.

Validated against the real CI flake8, pycodestyle, and the framework style check; tests/lint_fixtures.rs checks eb-stack against expected.json.
