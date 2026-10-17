# lint_concat_shlib: EB_FMT_CONCAT

Severity: warning. Enforced by: reviewer style (not CI).

Files: real. libsupermesh-2025.4-foss-2025a.eb, #24492: bad = 2e17e46892, good = f0e1d3b1e8. PR: https://github.com/easybuilders/easybuild-easyconfigs/pull/24492

Expected findings in bad.eb (good.eb has none for this rule):

- line 31: EB_FMT_CONCAT: string concatenation with SHLIB_EXT; put it inside the f-string instead (e.g. f'lib/libfoo.{SHLIB_EXT}')

Precedent:

- Micket suggestion: 'files': [f'include/{name}.h', f'lib/{name}.a', f'lib/{name}.{SHLIB_EXT}'],  
  https://github.com/easybuilders/easybuild-easyconfigs/pull/24492#discussion_r2508082842

The fix was pushed by a maintainer, which is why #24492 is not in the proof set. Note the suggestion itself uses {name}; see the README index on why plain {name}/{version} is tolerated.

Validated against the real CI flake8, pycodestyle, and the framework style check; tests/lint_fixtures.rs checks eb-stack against expected.json.
