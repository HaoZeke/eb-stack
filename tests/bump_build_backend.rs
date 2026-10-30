//! A version bump reads the new release's `pyproject.toml` and adds the build
//! backend it declares, mapped onto the modules of the target hierarchy.

use eb_stack::build_backend::{declared_build_in, map_build_requirements, DeclaredBuild};
use eb_stack::package::{StackPolicy, STACK_POLICY_SCHEMA_VERSION};
use eb_stack::registry::MapClient;
use eb_stack::source_fetch::plan_package_bump_fetching;
use eb_stack::{resolve_easyconfig_str, BumpPackageRequest, PackageEmitOptions, Toolchain};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

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

fn module(robot: &Path, name: &str, version: &str, extra: &str) {
    let directory = robot.join(name.to_lowercase());
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join(format!("{name}-{version}-GCCcore-15.2.0.eb")),
        format!(
            "easyblock = 'ConfigureMake'\nname = '{name}'\nversion = '{version}'\n\
             homepage = 'https://example.invalid/'\ndescription = 'Module'\n\
             toolchain = {{'name': 'GCCcore', 'version': '15.2.0'}}\n\
             sources = []\nchecksums = []\n{extra}moduleclass = 'tools'\n"
        ),
    )
    .unwrap();
}

/// Python with the build tools it bundles, maturin and Rust; no hatchling.
fn robot(dir: &Path) -> PathBuf {
    let robot = dir.join("robot");
    module(
        &robot,
        "Python",
        "3.14.2",
        "exts_list = [\n    ('setuptools', '80.9.0'),\n    ('wheel', '0.45.1'),\n    \
         ('setuptools_scm', '9.2.2'),\n]\n",
    );
    module(&robot, "maturin", "1.13.1", "");
    module(&robot, "Rust", "1.94.1", "");
    robot
}

fn source(dir: &Path) -> PathBuf {
    let source = dir.join("Tiny-1.0-GCCcore-15.2.0.eb");
    fs::write(
        &source,
        "easyblock = 'PythonPackage'\nname = 'Tiny'\nversion = '1.0'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Synthetic package'\n\
         toolchain = {'name': 'GCCcore', 'version': '15.2.0'}\n\
         source_urls = ['https://example.invalid/src/']\n\
         sources = ['tiny-%(version)s.tar.gz']\n\
         checksums = ['cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc']\n\n\
         dependencies = [\n    ('Python', '3.14.2'),\n]\n\n\
         moduleclass = 'tools'\n",
    )
    .unwrap();
    source
}

fn request(source: PathBuf, robot: PathBuf) -> BumpPackageRequest {
    let toolchain = Toolchain {
        name: "GCCcore".into(),
        version: "15.2.0".into(),
    };
    BumpPackageRequest {
        source,
        toolchain: toolchain.clone(),
        version: Some("2.0".into()),
        source_checksum: None,
        easyconfig_roots: vec![robot],
        hierarchy_fixture: None,
        overrides: HashMap::new(),
        stack_policy: StackPolicy {
            schema_version: STACK_POLICY_SCHEMA_VERSION,
            name: "default".into(),
            toolchain,
            pins: Vec::new(),
            exclusions: Vec::new(),
        },
        strict_patches: false,
        package_layers: Vec::new(),
    }
}

const MATURIN_PYPROJECT: &str = "[project]\nname = \"tiny\"\nversion = \"2.0\"\n\n\
    [build-system]\nrequires = [\"maturin>=1.4,<2.0\"]\nbuild-backend = \"maturin\"\n\n\
    [tool.maturin]\nmanifest-path = \"tiny/rust/Cargo.toml\"\n";

const CARGO_LOCK: &str = r#"
version = 4

[[package]]
name = "tiny"
version = "2.0.0"

[[package]]
name = "libc"
version = "0.2.177"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
"#;

fn client(pyproject: &str, with_crate: bool) -> MapClient {
    let mut files = vec![
        ("tiny-2.0/pyproject.toml", pyproject),
        ("tiny-2.0/tiny/__init__.py", ""),
    ];
    if with_crate {
        files.push((
            "tiny-2.0/tiny/rust/Cargo.toml",
            "[package]\nname = \"tiny\"\nversion = \"2.0.0\"\n",
        ));
        files.push(("tiny-2.0/tiny/rust/Cargo.lock", CARGO_LOCK));
    }
    let mut client = MapClient::default();
    client.pages.insert(
        "https://example.invalid/src/tiny-2.0.tar.gz".into(),
        tar_gz(&files),
    );
    client
}

