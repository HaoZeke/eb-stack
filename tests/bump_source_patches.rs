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

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

const REBASED: &str = "--- delta-2.0.orig/util.c\n+++ delta-2.0/util.c\n@@ -1,3 +1,3 @@\n int util(void) {\n-  return 2;\n+  return 3;\n }\n";

fn patch_summaries(bundle: &eb_stack::PackageBundle) -> Vec<String> {
    bundle
        .plan
        .residuals
        .iter()
        .filter(|residual| residual.category == "patch-decision")
        .map(|residual| residual.summary.clone())
        .collect()
}

#[test]
fn a_rebased_patch_beside_the_recipe_replaces_the_one_that_conflicts() {
    let temp = tempfile::tempdir().unwrap();
    let robot = temp.path().join("robot");
    fs::create_dir_all(&robot).unwrap();
    let source = write_fixture(temp.path());
    fs::write(temp.path().join("Delta-2.0_conflicts.patch"), REBASED).unwrap();
    let (bundle, _) = plan_package_bump_fetching(
        &request(&source, &robot, true),
        &PackageEmitOptions::default(),
        &client(),
    )
    .expect("the rebased patch settles the only undecided one");
    let summaries = patch_summaries(&bundle);
    assert!(
        summaries.iter().any(|s| s.starts_with(
            "replace Delta-1.0_conflicts.patch: with Delta-2.0_conflicts.patch, which applies to \
             the delta-2.0.tar.gz source at -p1"
        )),
        "{summaries:?}"
    );
    let text = &bundle.easyconfigs[0].text;
    let digest = sha256_hex(REBASED.as_bytes());
    assert!(text.contains("'Delta-2.0_conflicts.patch',\n"), "{text}");
    assert!(
        text.contains(&format!("{{'Delta-2.0_conflicts.patch': '{digest}'}}")),
        "{text}"
    );
    assert!(!text.contains("Delta-1.0_conflicts.patch"), "{text}");
    assert!(!text.contains("4444444444"), "{text}");
    let artifact = bundle
        .plan
        .build
        .patches
        .iter()
        .find(|patch| patch.filename == "Delta-2.0_conflicts.patch")
        .expect("the plan carries the rebased patch");
    assert_eq!(artifact.sha256.as_deref(), Some(digest.as_str()));
    assert_eq!(
        artifact.resolved_source.as_deref(),
        Some(temp.path().join("Delta-2.0_conflicts.patch").as_path())
    );
}

#[test]
fn a_rebased_patch_is_found_under_an_easyconfigs_root_and_must_apply() {
    let temp = tempfile::tempdir().unwrap();
    let robot = temp.path().join("robot");
    let overlay = temp.path().join("overlay");
    fs::create_dir_all(&robot).unwrap();
    fs::create_dir_all(overlay.join("d/Delta")).unwrap();
    let source = write_fixture(temp.path());
    // A rebased file that does not apply settles nothing.
    fs::write(
        overlay.join("d/Delta/Delta-2.0_conflicts.patch"),
        "--- a/util.c\n+++ b/util.c\n@@ -1,3 +1,3 @@\n int nothing(void) {\n-  return 9;\n+  return 10;\n }\n",
    )
    .unwrap();
    let mut req = request(&source, &robot, false);
    req.easyconfig_roots.push(overlay.clone());
    let (bundle, _) =
        plan_package_bump_fetching(&req, &PackageEmitOptions::default(), &client()).expect("bump");
    let summaries = patch_summaries(&bundle);
    assert!(
        summaries
            .iter()
            .any(|s| s.starts_with("undecided Delta-1.0_conflicts.patch")
                && s.contains("the rebased Delta-2.0_conflicts.patch does not settle it either")),
        "{summaries:?}"
    );
    fs::write(overlay.join("d/Delta/Delta-2.0_conflicts.patch"), REBASED).unwrap();
    let (bundle, _) =
        plan_package_bump_fetching(&req, &PackageEmitOptions::default(), &client()).expect("bump");
    let summaries = patch_summaries(&bundle);
    assert!(
        summaries
            .iter()
            .any(|s| s
                .starts_with("replace Delta-1.0_conflicts.patch: with Delta-2.0_conflicts.patch")),
        "{summaries:?}"
    );
}

