//! `recipe check --verify-sources` refuses a checksum copied from a different
//! artifact class.
//!
//! The shape reproduced here is the LLVM 22.1.8 bump: the foreign recipe hashed
//! the tarball GitHub generates from the tag, while the easyconfig downloads the
//! asset the project uploaded to the release. Same version, different bytes,
//! and a checksum that is well-formed and wrong.

use std::process::Command;

const RELEASE_ASSET_RECIPE: &str = "\
easyblock = 'CMakeMake'

name = 'LLVMdemo'
version = '22.1.8'

homepage = 'https://llvm.org/'
description = \"Release-asset shaped source, as easyconfigs commonly use.\"

toolchain = {'name': 'GCCcore', 'version': '14.3.0'}

source_urls = ['https://github.com/llvm/llvm-project/releases/download/llvmorg-%(version)s']
sources = ['llvm-project-%(version)s.src.tar.xz']
checksums = ['0000000000000000000000000000000000000000000000000000000000000000']

moduleclass = 'compiler'
";

const TAG_ARCHIVE_URL: &str =
    "https://github.com/llvm/llvm-project/archive/refs/tags/llvmorg-22.1.8.tar.gz";
const RELEASE_ASSET_URL: &str =
    "https://github.com/llvm/llvm-project/releases/download/llvmorg-22.1.8/llvm-project-22.1.8.src.tar.xz";

struct Fixture {
    _dir: tempfile::TempDir,
    recipe: std::path::PathBuf,
    tree: std::path::PathBuf,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let tree = dir.path().to_path_buf();
    let recipe = tree.join("LLVMdemo-22.1.8-GCCcore-14.3.0.eb");
    std::fs::write(&recipe, RELEASE_ASSET_RECIPE).expect("write recipe");
    Fixture {
        _dir: dir,
        recipe,
        tree,
    }
}

fn check(fx: &Fixture, extra: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args(["recipe", "check", "--recipe"])
        .arg(&fx.recipe)
        .arg("--easyconfigs")
        .arg(&fx.tree)
        .args(["--metadata-only"])
        .args(extra)
        .output()
        .expect("run recipe check")
}

#[test]
fn a_tag_archive_checksum_on_a_release_asset_recipe_fails_the_check() {
    let fx = fixture();
    let out = check(
        &fx,
        &[
            "--verify-sources",
            "--seeded-from",
            "conda-forge",
            "--seeded-source-url",
            TAG_ARCHIVE_URL,
        ],
    );
    assert!(
        !out.status.success(),
        "a cross-class checksum passed: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(combined.contains("github-tag-archive"), "{combined}");
    assert!(combined.contains("github-release-asset"), "{combined}");
    assert!(
        combined.contains("source verification failed"),
        "{combined}"
    );
}

#[test]
fn a_checksum_from_the_same_artifact_class_passes() {
    let fx = fixture();
    let out = check(
        &fx,
        &[
            "--verify-sources",
            "--seeded-from",
            "conda-forge",
            "--seeded-source-url",
            RELEASE_ASSET_URL,
        ],
    );
    assert!(
        out.status.success(),
        "a matching class was rejected: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn the_source_class_is_reported_even_without_a_seed() {
    let fx = fixture();
    let out = check(&fx, &["--verify-sources"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("source_class=github-release-asset"),
        "{stdout}"
    );
}

#[test]
fn the_check_is_unchanged_when_the_flag_is_absent() {
    let fx = fixture();
    let out = check(&fx, &[]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("source_class="),
        "classification leaked into the default path: {stdout}"
    );
}
