# Recorded slices for the dependency-closure page

`docs/orgmode/explanation/sbom-blast-radius.org` runs on these directories.
`SHA256SUMS` in each subdirectory lists every recorded file.

## easybuild-easyconfigs develop

`cbc/`, `gromacs/`, `quarto/`, and `rebuild/` are copies of
easybuild-easyconfigs `develop` at
`981d2123dddccca9ce0b15f9eb2643e872332f5e`, the commit
`scripts/fetch-upstream-trees.sh` checks out.

- `cbc/` is the foss-2025b closure of `Cbc-2.10.12` plus the foss-2025a
  counterparts of Cbc, Cgl, Clp, and MUMPS. 55 easyconfigs.
- `gromacs/` is `GROMACS-2024.4-foss-2023b.eb`, the six dependencies a
  foss-2025b bump selects, and the four patch files that recipe names.
- `quarto/` is `quarto-1.8.26.eb`.
- `rebuild/` is the 90 easyconfigs named by
  `easystacks/software.eessi.io/2026.06/eessi-2026.06-eb-5.4.0-2026.1.yml`
  in EESSI software-layer `b2c10c7278f8b741f4053ddb2907dc4d062063d1`, plus
  that easystack file. `changed.txt` names `git-2.52.0-GCCcore-15.2.0.eb`.
  `installed.txt` names `git/2.52.0-GCCcore-15.2.0`.

`LICENSE` in each of those directories is the upstream licence file.
Its sha256 is `8177f97513213526df2cf6184d8ff986c675afb514d4e68a404010521b880643`.

## v5.4.1 easyblock slice

`easyblock/` copies the v5.4.1 tags that `scripts/fetch-upstream-trees.sh`
pins:

- easyblocks `9607addb10e02da5e3f44916a0be6222f9795287`: `cmakemake.py`,
  `configuremake.py`, `g/gromacs.py`, and `LICENSE`
- framework `2ca2297fd47ce1838b61b30d779c3b00b6d0e61c`: `easyblock.py` and
  `LICENSE`
- easyconfigs `5cc85e8a07a468385ac7157892be86462e7d3285`: `zfp`, `make`,
  `GROMACS-2024.4-foss-2024a`, `ADIOS2-2.10.2-foss-2025b`, and `LICENSE`

`installed.txt` names `zfp/1.0.1-GCCcore-15.2.0`.

## Foreign recipes

`foreign/conda/meta.yaml` is `recipe/meta.yaml` from
`conda-forge/zlib-feedstock` at `e34000ac354055f133215d211048bc3289afe5f4`.
`foreign/conda/LICENSE.txt` is the licence file from that checkout.

`foreign/spack/package.py` is
`repos/spack_repo/builtin/packages/zlib/package.py` from
`spack/spack-packages` at `a19611deab3c33e3b2855b8379e8ddee12ef843a`.
`LICENSE-MIT` and `LICENSE-APACHE` are the licence files from that
checkout.
