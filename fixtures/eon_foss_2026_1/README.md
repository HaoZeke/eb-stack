# eOn foss-2026.1 packaging regression fixtures

**Intended upstream target:** `foss/2026.1` (GCCcore-15.2.0 hierarchy).

This tree is the maintained EasyBuild baseline used to test parsing, resolving,
packaging checks, and generated-recipe comparisons for eOn 2.16.0 on
`foss-2026.1`. Fixture presence does not establish a successful build.

| Generation | Fixture path | Role |
|------------|--------------|------|
| foss-2024a | `fixtures/eon_packaging/` | Site/EESSI feedstock-parity baseline for GCCcore 13.3.0 |
| foss-2026.1 | **this directory** | Maintained comparison baseline for the current target generation |

## Files

- `e/eOn/eOn-2.16.0-foss-2026.1.eb` — full product (metatomic + xTB + serve + rgpot)
- `e/eOn/eOn-2.16.0_safemath-eigen5-core-guard.patch`
- Companions (robot holes on develop): CapnProto 1.4.0, metatensor 0.2.2, metatensor-torch 0.10.0,
  metatomic-torch 0.1.15, quill 11.1.0 on GCCcore-15.2.0

## Additional overlay inputs for `eb-stack recipe check`

- `c/CapnProto/CapnProto-1.4.0-GCCcore-15.2.0.eb` (serve feature; robot hole until develop has 15.2.0)
- `i/inih/inih-62-GCCcore-15.2.0.eb` (on develop; often missing from older robot clones)
- `c/cargo-c/cargo-c-0.10.23-GCCcore-15.2.0.eb` (readcon-core needs cargo-c >= 0.10.17)
- `p/PyTorch/PyTorch-2.9.1-foss-2024a.eb` (cross-gen pin until foss-2026.1 PyTorch exists)
- `m/Meson/Meson-1.8.2-GCCcore-13.3.0.eb` (cross-generation compatibility fixture)

## Verification status

- `resolves`: exercised by the fixture regression suites.
- `builds`: established only by a successful campaign against these recipe bytes.
- `binary-verified`: established only when the campaign's declared verification
  commands pass.

The stack policy defines preferred cross-generation PyTorch, xtb, and Eigen
identities. Resolvo records whether each preference was selected or required a
compatible fallback.
