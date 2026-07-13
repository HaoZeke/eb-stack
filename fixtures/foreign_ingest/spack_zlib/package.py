# Fixture: Spack package.py subset for eb-stack restricted ingest (no exec).
# Copyright Spack Project Developers. SPDX-License-Identifier: (Apache-2.0 OR MIT)

from spack.package import *


class Zlib(Package):
    """A free, general-purpose, lossless data-compression library."""

    homepage = "https://zlib.net"
    url = "https://github.com/madler/zlib/releases/download/v1.3.1/zlib-1.3.1.tar.gz"

    license("Zlib")

    version("1.3.1", sha256="9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23")
    version("1.2.13", sha256="b3a24de97a8fdbc835b9833169501030b8977031bcb54b3b3ac13740f846ab30")

    depends_on("c", type="build")
    depends_on("gmake", type="build")
