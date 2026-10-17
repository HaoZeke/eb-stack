# lint_template_removed: EB_FMT_TEMPLATE_REMOVED

Severity: warning. Enforced by: reviewer style (not CI); diff mode, needs the base file.

Files: real bad+base / derived good. spdlog-1.15.3-GCCcore-14.3.0.eb, #24391: base = develop at 418a4d0a71, bad = efaa57933b, good = bad with Crivella's suggested line applied. PR: https://github.com/easybuilders/easybuild-easyconfigs/pull/24391

Expected findings in bad.eb (good.eb has none for this rule):

- line 27: EB_FMT_TEMPLATE_REMOVED: %(name)s template from the base version was replaced by a Python expression; keep the template

Precedent:

- Crivella: "`%(name)s` are EB templates. I do not expect it to ever be different from `name` in the EC but i would keep it as such w.r.t other ECs"  
  https://github.com/easybuilders/easybuild-easyconfigs/pull/24391#discussion_r2473147924
- Crivella: "EDIT: for context we already have some `{name}` in EB but templates are the majority"  
  https://github.com/easybuilders/easybuild-easyconfigs/pull/24391#discussion_r2473247863

The PR was merged with f'include/{name}/{name}.h' still in place, so this is a warning, not an error. Overlaps proof case 24391 (keep it out of the agent's context for that case).

Validated against the real CI flake8, pycodestyle, and the framework style check; tests/lint_fixtures.rs checks eb-stack against expected.json.
