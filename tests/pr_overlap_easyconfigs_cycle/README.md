# Two-patch cycle fixture

`Alpha` depends on `Beta` and `Beta` depends on `Alpha`.
Pull request 1 patches `Alpha`. Pull request 2 patches `Beta`.
Each patch builds a dependent of the other, so the `depends-on` rows
point both ways and the merge order reports one cycle.
