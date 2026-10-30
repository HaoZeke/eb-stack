//! A version bump with `--fetch-checksums` decides each patch the tree cannot
//! decide by trying it against the downloaded source, the way EasyBuild's
//! patch step will.

use eb_stack::package::{ResidualSeverity, StackPolicy, STACK_POLICY_SCHEMA_VERSION};
use eb_stack::registry::MapClient;
use eb_stack::source_fetch::plan_package_bump_fetching;
use eb_stack::{BumpPackageRequest, PackageEmitOptions, Toolchain};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

fn tar_gz(files: &[(&str, &str)]) -> Vec<u8> {
    use flate2::write::GzEncoder;
    use flate2::Compression;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    {
        let mut builder = tar::Builder::new(&mut encoder);
        for (path, content) in files {
            let data = content.as_bytes();
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, *path, data).unwrap();
        }
        builder.finish().unwrap();
    }
    encoder.finish().unwrap()
}

fn request(source: &Path, robot: &Path, strict: bool) -> BumpPackageRequest {
    let toolchain = Toolchain {
        name: "GCCcore".into(),
        version: "15.2.0".into(),
    };
    BumpPackageRequest {
        source: source.to_path_buf(),
        toolchain: toolchain.clone(),
        version: Some("2.0".into()),
        source_checksum: None,
        easyconfig_roots: vec![robot.to_path_buf()],
        hierarchy_fixture: None,
        overrides: HashMap::new(),
        stack_policy: StackPolicy {
            schema_version: STACK_POLICY_SCHEMA_VERSION,
            name: "default".into(),
            toolchain,
            pins: Vec::new(),
            exclusions: Vec::new(),
        },
        strict_patches: strict,
        package_layers: Vec::new(),
    }
}

const NEW_MAIN: &str = "int main(void) {\n  return 0;\n}\n";
const NEW_UTIL: &str = "int util(void) {\n  return 2;\n}\n";

/// A patch that applies, one that 2.0 already contains, one for a file 2.0
/// no longer ships, and one that conflicts.
fn write_fixture(dir: &Path) -> std::path::PathBuf {
    fs::write(
        dir.join("Delta-1.0_applies.patch"),
        "--- delta-1.0.orig/main.c\n+++ delta-1.0/main.c\n@@ -1,3 +1,3 @@\n int main(void) {\n-  return 0;\n+  return 1;\n }\n",
    )
    .unwrap();
    fs::write(
        dir.join("Delta-1.0_upstream.patch"),
        "--- delta-1.0.orig/util.c\n+++ delta-1.0/util.c\n@@ -1,3 +1,3 @@\n int util(void) {\n-  return 1;\n+  return 2;\n }\n",
    )
    .unwrap();
    fs::write(
        dir.join("Delta-1.0_gone.patch"),
        "--- delta-1.0.orig/plugins/old.c\n+++ delta-1.0/plugins/old.c\n@@ -1 +1 @@\n-a\n+b\n",
    )
    .unwrap();
    fs::write(
        dir.join("Delta-1.0_conflicts.patch"),
        "--- delta-1.0.orig/util.c\n+++ delta-1.0/util.c\n@@ -1,3 +1,3 @@\n int helper(void) {\n-  return 7;\n+  return 8;\n }\n",
    )
    .unwrap();
    let source = dir.join("Delta-1.0-GCCcore-15.2.0.eb");
    fs::write(
        &source,
        "easyblock = 'ConfigureMake'\nname = 'Delta'\nversion = '1.0'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Synthetic package'\n\
         toolchain = {'name': 'GCCcore', 'version': '15.2.0'}\n\
         source_urls = ['https://example.invalid/src/']\n\
         sources = ['delta-%(version)s.tar.gz']\n\
         patches = [\n    '%(name)s-%(version)s_applies.patch',\n    'Delta-1.0_upstream.patch',\n    \
         'Delta-1.0_gone.patch',\n    'Delta-1.0_conflicts.patch',\n]\n\
         checksums = [\n    {'delta-1.0.tar.gz': 'cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc'},\n    \
         {'Delta-1.0_applies.patch': '1111111111111111111111111111111111111111111111111111111111111111'},\n    \
         {'Delta-1.0_upstream.patch': '2222222222222222222222222222222222222222222222222222222222222222'},\n    \
         {'Delta-1.0_gone.patch': '3333333333333333333333333333333333333333333333333333333333333333'},\n    \
         {'Delta-1.0_conflicts.patch': '4444444444444444444444444444444444444444444444444444444444444444'},\n]\n\
         moduleclass = 'tools'\n",
    )
    .unwrap();
    source
}

