A CRAN package whose imports are not in the robot tree, so the plan has
leftovers and has to emit a `Bundle` with `exts_defaultclass = 'RPackage'`
rather than a single `RPackage`. The robot carries only R itself.