#[test]
fn the_declared_build_system_is_read_from_the_project_root() {
    let files = vec![
        (
            "pulp-4.2.0/pyproject.toml".to_string(),
            MATURIN_PYPROJECT.as_bytes().to_vec(),
        ),
        (
            "pulp-4.2.0/vendor/other/pyproject.toml".to_string(),
            b"[build-system]\nrequires = [\"flit_core\"]\n".to_vec(),
        ),
        (
            "pulp-4.2.0/pulp/rust/Cargo.lock".to_string(),
            CARGO_LOCK.as_bytes().to_vec(),
        ),
    ];
    let declared = declared_build_in(&files).expect("declared");
    assert_eq!(declared.requires, vec!["maturin".to_string()]);
    assert_eq!(declared.backend.as_deref(), Some("maturin"));
    assert!(declared.cargo_lock);
    assert!(declared.builds_rust_crate());
}

#[test]
fn a_requirement_a_dependency_bundles_adds_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let robot = robot(temp.path());
    let tree = eb_stack::eb_parse::parse_easyconfig_trees(&[robot.as_path()]).unwrap();
    let python = tree
        .candidates
        .iter()
        .find(|candidate| candidate.name == "Python")
        .unwrap();
    let declared = DeclaredBuild {
        backend: Some("setuptools.build_meta".into()),
        requires: vec!["setuptools".into(), "setuptools-scm".into(), "wheel".into()],
        cargo_lock: false,
    };
    let mapped = map_build_requirements(&declared, &[python], &tree.candidates);
    assert!(
        mapped
            .iter()
            .all(|entry| entry.module.is_none() && entry.provided_by.as_deref() == Some("Python")),
        "{mapped:?}"
    );
}

#[test]
fn a_maturin_release_gains_maturin_rust_and_its_crates() {
    let temp = tempfile::tempdir().unwrap();
    let robot = robot(temp.path());
    let source = source(temp.path());
    let (bundle, _fetched) = plan_package_bump_fetching(
        &request(source, robot),
        &PackageEmitOptions::default(),
        &client(MATURIN_PYPROJECT, true),
    )
    .expect("bump");
    let text = &bundle.easyconfigs[0].text;
    let recipe = resolve_easyconfig_str(text).expect("parses");
    assert_eq!(
        recipe.easyblock.as_deref(),
        Some("CargoPythonPackage"),
        "{text}"
    );
    let build = recipe
        .builddependencies
        .iter()
        .map(|dependency| (dependency.name.as_str(), dependency.version.as_str()))
        .collect::<Vec<_>>();
    assert!(build.contains(&("maturin", "1.13.1")), "{text}");
    assert!(build.contains(&("Rust", "1.94.1")), "{text}");
    assert!(text.contains("('libc', '0.2.177')"), "{text}");
    assert!(
        text.contains("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        "the crate checksum comes from Cargo.lock:\n{text}"
    );
    assert!(
        text.find("sources =").unwrap() < text.find("crates =").unwrap(),
        "crates follows sources:\n{text}"
    );
    assert_eq!(
        bundle.plan.build.easyblock.as_deref(),
        Some("CargoPythonPackage")
    );
    assert!(
        !bundle
            .plan
            .residuals
            .iter()
            .any(|residual| residual.category == "build-requirement-unmapped"),
        "{:?}",
        bundle.plan.residuals
    );
}

#[test]
fn a_setuptools_release_is_left_as_it_was() {
    let temp = tempfile::tempdir().unwrap();
    let robot = robot(temp.path());
    let source = source(temp.path());
    let pyproject = "[build-system]\nrequires = [\"setuptools>=61\", \"wheel\"]\n\
                     build-backend = \"setuptools.build_meta\"\n";
    let (bundle, _fetched) = plan_package_bump_fetching(
        &request(source, robot),
        &PackageEmitOptions::default(),
        &client(pyproject, false),
    )
    .expect("bump");
    let text = &bundle.easyconfigs[0].text;
    assert!(!text.contains("builddependencies"), "{text}");
    assert!(text.contains("easyblock = 'PythonPackage'"), "{text}");
}

#[test]
fn a_backend_no_module_provides_is_reported() {
    let temp = tempfile::tempdir().unwrap();
    let robot = robot(temp.path());
    let source = source(temp.path());
    let pyproject =
        "[build-system]\nrequires = [\"hatchling\"]\nbuild-backend = \"hatchling.build\"\n";
    let (bundle, _fetched) = plan_package_bump_fetching(
        &request(source, robot),
        &PackageEmitOptions::default(),
        &client(pyproject, false),
    )
    .expect("bump");
    assert!(
        bundle.plan.residuals.iter().any(|residual| {
            residual.category == "build-requirement-unmapped"
                && residual.summary.contains("hatchling")
        }),
        "{:?}",
        bundle.plan.residuals
    );
}
