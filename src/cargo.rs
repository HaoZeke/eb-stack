//! Offline Cargo.toml / crates.io adapter.
//!
//! Same leftover model as PyPI: existing robot `Rust` / `maturin` modules
//! are provides. A crate that is not in the robot becomes its own recipe
//! (`PythonPackage` when it binds Python via PyO3/maturin, otherwise
//! `Crate`). The adapter never invents a SHA-256.

use crate::ecosystem::nonempty;
use crate::foreign::{
    ForeignDep, ForeignError, ForeignFormat, ForeignRecipe, ForeignResidual, ForeignSource,
};
use crate::package::{ConditionExpr, ResidualSeverity};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

/// Parse a Cargo.toml document or a crates.io API JSON document.
pub fn parse_cargo_str(text: &str) -> Result<ForeignRecipe, ForeignError> {
    let trimmed = text.trim();
    if trimmed.starts_with('{') {
        parse_crates_io_json(trimmed)
    } else {
        parse_cargo_toml(trimmed)
    }
}

fn parse_cargo_toml(text: &str) -> Result<ForeignRecipe, ForeignError> {
    let value: toml::Value = toml::from_str(text)
        .map_err(|error| ForeignError::Parse(format!("cargo toml: {error}")))?;
    let package = value
        .get("package")
        .ok_or_else(|| ForeignError::Parse("cargo toml missing [package]".into()))?;
    let name = toml_string(package, "name")
        .ok_or_else(|| ForeignError::Parse("cargo toml missing package.name".into()))?;
    let version = toml_string(package, "version").ok_or_else(|| {
        ForeignError::Parse("cargo toml package.version is missing or workspace-inherited".into())
    })?;
    let homepage = toml_string(package, "homepage").or_else(|| toml_string(package, "repository"));
    let summary = toml_string(package, "description");
    let license = toml_string(package, "license");
    let python = is_python_crate(&value);
    recipe(CrateFields {
        raw_name: name,
        version,
        homepage,
        summary,
        license,
        source_url: None,
        source_filename: None,
        sha256: None,
        python,
        crate_deps: cargo_deps(&value),
        note: "parsed from Cargo.toml",
    })
}

fn parse_crates_io_json(text: &str) -> Result<ForeignRecipe, ForeignError> {
    let value: Value = serde_json::from_str(text)
        .map_err(|error| ForeignError::Parse(format!("crates.io json: {error}")))?;
    let doc: CratesIoDocument = serde_json::from_value(value.clone())
        .map_err(|error| ForeignError::Parse(format!("crates.io json: {error}")))?;
    let version = doc
        .versions
        .iter()
        .find(|entry| {
            entry.num == doc.krate.max_stable_version || entry.num == doc.krate.max_version
        })
        .or_else(|| doc.versions.first())
        .ok_or_else(|| ForeignError::Parse("crates.io json has no versions".into()))?;
    let url = nonempty(version.url.clone()).unwrap_or_else(|| {
        format!(
            "https://static.crates.io/crates/{}/{}-{}.crate",
            doc.krate.name, doc.krate.name, version.num
        )
    });
    let filename = nonempty(version.filename.clone())
        .unwrap_or_else(|| format!("{}-{}.crate", doc.krate.name, version.num));
    let python = doc.versions.iter().any(|entry| {
        entry
            .features
            .keys()
            .any(|feature| feature.contains("pyo3") || feature.contains("python"))
    });
    recipe(CrateFields {
        raw_name: doc.krate.name,
        version: version.num.clone(),
        homepage: nonempty(doc.krate.homepage).or_else(|| nonempty(doc.krate.repository)),
        summary: nonempty(doc.krate.description),
        license: None,
        source_url: Some(url),
        source_filename: Some(filename),
        sha256: nonempty(version.checksum.clone()),
        python,
        crate_deps: Vec::new(),
        note: "parsed from crates.io JSON",
    })
}

#[derive(Debug, Deserialize)]
struct CratesIoDocument {
    #[serde(rename = "crate")]
    krate: CratesIoCrate,
    #[serde(default)]
    versions: Vec<CratesIoVersion>,
}

#[derive(Debug, Deserialize)]
struct CratesIoCrate {
    name: String,
    #[serde(default)]
    #[allow(dead_code)]
    id: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    homepage: Option<String>,
    #[serde(default)]
    repository: Option<String>,
    #[serde(default)]
    max_version: String,
    #[serde(default)]
    max_stable_version: String,
}

#[derive(Debug, Deserialize)]
struct CratesIoVersion {
    num: String,
    #[serde(default)]
    checksum: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    filename: Option<String>,
    #[serde(default)]
    features: BTreeMap<String, Vec<String>>,
}

