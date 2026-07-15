# eOn foss-2026.1 packaging fixtures (landable PR set)

**Intended upstream target:** `foss/2026.1` (GCCcore-15.2.0 hierarchy).

This tree is the **landable** EasyBuild easyconfigs set for eOn 2.16.0 on
`foss-2026.1`, not the historical wrong-generation PR that targeted `foss-2024a`.

| Generation | Fixture path | Role |
|------------|--------------|------|
| foss-2024a | `fixtures/eon_packaging/` | Site/EESSI feedstock-parity evidence; companions for 13.3.0 |
| foss-2026.1 | **this directory** | Upstream-correct PR recipes for develop |

## Files

- `e/eOn/eOn-2.16.0-foss-2026.1.eb` — full product (metatomic + xTB + serve + rgpot)
- `e/eOn/eOn-2.16.0_safemath-eigen5-core-guard.patch`
- Companions (robot holes on develop): CapnProto 1.4.0, metatensor 0.2.2, metatensor-torch 0.10.0,
  metatomic-torch 0.1.15, quill 11.1.0 on GCCcore-15.2.0

## Overlay companions for `eb-stack recipe check`

- `c/CapnProto/CapnProto-1.4.0-GCCcore-15.2.0.eb` (serve feature; robot hole until develop has 15.2.0)
- `i/inih/inih-62-GCCcore-15.2.0.eb` (on develop; often missing from older robot clones)
- `c/cargo-c/cargo-c-0.10.23-GCCcore-15.2.0.eb` (readcon-core needs cargo-c >= 0.10.17)
- `p/PyTorch/PyTorch-2.9.1-foss-2024a.eb` (cross-gen pin until foss-2026.1 PyTorch exists)
- `m/Meson/Meson-1.8.2-GCCcore-13.3.0.eb` (cross-gen pin satisfying eOn's Meson >= 1.8 floor)

## Residuals (skill / human judgment — not invented by eb-stack)

- Cross-generation pins in the eOn recipe: `xtb` gfbf-2024a, `PyTorch` foss-2024a
  (no 2026.1 recipes on develop yet). Overlay freezes the PyTorch recipe identity;
  a full robot still supplies xtb and the rest of foss-2024a/2026.1.
- Companion greenfield build/runtime for metatensor stack / quill if CI has not
  built them yet.
- Rust comes from the robot hierarchy. Meson 1.8.2 is frozen in the overlay
  because its complete cross-generation identity is a preferred stack pin.

Provenance: HaoZeke/easybuild-easyconfigs `feat/eon-2.16.0-foss-2026.1` /
`~/Git/tmp/eb-easyconfigs-push` freeze.
