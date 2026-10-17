# lint_f541: F541

Severity: error. Enforced by: easyconfigs CI flake8 (--select F).

Files: synthetic. libRmath-4.5.1-GCCcore-12.3.0.eb as merged in #24056 (good.eb); bad.eb adds an `f` prefix to the placeholder-less configopts on line 35.

Expected findings in bad.eb (good.eb has none for this rule):

- line 35: F541: f-string is missing placeholders

Precedent:

- CI enables pyflakes F codes for *.eb since #20173 (merged 2024-03-21, commit 6cc93944cf): "Fix duplicate dict entries and enable fatal error checking for easyconfigs via `flake8`"  
  https://github.com/easybuilders/easybuild-easyconfigs/pull/20173
- Workflow step: `flake8 --select F,W605,W291  --filename '*.eb'` (.github/workflows/linting.yml on develop)  
  https://github.com/easybuilders/easybuild-easyconfigs/blob/develop/.github/workflows/linting.yml

This is the exact mistake gpt-oss made in the #27187 proof run (an f prefix on a string with no {}); #27187 is held out, so this fixture is synthetic. Not part of `eb --check-contrib`.

Validated against the real CI flake8, pycodestyle, and the framework style check; tests/lint_fixtures.rs checks eb-stack against expected.json.
