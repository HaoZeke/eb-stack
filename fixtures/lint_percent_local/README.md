# lint_percent_local: EB_FMT_PERCENT

Severity: warning. Enforced by: reviewer style (not CI).

Files: real bad (unmerged PR text) / derived good. Beast2-2.7.7-GCC-12.3.0-beagle-lib-4.0.1-CUDA-12.1.1.eb from #21194 (not merged): bad = 27c931f432; good is b4349dae64 with the missing `]` added on line 23 and line 7 converted to an f-string. PR: https://github.com/easybuilders/easybuild-easyconfigs/pull/21194

Expected findings in bad.eb (good.eb has none for this rule):

- line 7: EB_FMT_PERCENT: %-formatting of a string literal; use an f-string instead (e.g. f'lib/libfoo.{SHLIB_EXT}') and keep %(...)s templates as they are
- line 8: EB_FMT_PERCENT: %-formatting of a string literal; use an f-string instead (e.g. f'lib/libfoo.{SHLIB_EXT}') and keep %(...)s templates as they are
- line 23: EB_FMT_PERCENT: %-formatting of a string literal; use an f-string instead (e.g. f'lib/libfoo.{SHLIB_EXT}') and keep %(...)s templates as they are; un-escape %%(...)s to %(...)s when converting

Precedent:

- Crivella suggestion: versionsuffix = f'-beagle-lib-{local_beagle_version}{local_cuda_suffix}'  
  https://github.com/easybuilders/easybuild-easyconfigs/pull/21194#discussion_r2438868296
- Crivella suggestion: sources = [f'BEAST.v%(version)s.Linux.{local_archs[ARCH]}.tgz'  
  https://github.com/easybuilders/easybuild-easyconfigs/pull/21194#discussion_r2438870296

Line 23 shows the escaped-template trap: `'BEAST.v%%(version)s.Linux.%s.tgz' % ...` must become f'BEAST.v%(version)s...' (un-escape %% when the % operator goes away). The suggestion as posted is missing its closing `]`; the author committed it verbatim, so the real b4349dae64 has a syntax error. PR is unmerged, so this is used for lint only, never as proof.

Validated against the real CI flake8, pycodestyle, and the framework style check; tests/lint_fixtures.rs checks eb-stack against expected.json.
