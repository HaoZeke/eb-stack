use std::path::PathBuf;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn public_policies_describe_the_executable_campaign_surface() {
    let code_of_conduct =
        std::fs::read_to_string(repo().join("CODE_OF_CONDUCT.md")).expect("read code of conduct");
    assert!(!code_of_conduct.contains("INSERT CONTACT METHOD"));
    assert!(code_of_conduct.contains("rgoswami@ieee.org"));

    let security =
        std::fs::read_to_string(repo().join("SECURITY.md")).expect("read security policy");
    assert!(security.contains("executes EasyBuild recipes"));
    assert!(security.contains("untrusted recipe"));
    assert!(security.contains("SSH"));
    assert!(security.contains("container"));
}

#[test]
fn package_and_citation_metadata_cover_the_public_workflow() {
    let cargo = std::fs::read_to_string(repo().join("Cargo.toml")).expect("read Cargo.toml");
    for term in ["conda-forge", "Spack", "Resolvo", "CycloneDX"] {
        assert!(cargo.contains(term), "Cargo metadata must mention {term}");
    }

    let citation = std::fs::read_to_string(repo().join("CITATION.cff")).expect("read CITATION.cff");
    for term in [
        "conda-forge",
        "Spack",
        "Resolvo",
        "CycloneDX",
        "build campaign",
    ] {
        assert!(
            citation.contains(term),
            "citation metadata must mention {term}"
        );
    }
    assert!(!citation.contains("date-released:"));
}

#[test]
fn generated_documentation_builds_stay_out_of_source_packages() {
    let gitignore = std::fs::read_to_string(repo().join(".gitignore")).expect("read .gitignore");
    assert!(gitignore.lines().any(|line| line == "docs/build*/"));

    let cargo = std::fs::read_to_string(repo().join("Cargo.toml")).expect("read Cargo.toml");
    assert!(cargo.contains("\"docs/build*/\""));
}

#[test]
fn ci_enforces_the_declared_msrv_and_quality_gates() {
    let cargo = std::fs::read_to_string(repo().join("Cargo.toml")).expect("read Cargo.toml");
    assert!(cargo.contains("rust-version = \"1.88\""));

    let ci = std::fs::read_to_string(repo().join(".github/workflows/ci_test.yml"))
        .expect("read test workflow");
    for command in [
        "toolchain: 1.88.0",
        "cargo check --locked --all-targets",
        "cargo fmt --all --check",
        "cargo clippy --locked --all-targets -- -D warnings",
    ] {
        assert!(ci.contains(command), "test workflow must run {command}");
    }
}

#[test]
fn public_manual_uses_only_the_version_one_cli() {
    let manual_paths = [
        "docs/orgmode/explanation/parser-approach.org",
        "docs/orgmode/explanation/fidelity.org",
        "docs/orgmode/howto/emit-reports.org",
    ];
    let manual = manual_paths
        .iter()
        .map(|path| std::fs::read_to_string(repo().join(path)).expect("read manual source"))
        .collect::<Vec<_>>()
        .join("\n");

    for obsolete in [
        "eb-stack parse",
        "~bump --easyconfigs",
        "~parse~ / ~solve~",
        "--keep-old-deps",
        "~solve~ yet",
    ] {
        assert!(
            !manual.contains(obsolete),
            "public manual still names removed CLI surface {obsolete}"
        );
    }
    for canonical in [
        "eb-stack package plan",
        "eb-stack package bump",
        "eb-stack stack solve",
    ] {
        assert!(
            manual.contains(canonical),
            "public manual must name canonical command {canonical}"
        );
    }
}

#[test]
fn documentation_ci_is_warning_strict_and_checks_rendered_links() {
    let sphinx = std::fs::read_to_string(repo().join("scripts/sphinx-build-docs.sh"))
        .expect("read Sphinx build script");
    assert!(sphinx.contains("-W --keep-going"));

    let ci = std::fs::read_to_string(repo().join(".github/workflows/ci_docs.yml"))
        .expect("read docs workflow");
    assert!(ci.contains("scripts/check-doc-links.sh docs/build"));
    assert!(ci.contains("lychee --config lychee.toml"));
}