fn client() -> MapClient {
    let mut client = MapClient::default();
    client.pages.insert(
        "https://example.invalid/src/delta-2.0.tar.gz".into(),
        tar_gz(&[
            ("delta-2.0/main.c", NEW_MAIN),
            ("delta-2.0/util.c", NEW_UTIL),
        ]),
    );
    client
}

#[test]
fn each_patch_is_carried_dropped_or_left_by_the_new_source() {
    let temp = tempfile::tempdir().unwrap();
    let robot = temp.path().join("robot");
    fs::create_dir_all(&robot).unwrap();
    let source = write_fixture(temp.path());
    let (bundle, fetched) = plan_package_bump_fetching(
        &request(&source, &robot, false),
        &PackageEmitOptions::default(),
        &client(),
    )
    .expect("bump");
    assert!(fetched[0].sha256.is_some());
    let text = &bundle.easyconfigs[0].text;
    let decisions: Vec<(String, ResidualSeverity)> = bundle
        .plan
        .residuals
        .iter()
        .filter(|residual| residual.category == "patch-decision")
        .map(|residual| (residual.summary.clone(), residual.severity))
        .collect();
    let find = |prefix: &str| {
        decisions
            .iter()
            .find(|(summary, _)| summary.starts_with(prefix))
            .unwrap_or_else(|| panic!("no {prefix}: {decisions:?}"))
            .1
    };
    assert_eq!(
        find("carry Delta-1.0_applies.patch: applies"),
        ResidualSeverity::Mechanical
    );
    assert_eq!(
        find("drop Delta-1.0_upstream.patch: the delta-2.0.tar.gz source already contains it"),
        ResidualSeverity::Mechanical
    );
    assert_eq!(
        find("drop Delta-1.0_gone.patch: no file it modifies exists"),
        ResidualSeverity::Mechanical
    );
    assert_eq!(
        find("undecided Delta-1.0_conflicts.patch: does not apply"),
        ResidualSeverity::Judgment
    );
    // The templated name keeps the release the file was written for.
    assert!(text.contains("'%(name)s-1.0_applies.patch'"), "{text}");
    assert!(text.contains("'Delta-1.0_conflicts.patch'"), "{text}");
    assert!(!text.contains("Delta-1.0_upstream.patch"), "{text}");
    assert!(!text.contains("Delta-1.0_gone.patch"), "{text}");
    assert!(!text.contains("2222222222"), "{text}");
    assert!(!text.contains("3333333333"), "{text}");
    assert!(text.contains("4444444444"), "{text}");
    assert!(bundle
        .plan
        .build
        .patches
        .iter()
        .all(|patch| patch.filename != "Delta-1.0_upstream.patch"));
    // Per-patch evidence replaces the blanket review warning.
    assert!(!bundle
        .plan
        .residuals
        .iter()
        .any(|residual| residual.summary.contains("patches were not modified")));
}

