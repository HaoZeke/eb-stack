# lint_local_var_naming: EB_LOCAL_VAR_NAMING

Severity: error. Enforced by: easyconfigs test suite (local_var_naming_check='error').

Files: synthetic. libRmath good.eb from #24056; bad.eb introduces `r_srcdir`, good.eb names it `local_r_srcdir`.

Expected findings in bad.eb (good.eb has none for this rule):

- line 34: EB_LOCAL_VAR_NAMING: 'r_srcdir' is not a generic easyconfig parameter; if it is a local variable rename it to 'local_r_srcdir' (unless it is an extra option of the easyblock in use)

Precedent:

- Micket: "nitpick but we specifically allow for local variables that either start with _ or with local, so it's a bit cake-on-cake to do both here"  
  https://github.com/easybuilders/easybuild-easyconfigs/pull/24587#discussion_r2535805898

A name is fine if it is a generic easyconfig parameter, an extra option of the easyblock in use, starts with local_ or _, or is one letter. Do not use both prefixes (_local_). The check must know the easyblock's extra options (files_to_copy for MakeCp, test_cmd for CMakeMake, ...) or it produces false positives; skip it when they are unknown.

Validated against the real CI flake8, pycodestyle, and the framework style check; tests/lint_fixtures.rs checks eb-stack against expected.json.
