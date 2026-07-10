# QMCPACK foss-2026.1 packaging fixtures

**Upstream PR:** easybuilders/easybuild-easyconfigs #26437  
**Fork branch:** HaoZeke/easybuild-easyconfigs `20260710_new_pr_QMCPACK430`

## Files

- `q/QMCPACK/QMCPACK-4.3.0-foss-2026.1.eb` — CPU real reference (MPI+OpenMP,
  full double, no GPU); CMakeNinja; explicit `test_cmd = ctest`; Nexus PYTHONPATH.

## Residuals

- No companion easyconfigs: deps (HDF5 parallel, Boost, libxml2, Python, foss
  toolchain) are expected to resolve from the robot universe alone.
- Performance ctest suites need external QMC_DATA (excluded via `-E performance`).

Provenance: raw fetch from fork branch head used for PR #26437.
