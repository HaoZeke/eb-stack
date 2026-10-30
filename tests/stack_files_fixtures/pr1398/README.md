Shape of EESSI software-layer pull request 1398.

The easystack pins Xfce 4.20 to commit 7e74f85 of easybuild-easyconfigs
pull request 25365. Xfce is a Bundle. Its xfce4-terminal component names
two patches, and both sit beside the easyconfig in x/Xfce/. The bundle's
fetch finds them there. The component install fetches them again under
x/xfce4-terminal/, and for a from-commit easyconfig it does not look beside
the easyconfig (easybuild-framework issue 5133), so the build failed with
"Couldn't find file xfce-terminal-1.1.5_fix_version.patch anywhere".

robot-5.2.0 stands in for the EasyBuild 5.2.0 tree, which has no Xfce.
robot-develop holds the same easyconfig and patches on the robot path, for
the case without a pin: then the easyconfig's directory is searched and the
patches are found. sourcepath holds the patches under x/xfce4-terminal/,
which is where the pull request's workaround put them.

Patch files are placeholders: only their presence matters.
