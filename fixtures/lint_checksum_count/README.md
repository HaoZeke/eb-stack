# lint_checksum_count: EB_CHECKSUM_COUNT

Severity: error. Enforced by: eb --check-contrib (checksum checks); easyconfigs test suite.

Files: real. x11vnc-0.9.17-GCCcore-14.3.0.eb, #24662: bad = d3fcc3396f^, good = d3fcc3396f. PR: https://github.com/easybuilders/easybuild-easyconfigs/pull/24662

Expected findings in bad.eb (good.eb has none for this rule):

- line 15: EB_CHECKSUM_COUNT: Checksums missing for one or more sources/patches in bad.eb: found 1 sources + 1 patches vs 1 checksums

Precedent:

- jfgrimm suggestion: checksums as one dict per file, adding the missing patch checksum  
  https://github.com/easybuilders/easybuild-easyconfigs/pull/24662#discussion_r2556767006
- framework EasyBlock.check_checksums_for message: "Checksums missing for one or more sources/patches in <file>: found N sources + M patches vs K checksums"  
  https://github.com/easybuilders/easybuild-framework/blob/develop/easybuild/framework/easyblock.py

Both files also carry the duplicate cairo dependency (see lint_dep_duplicate); it was removed in a later commit.

Validated against the real CI flake8, pycodestyle, and the framework style check; tests/lint_fixtures.rs checks eb-stack against expected.json.
