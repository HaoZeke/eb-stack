//! Package-source catalog: layered TOML providers for recursive hole planning.

use eb_stack::package_catalog::{
    resolve_package_catalog_layers, PackageCatalogError, PackageCatalogLayer,
    PACKAGE_CATALOG_SCHEMA_VERSION,
};
use eb_stack::{ForeignFormat, Toolchain};
use std::path::{Path, PathBuf};

fn write_catalog(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("catalog parent");
    }
    std::fs::write(&path, body).expect("write catalog");
    path
}

#[test]
fn layered_catalog_resolves_paths_relative_to_each_catalog_file() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();

    // Layout:
    //   catalog/base.toml
    //   catalog/site.toml
    //   foreign/pkg/package.py
    //   policy/common.toml
    //   policy/pkg.toml
    //   stacks/foss.toml
    std::fs::create_dir_all(root.join("catalog")).unwrap();
    std::fs::create_dir_all(root.join("foreign/pkg")).unwrap();
    std::fs::create_dir_all(root.join("policy")).unwrap();
    std::fs::create_dir_all(root.join("stacks")).unwrap();
    std::fs::write(root.join("foreign/pkg/package.py"), "# foreign\n").unwrap();
    std::fs::write(root.join("policy/common.toml"), "schema_version = 1\n").unwrap();
    std::fs::write(root.join("policy/pkg.toml"), "schema_version = 1\n").unwrap();
    std::fs::write(
        root.join("stacks/foss.toml"),
        "schema_version = 1\nname = \"foss\"\n[toolchain]\nname = \"foss\"\nversion = \"2026.1\"\n",
    )
    .unwrap();

    let base = write_catalog(
        root,
        "catalog/base.toml",
        r#"
schema_version = 1

[[packages]]
name = "ExampleLib"
version = "1.2.3"
source = "../foreign/pkg/package.py"
format = "spack"
package_config = ["../policy/common.toml"]
source_checksums = ["0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"]
profile = "default"
toolchain = { name = "GCCcore", version = "15.2.0" }
"#,
    );
    let site = write_catalog(
        root,
        "catalog/site.toml",
        r#"
schema_version = 1

[[packages]]
name = "ExampleLib"
version = "1.2.3"
package_config = ["../policy/common.toml", "../policy/pkg.toml"]
stack_policy = "../stacks/foss.toml"
"#,
    );

    let layers = [
        PackageCatalogLayer::from_path(&base).expect("base catalog"),
        PackageCatalogLayer::from_path(&site).expect("site catalog"),
    ];
    let catalog = resolve_package_catalog_layers(&layers).expect("resolve catalog");
    let provider = catalog
        .lookup("ExampleLib", Some("1.2.3"))
        .expect("lookup provider");

    assert_eq!(provider.name, "ExampleLib");
    assert_eq!(provider.version.as_deref(), Some("1.2.3"));
    assert_eq!(provider.format, Some(ForeignFormat::Spack));
    assert_eq!(provider.profile, "default");
    assert_eq!(
        provider.toolchain,
        Toolchain {
            name: "GCCcore".into(),
            version: "15.2.0".into(),
        }
    );
    assert_eq!(
        provider.source_checksums,
        vec!["0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string()]
    );
    assert_eq!(
        provider.source.canonicalize().unwrap(),
        root.join("foreign/pkg/package.py").canonicalize().unwrap()
    );
    assert_eq!(provider.package_config.len(), 2);
    assert_eq!(
        provider.package_config[0].canonicalize().unwrap(),
        root.join("policy/common.toml").canonicalize().unwrap()
    );
    assert_eq!(
        provider.package_config[1].canonicalize().unwrap(),
        root.join("policy/pkg.toml").canonicalize().unwrap()
    );
    assert_eq!(
        provider
            .stack_policy
            .as_ref()
            .unwrap()
            .canonicalize()
            .unwrap(),
        root.join("stacks/foss.toml").canonicalize().unwrap()
    );
}

