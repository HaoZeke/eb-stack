# lint_dep_duplicate: EB_DEP_DUPLICATE

Severity: warning. Enforced by: reviewer.

Files: real. x11vnc-0.9.17-GCCcore-14.3.0.eb, #24662: bad = d3fcc3396f, good = 7129dc6722. PR: https://github.com/easybuilders/easybuild-easyconfigs/pull/24662

Expected findings in bad.eb (good.eb has none for this rule):

- line 30: EB_DEP_DUPLICATE: duplicate entry ('cairo', '1.18.4') in dependencies (first on line 29)

Precedent:

- jfgrimm: empty suggestion on the second ('cairo', '1.18.4') line, i.e. delete it  
  https://github.com/easybuilders/easybuild-easyconfigs/pull/24662#discussion_r2556853582

Validated against the real CI flake8, pycodestyle, and the framework style check; tests/lint_fixtures.rs checks eb-stack against expected.json.
