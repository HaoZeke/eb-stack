Shape of EESSI software-layer pull request 1048.

The easystack rebuilds SciPy-bundle 2023.07 for icelake and cascadelake
from the reprod copy EESSI installed for haswell, written as its /cvmfs
path. That copy names scipy-1.11.1_vectorization_error.patch for the scipy
extension. The reprod directory keeps no extension patches, and the
EasyBuild 4.9.4 robot tree predates that patch, so the build failed with
"Couldn't find file scipy-1.11.1_vectorization_error.patch anywhere". The
pull request dropped SciPy-bundle and later entries.

cvmfs stands in for /cvmfs, with the reprod copy at its real path, and is
reached with --path-map /cvmfs=<this directory>/cvmfs. robot-develop has the patch, which easybuild-easyconfigs added
in October 2024.

The easyconfig is trimmed to the fields the lookup reads. Patch files are
placeholders: only their presence matters.
