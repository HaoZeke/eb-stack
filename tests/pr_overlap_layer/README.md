# Recorded software-layer overlap

`docs/orgmode/explanation/sbom-blast-radius.org` and
`docs/orgmode/howto/pr-overlap.org` run `pr-overlap` on this directory.

The easyconfigs are copies of easybuild-easyconfigs `develop` at
`981d2123dddccca9ce0b15f9eb2643e872332f5e`. They are the 109 modules the
two closures reach:

- 1585, head `5abcffa10e4d63a6494f51340474a2f95502adf2`
- 1728, head `252edc963ef08b209cd73330149f2c345a2536be`

`prs.json` stores the base and head text of
`easystacks/software.eessi.io/2026.06/eessi-2026.06-eb-5.4.0-2026.1.yml`
for those pull requests on EESSI software-layer
`b2c10c7278f8b741f4053ddb2907dc4d062063d1`. 1585 adds
`SciPy-bundle-2026.05-gfbf-2026.1.eb`. 1728 adds
`PLUMED-2.10.0-foss-2026.1.eb`. `installed.txt` is a comment: this
checkout has no mounted prefix. `LICENSE` is the easybuild-easyconfigs
licence file from that develop commit, sha256
`8177f97513213526df2cf6184d8ff986c675afb514d4e68a404010521b880643`.

`SHA256SUMS` lists the recorded files.