#[test]
fn strict_patches_fails_only_on_what_the_new_source_leaves_undecided() {
    let temp = tempfile::tempdir().unwrap();
    let robot = temp.path().join("robot");
    fs::create_dir_all(&robot).unwrap();
    let source = write_fixture(temp.path());
    let error = plan_package_bump_fetching(
        &request(&source, &robot, true),
        &PackageEmitOptions::default(),
        &client(),
    )
    .expect_err("one patch conflicts");
    let message = error.to_string();
    assert!(message.contains("Delta-1.0_conflicts.patch"), "{message}");
    assert!(!message.contains("Delta-1.0_applies.patch"), "{message}");
    assert!(!message.contains("Delta-1.0_upstream.patch"), "{message}");
}

#[test]
fn the_bumped_extensions_own_patches_are_judged_against_its_new_sdist() {
    let temp = tempfile::tempdir().unwrap();
    let robot = temp.path().join("robot");
    fs::create_dir_all(&robot).unwrap();
    fs::write(
        temp.path().join("widget-0.1_upstream.patch"),
        "--- a/util.c\n+++ b/util.c\n@@ -1,3 +1,3 @@\n int util(void) {\n-  return 1;\n+  return 2;\n }\n",
    )
    .unwrap();
    fs::write(
        temp.path().join("widget-0.1_conflicts.patch"),
        "--- a/util.c\n+++ b/util.c\n@@ -1,3 +1,3 @@\n int helper(void) {\n-  return 7;\n+  return 8;\n }\n",
    )
    .unwrap();
    let source = temp.path().join("widget-1.0-GCCcore-15.2.0.eb");
    fs::write(
        &source,
        "easyblock = 'PythonBundle'\nname = 'widget'\nversion = '1.0'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Synthetic package'\n\
         toolchain = {'name': 'GCCcore', 'version': '15.2.0'}\n\
         exts_list = [\n    (name, version, {\n        'patches': [\n            ('widget-0.1_upstream.patch', 1),\n            \
         'widget-0.1_conflicts.patch',\n        ],\n        'checksums': [\n            \
         {'widget-1.0.tar.gz': 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'},\n            \
         {'widget-0.1_upstream.patch': '5555555555555555555555555555555555555555555555555555555555555555'},\n            \
         {'widget-0.1_conflicts.patch': '6666666666666666666666666666666666666666666666666666666666666666'},\n        ],\n    }),\n]\n\
         moduleclass = 'tools'\n",
    )
    .unwrap();
    let sdist = tar_gz(&[
        ("widget-2.0/main.c", NEW_MAIN),
        ("widget-2.0/util.c", NEW_UTIL),
    ]);
    let mut client = MapClient::default();
    client.pages.insert(
        "https://pypi.org/pypi/widget/2.0/json".into(),
        br#"{"info": {"version": "2.0"}, "urls": [{"packagetype": "sdist", "filename": "widget-2.0.tar.gz", "url": "https://files.example/widget-2.0.tar.gz"}]}"#.to_vec(),
    );
    client
        .pages
        .insert("https://files.example/widget-2.0.tar.gz".into(), sdist);
    let (bundle, _) = plan_package_bump_fetching(
        &request(&source, &robot, false),
        &PackageEmitOptions::default(),
        &client,
    )
    .expect("bump");
    let text = &bundle.easyconfigs[0].text;
    let summaries: Vec<&str> = bundle
        .plan
        .residuals
        .iter()
        .map(|residual| residual.summary.as_str())
        .collect();
    assert!(
        summaries.iter().any(|s| s.starts_with(
            "drop widget-0.1_upstream.patch: the widget 2.0 extension archive already contains it"
        )),
        "{summaries:?}"
    );
    assert!(
        summaries
            .iter()
            .any(|s| s.starts_with("undecided widget-0.1_conflicts.patch: does not apply")),
        "{summaries:?}"
    );
    assert!(!text.contains("widget-0.1_upstream.patch"), "{text}");
    assert!(!text.contains("5555555555"), "{text}");
    assert!(text.contains("'widget-0.1_conflicts.patch'"), "{text}");
    // The archive checksum is the new sdist's, and the patch digest stays.
    assert!(text.contains("6666666666"), "{text}");
    assert!(!text.contains("aaaaaaaaaa"), "{text}");
}
