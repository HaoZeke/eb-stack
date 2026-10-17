# lint_missing_fprefix: EB_FMT_MISSING_F

Severity: error. Enforced by: none (reviewer caught it).

Files: real. Tcl-9.0.1-GCCcore-14.3.0.eb, #24213: bad = 562f558613 (line 44 lacks the f prefix), good = edfbf9b6e3 "Fix missing f-string". PR: https://github.com/easybuilders/easybuild-easyconfigs/pull/24213

Expected findings in bad.eb (good.eb has none for this rule):

- line 44: EB_FMT_MISSING_F: string contains {SHLIB_EXT} but is not an f-string, so it stays literal; add the f prefix

Precedent:

- Micket suggestion (one f-string per piece): f'ln -s %(installdir)s/lib/libtcl%(version_major)s.%(version_minor)s.{SHLIB_EXT} %(installdir)s/lib/libtcl.{SHLIB_EXT}'  
  https://github.com/easybuilders/easybuild-easyconfigs/pull/24213#discussion_r2423224243

Splitting a long f-string over lines for E501 creates implicit concatenation; every piece that contains {...} needs its own f. Without it the module ships a literal `{SHLIB_EXT}` and nothing in CI notices. Overlaps proof case 24213; keep the fixture out of the agent's context when running that case. Incidental EB_FMT_PERCENT on line 49 stays in good.eb because the merged file still has it.

Validated against the real CI flake8, pycodestyle, and the framework style check; tests/lint_fixtures.rs checks eb-stack against expected.json.
