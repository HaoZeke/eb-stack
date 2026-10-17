# lint_template_unknown: EB_TEMPLATE_UNKNOWN

Severity: error. Enforced by: none before build (EasyBuild fails at template resolution time).

Files: real. OpenCV-4.12.0-foss-2025b-CUDA-12.9.1-contrib.eb, #24806: bad = 466c876b37, good = feb6672b87 ("Fix typo", pushed by Micket). PR: https://github.com/easybuilders/easybuild-easyconfigs/pull/24806

Expected findings in bad.eb (good.eb has none for this rule):

- line 90: EB_TEMPLATE_UNKNOWN: unknown EasyBuild template %(cuda__cc_cmake)s (did you mean %(cuda_cc_cmake)s?)

Precedent:

- Thyre: suggestion '%(cuda_cc_cmake)s' "Typo"  
  https://github.com/easybuilders/easybuild-easyconfigs/pull/24806#discussion_r2607357745

Known names = `eb --avail-easyconfig-templates` (framework aff7fa7) plus ALTERNATIVE_EASYCONFIG_TEMPLATES aliases; difflib proposes the nearest. Note %(arch)s exists but %(ARCH)s does not (ARCH is a constant: Micket, https://github.com/easybuilders/easybuild-easyconfigs/pull/24775#discussion_r2608633305, unmerged PR).

Validated against the real CI flake8, pycodestyle, and the framework style check; tests/lint_fixtures.rs checks eb-stack against expected.json.