#[test]
fn an_operator_drop_is_recorded_with_its_reason_and_an_unknown_name_fails() {
    use eb_stack::source_fetch::{plan_package_bump_fetching_with, BumpDecisions};
    let temp = tempfile::tempdir().unwrap();
    let robot = temp.path().join("robot");
    fs::create_dir_all(&robot).unwrap();
    let source = write_fixture(temp.path());
    let decisions = BumpDecisions {
        drop_patches: vec![(
            "Delta-1.0_conflicts.patch".into(),
            "fixed upstream by commit abc123".into(),
        )],
        ..BumpDecisions::default()
    };
    let (bundle, _) = plan_package_bump_fetching_with(
        &request(&source, &robot, true),
        &PackageEmitOptions::default(),
        &client(),
        &decisions,
    )
    .expect("the operator settles the only undecided patch");
    let decision = bundle
        .plan
        .residuals
        .iter()
        .find(|residual| {
            residual
                .summary
                .starts_with("drop Delta-1.0_conflicts.patch")
        })
        .expect("the drop is recorded");
    assert!(
        decision.summary.contains(
            "fixed upstream by commit abc123 (operator decision; the source alone: \
             does not apply to the delta-2.0.tar.gz source"
        ),
        "{}",
        decision.summary
    );
    assert_eq!(decision.severity, ResidualSeverity::Mechanical);
    let text = &bundle.easyconfigs[0].text;
    assert!(!text.contains("Delta-1.0_conflicts.patch"), "{text}");
    assert!(!text.contains("4444444444"), "{text}");

    let unknown = BumpDecisions {
        drop_patches: vec![("Delta-1.0_typo.patch".into(), "a reason".into())],
        ..BumpDecisions::default()
    };
    let error = plan_package_bump_fetching_with(
        &request(&source, &robot, false),
        &PackageEmitOptions::default(),
        &client(),
        &unknown,
    )
    .expect_err("a drop that names nothing fails");
    assert!(
        error.to_string().contains("Delta-1.0_typo.patch"),
        "{error}"
    );
}

#[test]
fn a_secondary_source_moves_to_its_newest_stable_release() {
    use eb_stack::source_fetch::{plan_package_bump_fetching_with, BumpDecisions};
    let temp = tempfile::tempdir().unwrap();
    let robot = temp.path().join("robot");
    fs::create_dir_all(&robot).unwrap();
    let source = temp.path().join("Gamma-1.0-GCCcore-15.2.0.eb");
    fs::write(
        &source,
        "easyblock = 'ConfigureMake'\nname = 'Gamma'\nversion = '1.0'\n\
         local_extra_version = '1.1'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Synthetic package'\n\
         toolchain = {'name': 'GCCcore', 'version': '15.2.0'}\n\
         source_urls = [\n    'https://example.invalid/src/',\n    \
         'https://github.com/acme/extra/releases/download/v%s/' % local_extra_version,\n]\n\
         sources = [\n    SOURCELOWER_TAR_GZ,\n    'extra-%s.tar.gz' % local_extra_version,\n]\n\
         checksums = [\n    {'gamma-1.0.tar.gz': 'cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc'},\n    \
         {'extra-1.1.tar.gz': 'eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee'},\n]\n\
         moduleclass = 'tools'\n",
    )
    .unwrap();
    let extra = tar_gz(&[("extra-1.2/README", "extra 1.2\n")]);
    let mut client = MapClient::default();
    client.pages.insert(
        "https://example.invalid/src/gamma-2.0.tar.gz".into(),
        tar_gz(&[("gamma-2.0/main.c", NEW_MAIN)]),
    );
    client.pages.insert(
        "https://api.github.com/repos/acme/extra/releases?per_page=100".into(),
        br#"[{"tag_name": "v1.1"}, {"tag_name": "v1.2"}, {"tag_name": "v2.0rc1", "prerelease": true}]"#
            .to_vec(),
    );
    client.pages.insert(
        "https://github.com/acme/extra/releases/download/v1.2/extra-1.2.tar.gz".into(),
        extra.clone(),
    );
    let decisions = BumpDecisions {
        latest_secondary: true,
        ..BumpDecisions::default()
    };
    let (bundle, fetched) = plan_package_bump_fetching_with(
        &request(&source, &robot, false),
        &PackageEmitOptions::default(),
        &client,
        &decisions,
    )
    .expect("bump");
    let text = &bundle.easyconfigs[0].text;
    let digest = sha256_hex(&extra);
    assert!(text.contains("local_extra_version = '1.2'\n"), "{text}");
    assert!(
        text.contains(&format!("{{'extra-1.2.tar.gz': '{digest}'}}")),
        "{text}"
    );
    assert!(!text.contains("eeeeeeeeee"), "{text}");
    let moved = bundle
        .plan
        .residuals
        .iter()
        .find(|residual| residual.category == "secondary-source")
        .expect("the move is recorded");
    assert_eq!(
        moved.summary,
        "extra 1.1 -> 1.2: newest stable release on github:acme/extra"
    );
    assert_eq!(moved.severity, ResidualSeverity::Mechanical);
    // The old file's download failure is not a residual of the new recipe.
    assert!(
        !bundle
            .plan
            .residuals
            .iter()
            .any(|residual| residual.summary.contains("extra-1.1.tar.gz")),
        "{:?}",
        bundle.plan.residuals
    );
    assert!(fetched
        .iter()
        .any(|fetch| fetch.filename == "extra-1.2.tar.gz"
            && fetch.sha256.as_deref() == Some(digest.as_str())));
    assert!(!fetched
        .iter()
        .any(|fetch| fetch.filename == "extra-1.1.tar.gz"));
    assert_eq!(
        bundle.plan.sources[1].sha256.as_deref(),
        Some(digest.as_str())
    );
}

