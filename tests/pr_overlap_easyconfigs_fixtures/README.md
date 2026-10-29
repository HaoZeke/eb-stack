# Synthetic easyconfigs unit fixture

These easyconfigs and pull requests are synthetic. Unit tests under
`tests/pr_overlap_easyconfigs.rs` read this directory. The how-tos do
not run them. The recorded open pull requests are
`tests/pr_overlap_easyconfigs_open/`.

`Bundle` lists `Lib` in `exts_list`. `Compile` build-depends on `Lib`.
`Tool` and `Wrap` depend on `Lib` at runtime. A pull request that patches
`Lib` schedules `Lib` and `Bundle`, and leaves `Compile`, `Tool`, and
`Wrap` off `to_build`.
