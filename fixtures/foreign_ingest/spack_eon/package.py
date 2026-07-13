# Copyright Spack Project Developers. See COPYRIGHT file for details.
#
# SPDX-License-Identifier: (Apache-2.0 OR MIT)

import os

from spack_repo.builtin.build_systems.meson import MesonPackage

from spack.package import *


class Eon(MesonPackage):
    """Long-timescale atomistic simulation (AKMC, NEB, saddle search).

    Core CPU recipe: ``eonclient`` plus the Python AKMC package
    (``import eon``). Optional ML / XTB / serve surfaces are deferred.
    """

    homepage = "https://eondocs.org/"
    url = "https://github.com/TheochemUI/eOn/releases/download/v2.16.0/eon-v2.16.0.tar.xz"
    git = "https://github.com/TheochemUI/eOn.git"

    maintainers("HaoZeke")
    license("BSD-3-Clause", checked_by="HaoZeke")

    version("2.16.0", sha256="3d4da89a393c8821bf370cb97c9d2403718d83f9cbb5e8b918cd90af14ed52dc")

    # Core first (see packaging blueprint): fortran + tests, no ML/serve/xtb.
    variant("fortran", default=True, description="Build in-tree Fortran potentials")
    variant("cuh2", default=True, description="Build CuH2 potential (requires +fortran)")
    variant("tests", default=True, description="Build eOn client unit tests")

    depends_on("c", type="build")
    depends_on("cxx", type="build")
    depends_on("fortran", type="build", when="+fortran")
    depends_on("meson@1.8.0:", type="build")
    depends_on("cmake", type="build")

    depends_on("python@3.11:", type=("build", "run"))
    depends_on("py-numpy", type=("build", "run"))
    depends_on("py-pyyaml", type=("build", "run"))
    extends("python")

    depends_on("eigen@3.4:")
    depends_on("quill@11:")
    depends_on("readcon-core@0.13.1:")
    depends_on("highway")
    depends_on("libinih")
    depends_on("blas")
    depends_on("lapack")

    conflicts("+cuh2", when="~fortran", msg="CuH2 requires Fortran")

    # Always use the meson wrap payload offline (avoids network at configure).
    resource(
        name="nlohmann_json",
        url="https://github.com/nlohmann/json/releases/download/v3.12.0/include.zip",
        sha256="b8cb0ef2dd7f57f18933997c9934bb1fa962594f701cd5a8d3c2c80541559372",
        destination="subprojects",
        placement="nlohmann_json-3.12.0",
    )

    @run_before("meson")
    def drop_network_wraps(self):
        """Prefer Spack-provided deps over meson wraps that download sources."""
        sub = join_path(self.stage.source_path, "subprojects")
        for name in (
            "xtb.wrap",
            "vesin.wrap",
            "readcon-core.wrap",
            "rgpot.wrap",
            "highway.wrap",
            "inih.wrap",
        ):
            path = join_path(sub, name)
            if os.path.exists(path):
                os.remove(path)

    def meson_args(self):
        true_false = lambda v: "true" if v else "false"
        return [
            f"-Dwith_fortran={true_false('+fortran' in self.spec)}",
            f"-Dwith_cuh2={true_false('+cuh2' in self.spec)}",
            f"-Dwith_tests={true_false('+tests' in self.spec)}",
            "-Dwith_xtb=false",
            "-Dwith_serve=false",
            "-Dwith_rgpot=false",
            "-Dwith_metatomic=false",
            "-Dpip_metatomic=false",
            "-Dwith_artn=false",
            "-Dwith_ira=false",
            "-Dwith_parallel_neb=false",
            "-Dwith_catlearn=false",
            "-Dwith_ase=false",
            "-Dwith_ase_orca=false",
            "-Dwith_ase_nwchem=false",
            "-Dwith_gp_surrogate=false",
            "-Dwith_gprd=false",
            "-Dwith_vasp=false",
            "-Dwith_ams=false",
            "-Dwith_mpi=false",
            "-Dwith_qsc=false",
            "-Dwith_water=false",
            "-Dwith_newpot=false",
            "-Dnative_arch=false",
            "-Dfast_math=false",
            "-Duse_mkl=false",
            "-Dpython.install_env=prefix",
        ]

    def setup_build_environment(self, env: EnvironmentModifications) -> None:
        for dep in ("readcon-core", "quill", "highway", "libinih"):
            if dep in self.spec:
                pc = join_path(self.spec[dep].prefix, "lib", "pkgconfig")
                if os.path.isdir(pc):
                    env.prepend_path("PKG_CONFIG_PATH", pc)

    def test_eonclient(self):
        """Smoke-check the installed client binary."""
        eonclient = which(self.prefix.bin.eonclient)
        eonclient("--help")
