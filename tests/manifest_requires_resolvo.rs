use eb_stack::package::{StackPolicy, STACK_POLICY_SCHEMA_VERSION};
use eb_stack::{plan_new_package, ForeignFormat, NewPackageRequest, Toolchain};

fn write_easyconfig(root: &std::path::Path, filename: &str, text: &str) {
    std::fs::write(root.join(filename), text).expect("write easyconfig fixture");
}

#[test]
fn hierarchy_candidates_cannot_replace_an_unsatisfiable_resolvo_solve() {
    let toolchain = Toolchain {
        name: "foss".into(),
        version: "2026.1".into(),
    };
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("recipe.yaml");
    std::fs::write(
        &source,
        r#"
package:
  name: app
  version: 1.0
requirements:
  host:
    - hdf5 >=1.14
    - zlib >=1.3
"#,
    )
    .expect("foreign recipe");
    let robot = temp.path().join("robot");
    std::fs::create_dir(&robot).expect("robot");
    write_easyconfig(
        &robot,
        "HDF5-1.14.2-foss-2026.1.eb",
        r#"
name = 'HDF5'
version = '1.14.2'
toolchain = {'name': 'foss', 'version': '2026.1'}
dependencies = [('zlib', '1.2')]
"#,
    );
    write_easyconfig(
        &robot,
        "zlib-1.2-foss-2026.1.eb",
        "name = 'zlib'\nversion = '1.2'\ntoolchain = {'name': 'foss', 'version': '2026.1'}\n",
    );
    write_easyconfig(
        &robot,
        "zlib-1.3-foss-2026.1.eb",
        "name = 'zlib'\nversion = '1.3'\ntoolchain = {'name': 'foss', 'version': '2026.1'}\n",
    );

    let error = plan_new_package(&NewPackageRequest {
        source,
        format: Some(ForeignFormat::CondaForge),
        toolchain: toolchain.clone(),
        source_checksums: Vec::new(),
        package_layers: Vec::new(),
        easyconfig_roots: vec![robot],
        stack_policy: StackPolicy {
            schema_version: STACK_POLICY_SCHEMA_VERSION,
            name: "test".into(),
            toolchain,
            pins: Vec::new(),
            exclusions: Vec::new(),
        },
    })
    .expect_err("incompatible dependency requirements must fail");
    assert!(error.to_string().contains("solve"), "{error}");
    assert!(error.to_string().contains("unsatisfiable"), "{error}");
}
