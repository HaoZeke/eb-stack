# Fat-build review fixtures (easybuild-easyconfigs #26480)

Frozen surfaces for the three warning-class maintainer checks distilled from
the #26480 review threads.

| File | Role |
|------|------|
| `rgpot-2.5.3-thin-pr-head.eb` | Real thin PR head (`pure_lib` + `rpc_client_only`) that drew the fat-build review |
| `bad_dep_pin.eb` | In-hierarchy dependency toolchain hard-coded on a foss app |
| `bad_tests_off.eb` | Test suite configured off and never run |
| `good_fat.eb` | Fat single-generation control: tests compiled and run, no pins |

Reviewer quotes (PR #26480):

- "In EasyBuild, we typically install packages as 'fat' as possible, i.e. with
  as many optional features enabled as we can."
- "We typically do prefer to run unit tests (if they exist) to validate the
  sanity of the installation."
- "No need to specify the toolchain here - in fact we only hard-code the
  toolchain for the dependency in very exceptional cases."

Codes: `EB_MAINT_THIN_BUILD`, `EB_MAINT_TESTS_OFF`, `EB_MAINT_DEP_TOOLCHAIN_PIN`
(all warnings; the #26435 classes stay the hard errors).
