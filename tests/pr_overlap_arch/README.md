Shape of EESSI software-layer pull request 1775.

The shared easystack already lists GDRCopy. The pull request adds an
unrelated entry. GDRCopy is installed on x86_64 and not on riscv64, so the
bot build set includes it only on riscv64. Lib is a dependency of GDRCopy
and is not installed on either architecture. The walk stops at an installed
module, so Lib is in the riscv64 set and not in the x86_64 set.

Pull request 1800 adds a different unrelated entry to the same file, so
merge order and impact have a second build set that also carries GDRCopy
on riscv64.