#[test]
fn catalog_lookup_is_case_and_punctuation_insensitive_on_name() {
    let layer = PackageCatalogLayer::from_toml_str(
        r#"
schema_version = 1

[[packages]]
name = "CapnProto"
version = "1.4.0"
source = "foreign/capnp/package.py"
format = "spack"
toolchain = { name = "GCCcore", version = "15.2.0" }
"#,
    )
    .expect("catalog");
    let catalog = resolve_package_catalog_layers(&[layer]).expect("resolve");

    let provider = catalog
        .lookup("capn-proto", Some("1.4.0"))
        .expect("normalized lookup");
    assert_eq!(provider.name, "CapnProto");
}

#[test]
fn catalog_rejects_unsupported_schema_version() {
    let err =
        PackageCatalogLayer::from_toml_str("schema_version = 99").expect_err("unsupported schema");
    assert!(matches!(err, PackageCatalogError::UnsupportedSchema(99)));
    assert_eq!(PACKAGE_CATALOG_SCHEMA_VERSION, 1);
}

#[test]
fn catalog_rejects_unknown_fields() {
    let err = PackageCatalogLayer::from_toml_str(
        r#"
schema_version = 1
unexpected_top_level = true
"#,
    )
    .expect_err("unknown field");
    assert!(matches!(err, PackageCatalogError::Toml(_)));
}

#[test]
fn catalog_rejects_unknown_package_fields() {
    let err = PackageCatalogLayer::from_toml_str(
        r#"
schema_version = 1

[[packages]]
name = "Lib"
source = "x.py"
format = "spack"
toolchain = { name = "foss", version = "2026.1" }
guessed_dep = "zlib"
"#,
    )
    .expect_err("unknown package field");
    assert!(matches!(err, PackageCatalogError::Toml(_)));
}

#[test]
fn resolve_rejects_duplicate_ambiguous_providers_in_one_layer() {
    let layer = PackageCatalogLayer::from_toml_str(
        r#"
schema_version = 1

[[packages]]
name = "Lib"
version = "1.0"
source = "a/package.py"
format = "spack"
toolchain = { name = "foss", version = "2026.1" }

[[packages]]
name = "lib"
version = "1.0"
source = "b/package.py"
format = "conda-forge"
toolchain = { name = "foss", version = "2026.1" }
"#,
    )
    .expect("parse allows raw duplicates until resolve");
    let err = resolve_package_catalog_layers(&[layer]).expect_err("duplicate identity");
    assert!(
        matches!(err, PackageCatalogError::DuplicateProvider { .. }),
        "unexpected: {err:?}"
    );
}

#[test]
fn later_layer_replaces_same_identity_without_duplicate_error() {
    let base = PackageCatalogLayer::from_toml_str(
        r#"
schema_version = 1

[[packages]]
name = "Lib"
version = "1.0"
source = "a/package.py"
format = "spack"
toolchain = { name = "foss", version = "2026.1" }
profile = "default"
"#,
    )
    .expect("base");
    let overlay = PackageCatalogLayer::from_toml_str(
        r#"
schema_version = 1

[[packages]]
name = "Lib"
version = "1.0"
source = "b/meta.yaml"
format = "conda-forge"
source_checksums = ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"#,
    )
    .expect("overlay");
    let catalog = resolve_package_catalog_layers(&[base, overlay]).expect("merge");
    let provider = catalog.lookup("Lib", Some("1.0")).expect("lookup");
    assert_eq!(provider.format, Some(ForeignFormat::CondaForge));
    assert_eq!(provider.source, PathBuf::from("b/meta.yaml"));
    assert_eq!(provider.profile, "default");
    assert_eq!(provider.source_checksums.len(), 1);
}

