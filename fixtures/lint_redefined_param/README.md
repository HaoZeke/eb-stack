# lint_redefined_param: EB_REDEFINED_PARAM

Severity: error. Enforced by: easyconfigs test suite (redefined parameters check).

Files: synthetic. libRmath good.eb from #24056; bad.eb defines start_dir twice.

Expected findings in bad.eb (good.eb has none for this rule):

- line 42: EB_REDEFINED_PARAM: easyconfig parameter 'start_dir' is defined again (first on line 41); define each parameter once

Precedent:

- test/easyconfigs/easyconfigs.py: regex ^(?P<key>\w+)\s*= over the file, failing on duplicates  
  https://github.com/easybuilders/easybuild-easyconfigs/blob/develop/test/easyconfigs/easyconfigs.py

Reported on the second definition.

Validated against the real CI flake8, pycodestyle, and the framework style check; tests/lint_fixtures.rs checks eb-stack against expected.json.
