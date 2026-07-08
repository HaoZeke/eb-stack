# eOn foss-2024a migration overlay

Robot overlay for use with **eb-stack** against a real EasyBuild universe:

```bash
eb-stack check-recipe \
  --recipe migration/eon-foss-2024a/e/eOn/eOn-2.16.0-foss-2024a.eb \
  --easyconfigs ~/.venvs/easybuild/easybuild/easyconfigs \
  --easyconfigs migration/eon-foss-2024a
```

```bash
export EASYBUILD_ROBOT_PATHS=$PWD/migration/eon-foss-2024a:$EASYBUILD_ROBOT_PATHS
eb eOn-2.16.0-foss-2024a.eb --robot
```

## Contribution path

Upstream-ready copies also live on the EasyBuild easyconfigs fork:

- https://github.com/HaoZeke/easybuild-easyconfigs/tree/feat/eon-2.16.0-foss-2024a

This directory is the in-repo migration overlay required by the eb-stack
workflow (closure under robot∪overlay). Greenfield companions (quill,
metatensor*, metatomic-torch) and GCCcore-13.3.0 backports (Meson 1.8.2,
Rust 1.88.0) are included; deps already in the robot (PyTorch 2.6.0,
cargo-c, CapnProto, xtb, …) are not duplicated here.
