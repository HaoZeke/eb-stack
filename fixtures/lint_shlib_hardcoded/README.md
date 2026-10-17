# lint_shlib_hardcoded: EB_SHLIB_HARDCODED

Severity: warning. Enforced by: reviewer style (not CI).

Files: real. libRmath-4.5.1-GCCcore-12.3.0.eb, #24056: bad = a3d80c155c, good = a86357c929. PR: https://github.com/easybuilders/easybuild-easyconfigs/pull/24056

Expected findings in bad.eb (good.eb has none for this rule):

- line 44: EB_SHLIB_HARDCODED: hardcoded '.so'; use SHLIB_EXT in an f-string (e.g. f'lib/libfoo.{SHLIB_EXT}')

Precedent:

- Crivella: suggestion f'lib/%(name)s.{SHLIB_EXT}' "Generalize dynamic library extension"  
  https://github.com/easybuilders/easybuild-easyconfigs/pull/24056#discussion_r2444693034

Flags '.so' at the end of a word in strings. Paths in configopts like $EBROOTFOO/lib64/libfoo.so are flagged too (see the incidental hits in lint_template_unknown), which matches what reviewers ask for.

Validated against the real CI flake8, pycodestyle, and the framework style check; tests/lint_fixtures.rs checks eb-stack against expected.json.
