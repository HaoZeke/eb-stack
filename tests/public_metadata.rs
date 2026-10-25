use std::path::PathBuf;
use std::process::Command;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Whether `text` names `package` as an identifier or literal rather than as
/// part of a longer word.
///
/// A raw substring test cannot tell `"eon"` in a match arm from the middle of
/// "someone", and the second kind of hit teaches people to write around the
/// guard instead of fixing what it found. Boundaries keep every real case:
/// package identity appears in code inside quotes, or beside `.`, `-` and `,`,
/// all of which are boundaries.
fn names_package_identity(text: &str, package: &str) -> bool {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    text.match_indices(package).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + package.len()..].chars().next();
        !before.is_some_and(is_word) && !after.is_some_and(is_word)
    })
}

#[test]
fn the_guard_reads_identifiers_and_not_prose() {
    assert!(names_package_identity(r#"name == "eon""#, "eon"));
    assert!(names_package_identity("matches!(pkg, \"eon-akmc\")", "eon"));
    assert!(names_package_identity("- eon, gromacs", "gromacs"));
    assert!(!names_package_identity(
        "a document someone can verify",
        "eon"
    ));
    assert!(!names_package_identity("the eons since", "eon"));
    assert!(!names_package_identity("scipy_bundle_name", "scipy"));
}

#[test]
fn production_rust_has_no_package_identity_branches() {
    for entry in std::fs::read_dir(repo().join("src")).expect("read source directory") {
        let path = entry.expect("source entry").path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("rs") {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("read Rust source");
        let production = source
            .split_once("#[cfg(test)]")
            .map_or(source.as_str(), |(production, _)| production);
        let production = production.to_ascii_lowercase();
        // Names that were once branched on in production code and now live in
        // data/overlay-policy.toml. Keeping them here is what stops the next
        // one being added back as a match arm.
        for package in [
            "eon", "qmcpack", "lammps", "gromacs", "numpy", "scipy", "pytorch", "readcon",
        ] {
            assert!(
                !names_package_identity(&production, package),
                "{} contains package-specific production text for {package}",
                path.display()
            );
        }
    }
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
fn local_agent_state_stays_out_of_public_commits() {
    let gitignore = std::fs::read_to_string(repo().join(".gitignore")).expect("read .gitignore");
    for path in [".claude/", ".codex/"] {
        assert!(
            gitignore.lines().any(|line| line == path),
            ".gitignore must exclude {path}"
        );
    }
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
        "cargo package --locked",
    ] {
        assert!(ci.contains(command), "test workflow must run {command}");
    }
    for suite in [
        "--test package_catalog",
        "--test package_closure",
        "--test package_closure_write",
        "--test package_sources_discovery",
        "--test mcp_v1",
        "--test campaign",
        "--test reproduce_real_prs",
    ] {
        assert!(
            ci.contains(suite),
            "test workflow must run deterministic suite {suite}"
        );
    }
    assert!(
        !ci.contains("&& exit 1 || true"),
        "expected-failure smoke checks must reject unexpected success"
    );
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

#[test]
fn unpublished_endpoints_are_excluded_from_external_link_checks() {
    let input = std::fs::read_to_string(repo().join("lychee.toml")).expect("read lychee config");
    let config: toml::Value = toml::from_str(&input).expect("parse lychee config");
    let patterns = config["exclude"]
        .as_array()
        .expect("exclude array")
        .iter()
        .map(|value| value.as_str().expect("exclude regex"))
        .map(|pattern| regex::Regex::new(pattern).expect("valid exclude regex"))
        .collect::<Vec<_>>();
    for url in [
        "https://crates.io/crates/eb_stack",
        "https://docs.rs/eb_stack",
        "https://eb-stack.rgoswami.me/tutorial.html",
        "https://github.com/HaoZeke/eb-stack",
        "https://github.com/example/project/commit/example.patch",
    ] {
        assert!(
            patterns.iter().any(|pattern| pattern.is_match(url)),
            "Lychee must exclude unpublished endpoint {url}"
        );
    }
}

#[test]
fn fetch_script_exports_one_name_per_pinned_tree() {
    let text = std::fs::read_to_string(repo().join("scripts/fetch-upstream-trees.sh"))
        .expect("read fetch script");
    for gone in [
        "EB_EASYCONFIGS_V541",
        "EB_STACK_EC_DEVELOP",
        "EB_STACK_EC_V541",
    ] {
        assert!(!text.contains(gone), "{gone} is still in the fetch script");
    }
    for kept in [
        "EB_EASYCONFIGS_DEVELOP",
        "EB_EASYCONFIGS_541",
        "EB_EASYCONFIGS=",
        "EB_EASYBLOCKS=",
        "EB_EASYBUILD_FRAMEWORK=",
    ] {
        assert!(text.contains(kept), "fetch script missing {kept}");
    }
}
#[test]
fn the_eessi_reach_test_stays_off_the_network_and_the_source_tree() {
    let text = std::fs::read_to_string(repo().join("tests/component_reach_eessi.rs"))
        .expect("read eessi test");
    assert!(
        text.contains("#[ignore"),
        "the EESSI reach test must be ignored"
    );
    assert!(!text.contains("git clone"), "{text}");
    assert!(!text.contains("api.github.com"), "{text}");
    assert!(!text.contains("CARGO_MANIFEST_DIR"), "{text}");
    assert!(
        !text.contains("UreqClient"),
        "the EESSI reach test must read OSV from the recorded fixture"
    );
    assert!(text.contains("EB_EASYCONFIGS_DEVELOP"));
    assert!(text.contains("EB_EESSI_SOFTWARE_LAYER"));
    assert!(text.contains("std::env::temp_dir()"));
    let script = std::fs::read_to_string(repo().join("scripts/fetch-upstream-trees.sh"))
        .expect("read fetch script");
    assert!(
        script.contains("export EB_EESSI_SOFTWARE_LAYER="),
        "fetch script must publish the pinned software-layer checkout"
    );
    assert!(script.contains("b2c10c7278f8b741f4053ddb2907dc4d062063d1"));
}

/// Every `tests/*.rs` binary is either named by the default CI workflow,
/// or it is ignore-only and named by the upstream-trees workflow.
/// `real_tree_scale` needs a local robot tree and stays off GitHub runners.
#[test]
fn ci_runs_every_integration_test_file() {
    let ci = std::fs::read_to_string(repo().join(".github/workflows/ci_test.yml"))
        .expect("read ci_test.yml");
    let upstream = std::fs::read_to_string(repo().join(".github/workflows/ci_upstream_trees.yml"))
        .expect("read ci_upstream_trees.yml");
    assert!(
        upstream.contains("--include-ignored"),
        "upstream workflow must pass --include-ignored"
    );
    for entry in std::fs::read_dir(repo().join("tests")).expect("read tests") {
        let path = entry.expect("tests entry").path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }
        let name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .expect("stem");
        if name == "real_tree_scale" {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("read test");
        let tests = source.matches("#[test]").count();
        let ignores = source.matches("#[ignore").count();
        let ignore_only = tests > 0 && ignores >= tests;
        if ignore_only {
            assert!(
                upstream.contains(name),
                "{name} is ignore-only and is not named by ci_upstream_trees.yml"
            );
        } else {
            assert!(
                ci.contains(&format!("--test {name}")),
                "ci_test.yml does not run --test {name}"
            );
        }
    }
}

/// Slugs returned by `https://crates.io/api/v1/categories` on 2026-10-25.
const CRATES_IO_CATEGORIES: &[&str] = &[
    "accessibility",
    "aerospace",
    "algorithms",
    "api-bindings",
    "artificial-intelligence",
    "asynchronous",
    "authentication",
    "automotive",
    "caching",
    "command-line-interface",
    "command-line-utilities",
    "compilers",
    "compression",
    "computer-vision",
    "concurrency",
    "config",
    "cryptography",
    "data-structures",
    "database",
    "database-implementations",
    "date-and-time",
    "development-tools",
    "email",
    "embedded",
    "emulators",
    "encoding",
    "external-ffi-bindings",
    "filesystem",
    "finance",
    "game-development",
    "game-engines",
    "games",
    "graphics",
    "gui",
    "hardware-support",
    "internationalization",
    "localization",
    "mathematics",
    "memory-management",
    "multimedia",
    "network-programming",
    "no-std",
    "os",
    "parser-implementations",
    "parsing",
    "rendering",
    "rust-patterns",
    "science",
    "security",
    "simulation",
    "template-engine",
    "text-editors",
    "text-processing",
    "value-formatting",
    "virtualization",
    "visualization",
    "wasm",
    "web-programming",
];

fn string_list(value: &toml::Value) -> Vec<String> {
    value
        .as_array()
        .expect("array")
        .iter()
        .map(|item| item.as_str().expect("string").to_string())
        .collect()
}

/// The PyPI trove classifier list. Cached under `target/`, not stored in git.
///
/// `None` means the cache is missing and the download failed before any
/// bytes arrived. A present file is returned as stored. A download that
/// exits non-zero after writing bytes, or that exits zero with a truncated
/// list, fails the test and does not replace the cache.
fn pypi_classifiers() -> Option<String> {
    let path = repo().join("target/index-fixtures/pypi-classifiers.txt");
    if path.is_file() {
        return Some(std::fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!("read {}: {error}", path.display());
        }));
    }
    let url = "https://pypi.org/pypi?%3Aaction=list_classifiers";
    let output = Command::new("curl")
        .args(["-fsSL", "--retry", "3", "--retry-delay", "1", url])
        .output()
        .unwrap_or_else(|error| panic!("curl {url}: {error}"));
    if !output.status.success() {
        if output.stdout.is_empty() {
            eprintln!(
                "skip: {url} was not downloaded and {} is missing: {}",
                path.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            );
            return None;
        }
        panic!(
            "partial download of {url}: curl wrote {} bytes then failed: {}",
            output.stdout.len(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let text = String::from_utf8(output.stdout)
        .unwrap_or_else(|error| panic!("download of {url} is not utf-8: {error}"));
    assert!(
        text.contains("Programming Language :: Rust"),
        "download of {url} finished but the body is incomplete"
    );
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .unwrap_or_else(|error| panic!("create {}: {error}", parent.display()));
    }
    std::fs::write(&path, &text)
        .unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
    Some(text)
}

#[test]
fn discovery_metadata_uses_real_index_terms() {
    let root = repo();
    let cargo: toml::Value =
        toml::from_str(&std::fs::read_to_string(root.join("Cargo.toml")).expect("read Cargo.toml"))
            .expect("parse Cargo.toml");
    let package = &cargo["package"];
    let keywords = string_list(&package["keywords"]);
    let categories = string_list(&package["categories"]);
    assert_eq!(keywords, vec!["easybuild", "resolvo", "sbom", "hpc", "mcp"]);
    assert!(keywords.len() <= 5);
    assert_eq!(
        categories,
        vec![
            "command-line-utilities",
            "development-tools",
            "science",
            "parser-implementations",
            "config",
        ]
    );
    assert!(categories.len() <= 5);
    for category in &categories {
        assert!(
            CRATES_IO_CATEGORIES.contains(&category.as_str()),
            "{category} is not a crates.io category"
        );
    }

    let python: toml::Value = toml::from_str(
        &std::fs::read_to_string(root.join("pyproject.toml")).expect("read pyproject.toml"),
    )
    .expect("parse pyproject.toml");
    let project = &python["project"];
    assert_eq!(string_list(&project["keywords"]), keywords);
    assert_eq!(project["version"].as_str(), package["version"].as_str());
    let classifiers = string_list(&project["classifiers"]);
    let Some(allowed) = pypi_classifiers() else {
        return;
    };
    let allowed: std::collections::HashSet<&str> =
        allowed.lines().filter(|line| !line.is_empty()).collect();
    assert!(!classifiers.is_empty());
    for classifier in &classifiers {
        assert!(
            allowed.contains(classifier.as_str()),
            "{classifier} is not a PyPI classifier"
        );
    }

    let cog = std::fs::read_to_string(root.join("cog.toml")).expect("read cog.toml");
    assert!(cog.contains("pyproject.toml"));

    let readme = std::fs::read_to_string(root.join("README.md")).expect("read README");
    for needle in [
        "llms.txt",
        "llms-full.txt",
        "AGENTS.md",
        "server.json",
        "skills/",
    ] {
        assert!(readme.contains(needle), "README must point at {needle}");
    }
}
