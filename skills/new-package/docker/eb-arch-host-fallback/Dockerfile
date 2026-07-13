# Optional fallback when host OS-dep name checks fight EasyBuild.
# Prefer host Arch + full-drive --ignore-osdeps when verbs.h is present.
# This image is Debian-based so EB package-name probes match apt packages.
FROM debian:bookworm-slim

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential gfortran python3 python3-pip python3-venv \
    git curl ca-certificates \
    libibverbs-dev rdma-core libnuma-dev \
    libssl-dev libffi-dev bzip2 unzip xz-utils \
    && rm -rf /var/lib/apt/lists/*

# Install EasyBuild into a venv (pin loosely; site may bind-mount its own)
RUN python3 -m venv /opt/eb && /opt/eb/bin/pip install --upgrade pip \
    && /opt/eb/bin/pip install easybuild
ENV PATH=/opt/eb/bin:$PATH
ENV EASYBUILD_PREFIX=/eb
ENV EASYBUILD_MODULES_TOOL=Lmod
# Lmod optional in minimal image; site can inject modules via volume

WORKDIR /work
# Usage (from site EasyBuild host):
#   docker build -t eb-osdeps-bookworm -f skills/new-package/docker/eb-arch-host-fallback/Dockerfile .
#   docker run --rm -v "$WORK:/work" -v "$ROBOT:/robot:ro" -v "$HOME/tmp/eb-sources:/sources" \
#     -e EASYBUILD_SOURCEPATH=/sources -e EASYBUILD_TMPDIR=/tmp/eb-tmp \
#     -e EASYBUILD_INSTALLPATH=/work/install \
#     eb-osdeps-bookworm \
#     eb --robot /work/easyconfigs:/robot /work/easyconfigs/.../Name.eb
#
# Prefer fixing host Arch with rdma-core + campaign --ignore-osdeps; use this
# image only when host OS-dep probing cannot be trusted even with headers.
