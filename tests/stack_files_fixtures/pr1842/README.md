Shape of EESSI software-layer pull request 1842.

The easystack rebuilds Python 3.10.8 from easybuild-easyconfigs pull
request 27224. The first push pinned the pull request head, 97a4c1a. Its
pip extension names pip-20.3.4_remove-emSign-root-cert.patch, and that
commit's p/Python has no such file. The EasyBuild 5.4.1 robot tree has none
either, so every target failed with "Couldn't find file ... anywhere". The
merge commit 5837547 has the patch, and the bot built with that pin.

The easyconfig is trimmed to the fields the lookup reads. Patch files are
placeholders: only their presence matters.
