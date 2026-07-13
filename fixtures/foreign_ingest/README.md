# Foreign recipe ingest fixtures

Inputs for `eb-stack ingest` (conda-forge + Spack → EasyBuild scaffold).

| Path | Format | Notes |
|------|--------|--------|
| `conda_zlib/meta.yaml` | classic conda-build `meta.yaml` | plain YAML, single source |
| `conda_eon/recipe.yaml` | rattler-build v1 (`context` + multi-source) | frozen from conda-forge eon-feedstock |
| `spack_zlib/package.py` | minimal Spack DSL | single base class |
| `spack_eon/package.py` | real Spack `Eon(MesonPackage)` | frozen from spack-packages |
| `spack_qmcpack/package.py` | real Spack `Qmcpack(CMakePackage, CudaPackage)` | multi-base + tag versions |

These drive parser regression; they do **not** claim parity with hand-authored
EasyBuild PR recipes (product flags, EB generation pins, multi-source extract
layout remain residual).
