# eb-stack

Private tooling: parse EasyBuild `*.eb` files, co-select a software stack with
**resolvo** (CDCL SAT, `solver.engine = resolvo_cdcl_sat`), emit a stack lock and
a planned CycloneDX SBOM.

## Example: GROMACS foss-2025a → foss-2025b

```bash
# build on a remote builder; do not thrash a laptop with cargo
cargo test
cargo build --release

./target/release/eb-stack solve \
  --easyconfigs fixtures/gromacs_2025_to_next/easyconfigs \
  --policy fixtures/gromacs_2025_to_next/policies/prefer_newer.json \
  --baseline-easyconfigs fixtures/gromacs_2025_to_next/easyconfigs \
  --lock-out stack.lock.json \
  --sbom-out stack.cdx.json
```

`prefer_newer` co-selects GROMACS 2025.0 with OpenBLAS 0.3.27, OpenMPI 5.0.3,
FFTW 3.3.10. Design notes (if any) live in a separate notes vault, not this repo.
