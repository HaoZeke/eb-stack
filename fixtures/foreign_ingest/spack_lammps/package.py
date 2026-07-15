# Extracted from spack/spack-packages at
# 79c931fee8b045fc6ff39e399b0ef209c6269b59.
import datetime as dt

from spack.package import *


class Lammps(CMakePackage, CudaPackage, ROCmPackage, PythonExtension):
    """LAMMPS stands for Large-scale Atomic/Molecular Massively Parallel Simulator."""

    homepage = "https://www.lammps.org/"
    url = "https://github.com/lammps/lammps/archive/patch_1Sep2017.tar.gz"
    git = "https://github.com/lammps/lammps.git"

    license("GPL-2.0-only")

    version("develop", branch="develop")
    version("20260704", sha256="be9deffba169d140c337fd29570d3f5469332ece7e77280cce998f6caaad5534")
    version(
        "20250722.4",
        sha256="411088d9c03339e025f6a975e0a5741bb9e3f351cc39eda220ab22ac318fe2fb",
        preferred=True,
    )

    stable_versions = {
        "20250722.4",
        "20250722.3",
        "20250722.2",
        "20250722.1",
        "20250722",
    }

    def url_for_version(self, version):
        split_ver = str(version).split(".")
        vdate = dt.datetime.strptime(split_ver[0], "%Y%m%d")
        if len(split_ver) < 2:
            update = ""
        else:
            update = "_update{0}".format(split_ver[1])

        return "https://github.com/lammps/lammps/archive/{0}_{1}{2}.tar.gz".format(
            "stable" if str(version) in Lammps.stable_versions else "patch",
            vdate.strftime("%d%b%Y").lstrip("0"),
            update,
        )

    variant("mpi", default=True, description="Build with MPI")
    variant("openmp", default=True, description="Build with OpenMP")
    variant("kokkos", default=False, description="Build with Kokkos")
    variant("kspace", default=True, description="Build the KSPACE package")
    variant(
        "fft",
        default="fftw3",
        when="+kspace",
        description="FFT library for KSPACE package",
        values=("kiss", "fftw3", "mkl", "nvpl"),
        multi=False,
    )
    variant(
        "fft_kokkos",
        default="fftw3",
        when="@20240417: +kspace+kokkos",
        description="FFT library for Kokkos-enabled KSPACE package",
        values=("kiss", "fftw3", "mkl", "mkl_gpu", "nvpl", "hipfft", "cufft"),
        multi=False,
    )

    depends_on("c", type="build")
    depends_on("cxx", type="build")
    depends_on("cmake@3.16:", type="build")
    depends_on("mpi", when="+mpi")
    depends_on("kokkos+shared@3.1:", when="@20200505:+kokkos")
    depends_on("kokkos@4.6.02:", when="@20250722:+kokkos+kspace")
    depends_on("fftw-api@3", when="+kspace fft=fftw3")
    depends_on("mkl", when="+kspace fft=mkl")
    depends_on("fftw-api@3", when="+kokkos+kspace fft_kokkos=fftw3")
    depends_on("mkl", when="+kokkos+kspace fft_kokkos=mkl")
    depends_on(
        "scafacos cflags=-fPIC cxxflags=-fPIC fflags=-fPIC",
        when="+scafacos+lib",
    )

    resource(
        name="C_10_10.mesocnt",
        url="https://download.lammps.org/potentials/C_10_10.mesocnt",
        sha256="923f600a081d948eb8b4510f84aa96167b5a6c3e1aba16845d2364ae137dc346",
        expand=False,
        placement={"C_10_10.mesocnt": "potentials/C_10_10.mesocnt"},
        when="+mesont",
    )