/// One crate's metadata, from Cargo.toml or from the crates.io index.
///
/// The two inputs describe the same thing with different fields present, so
/// they meet here rather than in an eleven-argument call whose order is the
/// only thing keeping the strings apart.
struct CrateFields<'a> {
    raw_name: String,
    version: String,
    homepage: Option<String>,
    summary: Option<String>,
    license: Option<String>,
    source_url: Option<String>,
    source_filename: Option<String>,
    sha256: Option<String>,
    python: bool,
    crate_deps: Vec<CargoDep>,
    note: &'a str,
}

fn recipe(fields: CrateFields<'_>) -> Result<ForeignRecipe, ForeignError> {
    let CrateFields {
        raw_name,
        version,
        homepage,
        summary,
        license,
        source_url,
        source_filename,
        sha256,
        python,
        crate_deps,
        note,
    } = fields;
    let mut residuals = Vec::new();
    // A PyO3 crate publishes under a crate name and imports under a module
    // name, and the mapping is a packaging decision rather than something the
    // manifest states, so it comes from policy data instead of a name branch
    // here.
    let module_name = python
        .then(|| crate::provides::python_module_for_crate(&raw_name))
        .flatten();
    let name = if let Some(module_name) = module_name {
        residuals.push(ForeignResidual {
            category: "cargo-python-name".into(),
            severity: ResidualSeverity::Judgment,
            summary: format!("PyO3 crate {raw_name} is the Python module {module_name}"),
            evidence: Some(raw_name.clone()),
            provenance: None,
        });
        module_name
    } else {
        raw_name
    };
    let mut dependencies = vec![
        ForeignDep {
            name: "Rust".into(),
            pin: None,
            role: "build".into(),
            original_spec: Some("Rust (implicit for Cargo leftovers)".into()),
            condition: ConditionExpr::Always,
            provenance: Vec::new(),
        },
        ForeignDep {
            name: "binutils".into(),
            pin: None,
            role: "build".into(),
            original_spec: Some("binutils (ld for cargo build scripts)".into()),
            condition: ConditionExpr::Always,
            provenance: Vec::new(),
        },
    ];
    if python {
        dependencies.push(ForeignDep {
            name: "Python".into(),
            pin: None,
            role: "run".into(),
            original_spec: Some("Python (implicit for PyO3/maturin crates)".into()),
            condition: ConditionExpr::Always,
            provenance: Vec::new(),
        });
        dependencies.push(ForeignDep {
            name: "maturin".into(),
            pin: None,
            role: "build".into(),
            original_spec: Some("maturin (implicit for PyO3 crates)".into()),
            condition: ConditionExpr::Always,
            provenance: Vec::new(),
        });
    }
    // Crate dependencies are linked out of the crate graph rather than loaded
    // as modules, so they are recorded rather than solved. What they say still
    // matters to a reviewer: the requirement, and whether the crate can build
    // from its published tarball at all.
    for dep in crate_deps {
        let requirement = dep
            .req
            .as_deref()
            .map_or_else(|| "unconstrained".to_string(), |req| format!("`{req}`"));
        let (category, severity, summary) = match dep.kind {
            CargoDepKind::Registry => (
                "cargo-dep",
                ResidualSeverity::Mechanical,
                format!(
                    "Cargo dependency {} {requirement} stays inside the crate graph",
                    dep.name
                ),
            ),
            CargoDepKind::Path => (
                "cargo-path-dep",
                ResidualSeverity::Judgment,
                format!(
                    "Cargo dependency {} is a path dependency {requirement}: it is not in the \
                     published crate, so the released tarball cannot build on its own",
                    dep.name
                ),
            ),
            CargoDepKind::Git => (
                "cargo-git-dep",
                ResidualSeverity::Judgment,
                format!(
                    "Cargo dependency {} is a git dependency {requirement}: the build would \
                     fetch it, which an offline build cannot do",
                    dep.name
                ),
            ),
        };
        residuals.push(ForeignResidual {
            category: category.into(),
            severity,
            summary,
            evidence: Some(dep.name),
            provenance: None,
        });
    }
    let sources = if source_url.is_some() || sha256.is_some() {
        vec![ForeignSource {
            url: source_url.clone(),
            filename: source_filename.clone(),
            sha256: sha256.clone(),
            git: None,
            tag: None,
            commit: None,
            target_directory: None,
            condition: ConditionExpr::Always,
        }]
    } else {
        Vec::new()
    };
    let mut build_system_hints = vec!["cargo".into()];
    if python {
        build_system_hints.extend(["python".into(), "maturin".into(), "pip".into()]);
    } else {
        build_system_hints.push("crate".into());
    }
    Ok(ForeignRecipe {
        format: ForeignFormat::Cargo,
        name,
        version,
        homepage,
        source_url,
        source_filename,
        sha256,
        sources,
        summary,
        description: None,
        license,
        dependencies,
        build_system_hints,
        configopts: None,
        patches: Vec::new(),
        variants: Vec::new(),
        rules: Vec::new(),
        notes: vec![note.into()],
        residuals,
    })
}

