# lint_sanity_paths_shape: EB_SANITY_PATHS_SHAPE

Severity: error. Enforced by: easyconfigs test suite test_sanity_check_paths.

Files: synthetic. libRmath good.eb from #24056; bad.eb adds a 'commands' key to sanity_check_paths.

Expected findings in bad.eb (good.eb has none for this rule):

- line 43: EB_SANITY_PATHS_SHAPE: sanity_check_paths must have exactly the keys 'files' and 'dirs', both lists, and at least one of them non-empty

Precedent:

- test/easyconfigs/easyconfigs.py: sanity_check_paths must have exactly 'files' and 'dirs', both lists, at least one non-empty  
  https://github.com/easybuilders/easybuild-easyconfigs/blob/develop/test/easyconfigs/easyconfigs.py

Evaluate the value with the framework parser: list comprehensions and concatenations (ITK, CRYSTAL, OpenCV fixtures) are valid.

Validated against the real CI flake8, pycodestyle, and the framework style check; tests/lint_fixtures.rs checks eb-stack against expected.json.
