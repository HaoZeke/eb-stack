# Docker fallback for EasyBuild OS-dep name checks

## Prefer host first (Arch)

On Arch, install:

```bash
sudo pacman -S --needed rdma-core
test -f /usr/include/infiniband/verbs.h
```

Set EasyBuild environment or command options in the site target layer when
those headers exist. Keep the workaround target-local rather than changing a
submitted recipe.

## When to use this image

Only if you need a Debian userspace where EasyBuild’s `libibverbs-dev` /
`rdma-core-devel` package-name probes succeed via `dpkg`, or you want an
isolated build root.

Build and run are documented in the Dockerfile comments. Configure the image
as a `docker` target runtime, bind-mount the work tree, robot easyconfigs, and
source path, and keep site hostnames out of images and public skills.
