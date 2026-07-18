# eOn foss-2026.1 overlay fixtures

**Intended upstream target:** `foss/2026.1` (GCCcore-15.2.0 hierarchy).

This tree holds the historical companion recipes from the fat-product
surface plus the tutorial overlay copy of the current core eOn recipe. The
canonical core + rgpot draft-PR snapshot lives in
`fixtures/eon_core_rgpot/`. Fixture presence does not establish a
successful build.

| Generation | Fixture path | Role |
|------------|--------------|------|
| foss-2024a | `fixtures/eon_packaging/` | Site/EESSI feedstock-parity baseline (full product) |
| foss-2026.1 | `fixtures/eon_core_rgpot/` | Core + rgpot draft-PR snapshot (canonical) |
| foss-2026.1 | **this directory** | Historical companions + tutorial overlay copy |

## Files

- `e/eOn/eOn-2.17.2-foss-2026.1.eb` — core + rgpot recipe, identical to the
  `eon_core_rgpot` copy (drift-guarded by `tests/eon_core_rgpot.rs`)
- Historical fat-product companions: metatensor 0.2.2, metatensor-torch
  0.10.0, metatomic-torch 0.1.15, quill 11.1.0, CapnProto 1.4.0 on
  GCCcore-15.2.0
- `i/inih/inih-62-GCCcore-15.2.0.eb` (on develop; often missing from older
  robot clones)
- `c/cargo-c/cargo-c-0.10.23-GCCcore-15.2.0.eb` (readcon-core needs
  cargo-c >= 0.10.17)
- `p/PyTorch/PyTorch-2.9.1-foss-2024a.eb` and
  `m/Meson/Meson-1.8.2-GCCcore-13.3.0.eb` — cross-generation parsing
  fixtures for the historical surface; the shipped eOn policy no longer
  consumes them

## Verification status

- `resolves`: exercised by the fixture regression suites.
- `builds`: established only by a successful campaign against these recipe
  bytes.
- `binary-verified`: established only when the campaign's declared
  verification commands pass.

The stack policy for eOn pins only Eigen 5 (safemath core-guard patch
generation); the core dependency closure stays on foss-2026.1 /
GCCcore-15.2.0.
