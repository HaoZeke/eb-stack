use eb_stack::{
    parse_foreign_str, plan_from_foreign, solve_plan_with_robot, ForeignFormat, IngestOpts,
    Toolchain,
};

fn write_easyconfig(root: &std::path::Path, filename: &str, text: &str) {
    std::fs::write(root.join(filename), text).expect("write easyconfig fixture");
}

#[test]
fn hierarchy_consensus_cannot_replace_an_unsatisfiable_resolvo_solve() {
    let recipe = parse_foreign_str(
        ForeignFormat::CondaForge,
        r#"
package:
  name: app
  version: 1.0
requirements:
  host:
    - hdf5 >=1.14
    - zlib >=1.2
"#,
    )
    .expect("parse foreign recipe");
    let toolchain = Toolchain {
        name: "foss".into(),
        version: "2026.1".into(),
    };
    let mut plan = plan_from_foreign(&recipe, &toolchain);

    let easyconfigs = tempfile::tempdir().expect("easyconfig tree");
    write_easyconfig(
        easyconfigs.path(),
        "HDF5-1.14.2-foss-2026.1.eb",
        r#"
name = 'HDF5'
version = '1.14.2'
toolchain = {'name': 'foss', 'version': '2026.1'}
dependencies = [('zlib', '1.2')]
"#,
    );
    write_easyconfig(
        easyconfigs.path(),
        "zlib-1.2-foss-2026.1.eb",
        r#"
name = 'zlib'
version = '1.2'
toolchain = {'name': 'foss', 'version': '2026.1'}
"#,
    );
    write_easyconfig(
        easyconfigs.path(),
        "zlib-1.3-foss-2026.1.eb",
        r#"
name = 'zlib'
version = '1.3'
toolchain = {'name': 'foss', 'version': '2026.1'}
"#,
    );

    let error = solve_plan_with_robot(
        &mut plan,
        &IngestOpts {
            easyconfigs: vec![easyconfigs.path().to_path_buf()],
            keep_old_deps: false,
            hierarchy_fixture: None,
        },
    )
    .expect_err("incompatible hierarchy pins must fail the plan");
    assert!(error.to_string().contains("resolvo"), "{error}");
    assert!(
        plan.solved.is_none(),
        "an unsatisfiable plan cannot be solved"
    );
}
