# lint_python_s: EB_PY_NO_USER_SITE

Severity: error. Enforced by: easyconfigs test suite test_pr_python_packages (new/changed files only).

Files: real. ITK-5.4.5-foss-2025a.eb, #24886: bad = ade8d36f0b, good = 817acf1323. PR: https://github.com/easybuilders/easybuild-easyconfigs/pull/24886

Expected findings in bad.eb (good.eb has none for this rule):

- line 87: EB_PY_NO_USER_SITE: Python invocation in 'sanity_check_commands = ["python -c 'import itk'"]' (line #87) should use the '-s' parameter in bad.eb

Precedent:

- boegel: "`-s` option should be used (to ignore user `site-packages`)"  
  https://github.com/easybuilders/easybuild-easyconfigs/pull/24886#discussion_r2622037471
- Thyre: "This will fail the CI otherwise"  
  https://github.com/easybuilders/easybuild-easyconfigs/pull/24394#discussion_r2472422040

Exact port of the test-suite regexes: `\bpython (-c|-m|cc|[^ ]*\w+.py) ` must also match `\bpython (-\w+ )*-s `; skipped for PythonPackage/PythonBundle/CargoPython* easyblocks; comments stripped first. Message copied from the test. The maintainer pushed the fix, so #24886 is not in the proof set.

Validated against the real CI flake8, pycodestyle, and the framework style check; tests/lint_fixtures.rs checks eb-stack against expected.json.
