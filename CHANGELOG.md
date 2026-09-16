# Changelog

All notable changes to this project are documented here.

## Unreleased

### Added

- Overlay planning treats existing robot modules as leaves: a `Python`
  easyconfig that names `binutils` no longer makes `package plan --format
  pypi` unsatisfiable. `--easyconfigs` is the solve robot only; it does
  not start package-closure discovery.

- `kind = "eessi"` target runtime: `eessi_container.sh --mode exec` is
  the install backend for planned PyPI/CRAN overlays. Plan stays on the
  host. `scripts/eessi-extend-eb.sh` loads `EESSI-extend` and execs `eb`.
  See `examples/targets/eessi-extend.toml`.

- `exts_list` entries are virtual Resolvo provides of the parent bundle.
  A requirement for `numpy` is satisfied by `SciPy-bundle` (and the same
  for `Python-bundle-PyPI` / `R-bundle-CRAN`) instead of failing as a
  missing easyconfig. Profile locks collapse the provide to the parent so
  emitted recipes depend on the bundle, not on a fake `numpy` module.
  Planning `numpy` / `scipy` / `torch` as a PyPI root against a robot
  that already ships them is an empty delta. Without that provider the
  plan refuses a pip overlay instead of emitting a `PythonBundle`.
  Warehouse `requires_dist: null` (live numpy JSON) parses as no extras.
  A leftover such as `eon-akmc` whose other PyPI deps are not in the
  robot keeps those names in `exts_list` instead of failing SAT.
- `--format cargo` reads `Cargo.toml` or crates.io JSON. PyO3/maturin
  crates emit `PythonPackage` with implicit `Rust`, `maturin`, and
  `binutils`; other crates emit `Crate`. Host Cargo wrappers
  (`sccache`, `mold`) are unset. Existing robot Rust modules are leaves.
  `--cargo-source` / `kind = "cargo"` closes a PyPI leftover that is a
  crate (for example `readcon`) as a companion module; remaining pure
  PyPI holes stay `exts_list` extras. Overlay extras prepend the
  install prefix to `PYTHONPATH` so `pip --no-build-isolation` sees
  prior extensions and the loaded EESSI modules. Mesonpy ingest reads
  an optional PEP 518 `build_system` object. Extra site packages and
  meson wrap natives named in that ingest become SAT requirements;
  Resolvo takes `quill` / `cbindgen` / `Eigen` / `PyYAML` from the
  robot when those modules exist. Do not hand-edit the emitted recipe
  for those names. Cargo leftovers and mesonpy wraps share one
  cargo-on-EESSI isolation prelude (`cargo::eessi_cargo_host_isolation`):
  host rustc wrappers and `RUSTFLAGS` are unset, `LINKER` is EESSI gcc,
  and the compat `ld` comes from `uname -m`, not a plan-time x86_64
  literal.
- `--format pypi` reads Warehouse-shaped JSON or a `requirements.txt` and
  emits a `PythonBundle` whose `exts_list` is the leftover package, with
  already-provided extras mapped to the parent bundle.
- `--format cran` reads a DESCRIPTION file, CRAN JSON, or a package list and
  emits one `RPackage` recipe when the robot carries its imports, or a
  `Bundle` with `exts_defaultclass = 'RPackage'` carrying the leftovers in
  `exts_list` when it does not. Base-R packages are recorded as residuals.
- Requirement parsing is one implementation shared by the solver and the
  emitter, covering `==`, `!=`, `>=`, `>`, `<=`, `<`, `~=` (PEP 440) and `^`
  and `~` (Cargo). A clause the language cannot express becomes an
  `unparsed-constraint` residual instead of an empty version set.
- Cargo dependencies keep the requirement the manifest states, and `path` and
  `git` dependencies are reported as judgment residuals: neither can be built
  from a published crate tarball.
- `package plan --package-index FILE` reads a repository index, either the
  format CRAN publishes as `PACKAGES` or a pinned requirements file as
  `pip freeze` writes it, supplying versions and checksums for dependencies
  that state none of their own. A CRAN source now comes from
  CRAN's contrib location rather than the project home page in `DESCRIPTION`,
  and an R bundle carries `exts_default_options` pointing at the current and
  archived CRAN paths.
- Package identity, the pip-overlay refusal list, PyO3 marker crates and the
  crate-to-module 