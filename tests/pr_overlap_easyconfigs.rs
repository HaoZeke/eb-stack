//! `eb-stack pr-overlap --kind easyconfigs` against a four-file tree.

use std::path::{Path, PathBuf};
use std::process::Command;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/pr_overlap_easyconfigs_fixtures")
}

fn head(name: &str, dep: Option<(&str, &str)>, description: &str) -> String {
    let deps = match dep {
        Some((dep_name, version)) => format!("dependencies = [('{dep_name}', '{version}')]\n\n"),
        None => String::new(),
    };
    format!(
        "easyblock = 'ConfigureMake'\n\n\
name = '{name}'\n\
version = '1.0'\n\n\
homepage = 'https://example.org/{name}'\n\
description = \"{description}\"\n\n\
toolchain = SYSTEM\n\n\
source_urls = ['https://example.org/']\n\
sources = ['{name}-1.0.tar.gz']\n\
checksums = ['0000000000000000000000000000000000000000000000000000000000000000']\n\n\
{deps}\
moduleclass = 'lib'\n"
    )
}

fn pr(
    number: u64,
    title: &str,
    path: &str,
    base: Option<&str>,
    head_text: &str,
) -> serde_json::Value {
    let mut change = serde_json::json!({ "path": path, "head": head_text });
    if let Some(base) = base {
        change["base"] = serde_json::json!(base);
    }
    serde_json::json!({
        "number": number,
        "head_sha": number.to_string().repeat(40),
        "title": title,
        "easyconfigs": [change],
    })
}

#[test]
fn the_cli_matches_the_easyconfig_fixture() {
    let dir = fixtures();
    let lib = std::fs::read_to_string(dir.join("l/Lib/Lib-1.0.eb")).unwrap();
    let dep = std::fs::read_to_string(dir.join("d/Dep/Dep-1.0.eb")).unwrap();
    let lib_head = lib.replace("Fixture.", "Head.");
    let dep_head = dep.replace("Fixture.", "Added.");
    let prs = vec![
        pr(1, "edit Lib", "l/Lib/Lib-1.0.eb", Some(&lib), &lib_head),
        pr(2, "edit Lib too", "l/Lib/Lib-1.0.eb", Some(&lib), &lib_head),
        pr(
            3,
            "add New",
            "n/New/New-1.0.eb",
            None,
            &head("New", Some(("Lib", "1.0")), "Head."),
        ),
        pr(
            4,
            "add New too",
            "n/New/New-1.0.eb",
            None,
            &head("New", Some(("Lib", "1.0")), "Head."),
        ),
        pr(5, "add Dep file", "contrib/Dep-1.0.eb", None, &dep_head),
        pr(
            6,
            "add App",
            "a/App/App-1.0.eb",
            None,
            &head("App", Some(("Dep", "1.0")), "Head."),
        ),
        pr(
            7,
            "add Need",
            "n/Need/Need-1.0.eb",
            None,
            &head("Need", None, "Head."),
        ),
        pr(
            8,
            "add User",
            "u/User/User-1.0.eb",
            None,
            &head("User", Some(("Need", "1.0")), "Head.").replace(
                "dependencies = [('Need', '1.0')]\n\n",
                "dependencies = [('Dep', '1.0'), ('Need', '1.0')]\n\n",
            ),
        ),
    ];
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("prs.json");
    std::fs::write(&input, serde_json::to_string(&prs).unwrap()).unwrap();
    let out = temp.path().join("out");
    let status = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .env_remove("EASYBUILD_FILTER_DEPS")
        .args(["pr-overlap", "--kind", "easyconfigs", "--pr"])
        .arg(&input)
        .arg("--easyconfigs")
        .arg(&dir)
        .args(["--generated-at", "2026-10-22T00:00:00Z"])
        .args([
            "--repo-url",
            "https://github.com/easybuilders/easybuild-easyconfigs",
        ])
        .arg("--out-dir")
        .arg(&out)
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let expected = dir.join("expected");
    for name in [
        "overlap.md",
        "overlap.toml",
        "prs/1.toml",
        "prs/2.toml",
        "prs/3.toml",
        "prs/4.toml",
        "prs/5.toml",
        "prs/6.toml",
        "prs/7.toml",
        "prs/8.toml",
    ] {
        let got = std::fs::read_to_string(out.join(name)).unwrap();
        let want = std::fs::read_to_string(expected.join(name)).unwrap();
        assert_eq!(got, want, "{name}");
    }
}

fn run_easy(out: &std::path::Path, extra: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .env_remove("EASYBUILD_FILTER_DEPS")
        .args(["pr-overlap", "--kind", "easyconfigs", "--pr"])
        .arg(fixtures().join("prs.json"))
        .arg("--easyconfigs")
        .arg(fixtures())
        .args(["--generated-at", "2026-10-24T00:00:00Z"])
        .args([
            "--repo-url",
            "https://github.com/easybuilders/easybuild-easyconfigs",
        ])
        .arg("--out-dir")
        .arg(out)
        .args(extra)
        .output()
        .expect("run pr-overlap")
}

/// `--impact` has to use the same installed set as `merge-order.toml`.
#[test]
fn impact_honours_merged_the_way_merge_order_does() {
    let temp = tempfile::tempdir().expect("temp");
    let out = temp.path().join("out");
    let status = run_easy(&out, &["--merged", "7", "--impact", "8"]);
    let err = String::from_utf8_lossy(&status.stderr);
    assert!(status.status.success(), "{err}");
    let order: eb_stack::merge_order::MergeOrder =
        toml::from_str(&std::fs::read_to_string(out.join("merge-order.toml")).unwrap()).unwrap();
    let step = order
        .steps
        .iter()
        .find(|step| step.pr == 8)
        .expect("PR 8 step");
    assert_eq!(step.builds, vec!["User/1.0".to_string()]);
    let report: eb_stack::merge_order::MergeImpactReport =
        toml::from_str(&std::fs::read_to_string(out.join("merge-impact.toml")).unwrap()).unwrap();
    let impact = report
        .impacts
        .iter()
        .find(|impact| impact.pr == 8)
        .expect("impact 8");
    assert!(
        impact.affected.iter().all(|effect| effect.pr != 7),
        "merged PR 7 is still affected: {:?}",
        impact.affected
    );
    for effect in &impact.affected {
        assert!(
            !effect.shrinks.iter().any(|module| module == "Need/1.0"),
            "Need/1.0 is already installed by merged PR 7: {:?}",
            impact.affected
        );
    }
    let both = run_easy(
        &temp.path().join("both"),
        &["--merged", "7", "--impact", "7"],
    );
    assert_eq!(both.status.code(), Some(2));
    let err = String::from_utf8_lossy(&both.stderr);
    assert!(
        err.contains("--impact 7: already listed in --merged"),
        "{err}"
    );
}
