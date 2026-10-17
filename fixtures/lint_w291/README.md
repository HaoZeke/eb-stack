# lint_w291: W291

Severity: error. Enforced by: easyconfigs CI flake8 (--select W291).

Files: real. catalyst-2.0.0-foss-2025b.eb, #24311: bad = 59894ff0c9, good = 1053e9d14f. PR: https://github.com/easybuilders/easybuild-easyconfigs/pull/24311

Expected findings in bad.eb (good.eb has none for this rule):

- line 7: W291: trailing whitespace
- line 8: W291: trailing whitespace

Precedent:

- Author commit 1053e9d14f "Remove trailing whitespaces" (2025-10-20, before any review comment on the description), most likely prompted by CI  
  https://github.com/easybuilders/easybuild-easyconfigs/pull/24311/commits/1053e9d14f7e61fe01a0bc3e07c47474f6da4c5c
- CI selects W291 for *.eb (.github/workflows/linting.yml)  
  https://github.com/easybuilders/easybuild-easyconfigs/blob/develop/.github/workflows/linting.yml

The trailing spaces are inside the `description` triple-quoted string. `eb --check-contrib` does NOT flag them (its custom W299 exempts description/examples/citing, and W291 is in its ignore list), but the CI flake8 W291 does. Also incidental: both files have `python -c` without `-s` on line 39 (EB_PY_NO_USER_SITE), which Thyre's suggestion in the same PR fixed later: https://github.com/easybuilders/easybuild-easyconfigs/pull/24311#discussion_r2444645583

Validated against the real CI flake8, pycodestyle, and the framework style check; tests/lint_fixtures.rs checks eb-stack against expected.json.
