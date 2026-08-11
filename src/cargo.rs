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
        crate_deps: cargo_dep_names(&value),
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
    crate_deps: Vec<String>,
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
    let name = if python && raw_name.eq_ignore_ascii_case("readcon-core") {
        residuals.push(ForeignResidual {
            category: "cargo-python-name".into(),
            severity: ResidualSeverity::Judgment,
            summary: "PyO3 crate readcon-core is the Python module readcon".into(),
            evidence: Some(raw_name.clone()),
            provenance: None,
        });
        "readcon".into()
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
    for dep in crate_deps {
        residuals.push(ForeignResidual {
            category: "cargo-dep".into(),
            severity: ResidualSeverity::Mechanical,
            summary: format!("Cargo dependency {dep} stays inside the crate graph"),
            evidence: Some(dep),
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
        .any(|name| matches!(name.as_str(), "pyo3" | "maturin" | "pyo3-ffi" | "numpy"))
}

fn cargo_dep_names(value: &toml::Value) -> Vec<String> {
    let mut names = Vec::new();
    for table in ["dependencies", "build-dependencies", "dev-dependencies"] {
        if let Some(map) = value.get(table).and_then(toml::Value::as_table) {
            names.extend(map.keys().cloned());
        }
    }
    names
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