#[test]
fn a_local_commit_of_the_old_release_tag_moves_to_the_new_tags_commit() {
    let temp = tempfile::tempdir().unwrap();
    let robot = temp.path().join("robot");
    fs::create_dir_all(&robot).unwrap();
    let old = "d1dc28bb95f3e35b4c9d1d7536cb1da6de4d7aa7";
    let new = "0b06695c24e10f6de6b9166c35ae24804c21f95f";
    let source = temp.path().join("Delta-1.0+7-GCCcore-15.2.0.eb");
    fs::write(
        &source,
        format!(
            "easyblock = 'CMakeMake'\nname = 'Delta'\nversion = '1.0+7'\n\
             local_git_rev = '{old}'\n\
             homepage = 'https://example.invalid/'\ndescription = 'Synthetic package'\n\
             toolchain = {{'name': 'GCCcore', 'version': '15.2.0'}}\n\
             source_urls = ['https://github.com/acme/delta/archive']\n\
             sources = ['v%(version)s.tar.gz']\n\
             checksums = [\n    {{'v1.0+7.tar.gz': 'cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc'}},\n]\n\
             configopts = '-DREV=' + local_git_rev\n\
             moduleclass = 'tools'\n"
        ),
    )
    .unwrap();
    let mut client = MapClient::default();
    client.pages.insert(
        "https://github.com/acme/delta/archive/v2.0+3.tar.gz".into(),
        tar_gz(&[("delta-2.0-3/main.c", NEW_MAIN)]),
    );
    client.pages.insert(
        "https://api.github.com/repos/acme/delta/commits/v1.0%2B7".into(),
        format!("{{\"sha\": \"{old}\"}}").into_bytes(),
    );
    client.pages.insert(
        "https://api.github.com/repos/acme/delta/commits/v2.0%2B3".into(),
        format!("{{\"sha\": \"{new}\"}}").into_bytes(),
    );
    let mut req = request(&source, &robot, false);
    req.version = Some("2.0+3".into());
    let (bundle, _) =
        plan_package_bump_fetching(&req, &PackageEmitOptions::default(), &client).expect("bump");
    let text = &bundle.easyconfigs[0].text;
    assert!(
        text.contains(&format!("local_git_rev = '{new}'\n")),
        "{text}"
    );
    assert!(!text.contains(old), "{text}");
    let moved = bundle
        .plan
        .residuals
        .iter()
        .find(|residual| residual.category == "release-commit")
        .expect("the move is recorded");
    assert_eq!(moved.severity, ResidualSeverity::Mechanical);
    assert_eq!(
        moved.summary,
        "local_git_rev d1dc28bb95f3 -> 0b06695c24e1: the commit of acme/delta v2.0+3, \
         as d1dc28bb95f3 was of v1.0+7"
    );

    // A pin that is not the old release's commit is left as written.
    client.pages.insert(
        "https://api.github.com/repos/acme/delta/commits/v1.0%2B7".into(),
        br#"{"sha": "1111111111111111111111111111111111111111"}"#.to_vec(),
    );
    let (bundle, _) =
        plan_package_bump_fetching(&req, &PackageEmitOptions::default(), &client).expect("bump");
    assert!(bundle.easyconfigs[0].text.contains(old));
    assert!(bundle
        .plan
        .residuals
        .iter()
        .any(|residual| residual.category == "release-commit"
            && residual.severity == ResidualSeverity::Judgment
            && residual.summary.contains("left as written")));
}