fn is_python_crate(value: &toml::Value) -> bool {
    if value
        .get("package")
        .and_then(|package| package.get("metadata"))
        .and_then(|meta| meta.get("maturin"))
        .is_some()
    {
        return true;
    }
    cargo_dep_names(value)
        .iter()
        .any(|name| crate::provides::is_python_marker_crate(name))
}

/// Where a Cargo dependency comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CargoDepKind {
    /// A crates.io release, resolvable from the published tarball.
    Registry,
    /// A sibling directory. Not in the published crate, so the tarball cannot
    /// build on its own.
    Path,
    /// A git revision, which the build would have to fetch.
    Git,
}

/// One Cargo dependency, with the requirement the manifest states.
#[derive(Debug, Clone)]
struct CargoDep {
    name: String,
    /// The requirement, translated into the shared grammar.
    req: Option<String>,
    kind: CargoDepKind,
}

/// Cargo's bare version string is a caret requirement.
///
/// `serde = "1.0"` means `^1.0`, not `==1.0`. An explicit operator is already
/// in the shared grammar and passes through.
fn cargo_version_req(spec: &str) -> Option<String> {
    let spec = spec.trim();
    if spec.is_empty() {
        return None;
    }
    if spec.starts_with(['=', '>', '<', '^', '~']) {
        return Some(spec.to_string());
    }
    if spec.starts_with(|character: char| character.is_ascii_digit()) {
        return Some(format!("^{spec}"));
    }
    // `*` and anything else stated loosely constrains nothing.
    None
}

fn cargo_deps(value: &toml::Value) -> Vec<CargoDep> {
    let mut deps = Vec::new();
    for table in ["dependencies", "build-dependencies", "dev-dependencies"] {
        let Some(map) = value.get(table).and_then(toml::Value::as_table) else {
            continue;
        };
        for (name, spec) in map {
            let dep = match spec {
                toml::Value::String(version) => CargoDep {
                    name: name.clone(),
                    req: cargo_version_req(version),
                    kind: CargoDepKind::Registry,
                },
                toml::Value::Table(entry) => {
                    let kind = if entry.contains_key("path") {
                        CargoDepKind::Path
                    } else if entry.contains_key("git") {
                        CargoDepKind::Git
                    } else {
                        CargoDepKind::Registry
                    };
                    CargoDep {
                        name: name.clone(),
                        req: entry
                            .get("version")
                            .and_then(toml::Value::as_str)
                            .and_then(cargo_version_req),
                        kind,
                    }
                }
                _ => CargoDep {
                    name: name.clone(),
                    req: None,
                    kind: CargoDepKind::Registry,
                },
            };
            deps.push(dep);
        }
    }
    deps
}

fn cargo_dep_names(value: &toml::Value) -> Vec<String> {
    cargo_deps(value).into_iter().map(|dep| dep.name).collect()
}

fn toml_string(value: &toml::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(toml::Value::as_str)
        .map(ToString::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cargo_toml_pyo3_is_python_package() {
        let recipe = parse_cargo_str(
            r#"
[package]
name = "readcon-core"
version = "0.13.1"
description = "CON reader"
license = "MIT"
repository = "https://github.com/lode-org/readcon-core"

[dependencies]
pyo3 = "0.22"
"#,
        )
        .expect("parse");
        assert_eq!(recipe.name, "readcon");
        assert_eq!(recipe.version, "0.13.1");
        assert!(recipe.dependencies.iter().any(|dep| dep.name == "Rust"));
        assert!(recipe.dependencies.iter().any(|dep| dep.name == "Python"));
        assert!(recipe.dependencies.iter().any(|dep| dep.name == "maturin"));
        assert!(recipe
            .build_system_hints
            .iter()
            .any(|hint| hint == "maturin"));
    }

    #[test]
    fn crates_io_json_reads_checksum() {
        let recipe = parse_cargo_str(
            r#"{
              "crate": {
                "id": "demo",
                "name": "demo",
                "max_version": "1.2.3",
                "max_stable_version": "1.2.3",
                "description": "demo crate",
                "repository": "https://example.invalid/demo"
              },
              "versions": [{
                "num": "1.2.3",
                "checksum": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
              }]
            }"#,
        )
        .expect("parse");
        assert_eq!(recipe.name, "demo");
        assert_eq!(recipe.version, "1.2.3");
        assert_eq!(
            recipe.sha256.as_deref(),
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );
        assert!(!recipe.dependencies.iter().any(|dep| dep.name == "Python"));
    }
}
