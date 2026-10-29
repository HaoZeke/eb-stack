# Java filter fixture

`App` is a GCCcore recipe whose only dependency is `('Java', '21', '', SYSTEM)`.
Pull request 1 patches `Java`. Pull request 2 patches `App`.
Without `--filter-deps`, `App` is a reverse dependent of that patch and a
`depends-on` row orders Java first. With `--filter-deps Java`, `App` is
absent from the reverse dependents and there is no `depends-on` row.
