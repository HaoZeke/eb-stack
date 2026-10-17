# lint_fstring_quote_reuse: EB_FSTRING_QUOTE_REUSE

Severity: error. Enforced by: none in CI (see note).

Files: real. CRYSTAL-23.1.0.1-gompi-2024a-Linux.eb, #23528: bad = a2a100c1e5, good = e4880a9668. PR: https://github.com/easybuilders/easybuild-easyconfigs/pull/23528

Expected findings in bad.eb (good.eb has none for this rule):

- line 18: EB_FSTRING_QUOTE_REUSE: string '_' inside an f-string replacement field reuses the enclosing quote '; Python < 3.12 (EasyBuild supports 3.6+, CI lints with 3.8) cannot parse this, use the other quote character

Precedent:

- lexming's suggestion introduced f'%(namelower)s{'_'.join(_ver)}_...'   
  https://github.com/easybuilders/easybuild-easyconfigs/pull/23528#discussion_r2426803853
- smoors corrected it to f'%(namelower)s{"_".join(_ver)}_...'  
  https://github.com/easybuilders/easybuild-easyconfigs/pull/23528#discussion_r2431197952

Valid only on Python >= 3.12 (PEP 701). Checked: CI's flake8 on Python 3.8 with --select F,W605,W291 does not report it (E999 is not selected), and check_easyconfigs_style on 3.8 returns 0, so only a reviewer or an old-Python test run catches it.

Validated against the real CI flake8, pycodestyle, and the framework style check; tests/lint_fixtures.rs checks eb-stack against expected.json.
