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
