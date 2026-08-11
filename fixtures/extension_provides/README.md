# Extension provides fixture

`SciPy-bundle` lists `numpy` and `scipy` in `exts_list`. `App` depends on
`numpy` by name. A stack solve of `App` must co-select `SciPy-bundle` via the
virtual provide rather than fail as a missing package.