#[test]
fn lookup_without_version_fails_when_multiple_versions_exist() {
    let layer = PackageCatalogLayer::from_toml_str(
        r#"
schema_version = 1

[[packages]]
name = "Lib"
version = "1.0"
source = "a.py"
format = "spack"
toolchain = { name = "foss", version = "2026.1" }

[[packages]]
name = "Lib"
version = "2.0"
source = "b.py"
format = "spack"
toolchain = { name = "foss", version = "2026.1" }
"#,
    )
    .expect("catalog");
    let catalog = resolve_package_catalog_layers(&[layer]).expect("resolve");
    let err = catalog.lookup("Lib", None).expect_err("ambiguous versions");
    assert!(matches!(err, PackageCatalogError::AmbiguousProvider { .. }));
    assert!(catalog.lookup("Lib", Some("2.0")).is_ok());
}

#[test]
fn lookup_missing_provider_fails() {
    let catalog = resolve_package_catalog_layers(&[]).expect("empty catalog");
    let err = catalog.lookup("Missing", None).expect_err("missing");
    assert!(matches!(err, PackageCatalogError::MissingProvider { .. }));
}

#[test]
fn resolve_rejects_incomplete_provider() {
    let layer = PackageCatalogLayer::from_toml_str(
        r#"
schema_version = 1

[[packages]]
name = "Lib"
version = "1.0"
# missing source and toolchain
"#,
    )
    .expect("parse incomplete");
    let err = resolve_package_catalog_layers(&[layer]).expect_err("incomplete");
    assert!(
        matches!(
            err,
            PackageCatalogError::IncompleteProvider { .. }
                | PackageCatalogError::MissingSource { .. }
                | PackageCatalogError::MissingToolchain { .. }
        ),
        "unexpected: {err:?}"
    );
}

#[test]
fn catalog_rejects_empty_package_name() {
    let err = PackageCatalogLayer::from_toml_str(
        r#"
schema_version = 1

[[packages]]
name = "  "
source = "x.py"
format = "spack"
toolchain = { name = "foss", version = "2026.1" }
"#,
    )
    .expect_err("empty name");
    assert!(matches!(err, PackageCatalogError::EmptyPackageName));
}

#[test]
fn catalog_default_profile_is_default_when_omitted() {
    let layer = PackageCatalogLayer::from_toml_str(
        r#"
schema_version = 1

[[packages]]
name = "Lib"
source = "x.py"
format = "conda-forge"
toolchain = { name = "foss", version = "2026.1" }
"#,
    )
    .expect("parse");
    let catalog = resolve_package_catalog_layers(&[layer]).expect("resolve");
    let provider = catalog.lookup("Lib", None).expect("lookup");
    assert_eq!(provider.profile, "default");
    assert!(provider.version.is_none());
}

#[test]
fn catalog_exposes_providers_in_stable_order() {
    let layer = PackageCatalogLayer::from_toml_str(
        r#"
schema_version = 1

[[packages]]
name = "Beta"
source = "b.py"
format = "spack"
toolchain = { name = "foss", version = "2026.1" }

[[packages]]
name = "Alpha"
source = "a.py"
format = "spack"
toolchain = { name = "foss", version = "2026.1" }
"#,
    )
    .expect("parse");
    let catalog = resolve_package_catalog_layers(&[layer]).expect("resolve");
    let names: Vec<_> = catalog
        .providers()
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    // Insertion order preserved (not forced alpha sort) — workflow walks deterministically.
    assert_eq!(names, vec!["Beta", "Alpha"]);
}

#[test]
fn from_path_io_error_is_typed() {
    let err = PackageCatalogLayer::from_path(Path::new("/no/such/package-catalog.toml"))
        .expect_err("missing file");
    assert!(matches!(err, PackageCatalogError::Io(_, _)));
}

#[test]
fn catalog_accepts_auto_format_when_format_omitted() {
    let layer = PackageCatalogLayer::from_toml_str(
        r#"
schema_version = 1

[[packages]]
name = "Lib"
source = "recipe.yaml"
toolchain = { name = "foss", version = "2026.1" }
"#,
    )
    .expect("parse");
    let catalog = resolve_package_catalog_layers(&[layer]).expect("resolve");
    let provider = catalog.lookup("Lib", None).expect("lookup");
    assert_eq!(provider.format, None);
}
