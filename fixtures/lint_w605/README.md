# lint_w605: W605

Severity: error. Enforced by: easyconfigs CI flake8 (--select W605); eb --check-contrib (pycodestyle W605).

Files: real. IMAGE-1.1-foss-2022b.eb, #20800: bad = 9e39eefa02^, good = 9e39eefa02 (author's fix). PR: https://github.com/easybuilders/easybuild-easyconfigs/pull/20800

Expected findings in bad.eb (good.eb has none for this rule):

- line 29: W605: invalid escape sequence '\$'
- line 30: W605: invalid escape sequence '\$'

Precedent:

- CI selects W605 for *.eb (added in #11149)  
  https://github.com/easybuilders/easybuild-easyconfigs/pull/11149

Use a raw string or escape the backslash. Python 3.12+ also prints a SyntaxWarning for these strings when the easyconfig is parsed.

Validated against the real CI flake8, pycodestyle, and the framework style check; tests/lint_fixtures.rs checks eb-stack against expected.json.
