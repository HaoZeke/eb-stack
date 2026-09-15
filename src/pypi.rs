//! Offline PyPI / requirements.txt adapter.
//!
//! Live Warehouse queries are out of scope for the default test gate. The
//! parser accepts three deterministic inputs:
//!
//! 1. a Warehouse-shaped JSON object (`info` + optional `urls`);
//! 2. a JSON array of those objects (first entry is the root, the rest are
//!    additional site extras that become run dependencies);
//! 3. a requirements.txt of PEP 508 specs (first package is the root).
//!
//! Markers with `extra ==` are dropped (optional extras). Other environment
//! markers become residuals. The adapter never invents a SHA-256.

use crate::ecosystem::{exact_version, nonempty, split_name_and_pin};
use crate::foreign::{
    ForeignDep, ForeignError, ForeignFormat, ForeignRecipe, ForeignResidual, ForeignSource,
};
use crate::package::{ConditionExpr, ResidualSeverity};
use serde::Deserialize;
use serde_json::Value;

/// Parse a PyPI JSON document or a requirements.txt body.
pub fn parse_pypi_str(text: &str) -> Result<ForeignRecipe, ForeignError> {
    let trimmed = text.trim();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        parse_pypi_json(trimmed)
    } else {
        parse_requirements_txt(trimmed)
    }
}

fn parse_pypi_json(text: &str) -> Result<ForeignRecipe, ForeignError> {
    if text.trim_start().starts_with('[') {
        let entries: Vec<Value> = serde_json::from_str(text)
            .map_err(|error| ForeignError::Parse(format!("pypi json: {error}")))?;
        let mut recipes = entries
            .iter()
            .map(recipe_from_warehouse)
            .collect::<Result<Vec<_>, _>>()?;
        if recipes.is_empty() {
            return Err(ForeignError::Parse("pypi json array is empty".into()));
        }
        let mut root = recipes.remove(0);
        for extra in recipes {
            root.dependencies.push(ForeignDep {
                name: extra.name,
                pin: Some(format!("=={}", extra.version)),
                role: "run".into(),
                original_spec: None,
                condition: ConditionExpr::Always,
                provenance: Vec::new(),
            });
        }
        return Ok(root);
    }
    let doc: WarehouseDocument = serde_json::from_str(text)
        .map_err(|error| ForeignError::Parse(format!("pypi warehouse: {error}")))?;
    recipe_from_document(doc)
}

#[derive(Debug, Deserialize)]
struct WarehouseInfo {
    name: String,
    version: String,
    #[serde(default)]
    home_page: Option<String>,
    #[serde(default)]
    project_url: Option<String>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    license: Option<String>,
    #[serde(default)]
    requires_dist: Option<Vec<String>>,
    /// Trove classifiers, which say what the package is for.
    #[serde(default)]
    classifiers: Vec<String>,
    #[serde(default)]
    requires_python: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WarehouseUrl {
    #[serde(default)]
    packagetype: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    filename: Option<String>,
    #[serde(default)]
    digests: WarehouseDigests,
}

#[derive(Debug, Default, Deserialize)]
struct WarehouseDigests {
    #[serde(default)]
    sha256: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WarehouseBuildSystem {
    #[serde(default, rename = "build-backend")]
    build_backend: Option<String>,
    #[serde(default)]
    requires: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct WarehouseDocument {
    info: WarehouseInfo,
    #[serde(default)]
    urls: Vec<WarehouseUrl>,
    #[serde(default)]
    build_system: Option<WarehouseBuildSystem>,
}

fn recipe_from_warehouse(value: &Value) -> Result<ForeignRecipe, ForeignError> {
    let doc: WarehouseDocument = serde_json::from_value(value.clone())
        .map_err(|error| ForeignError::Parse(format!("pypi warehouse: {error}")))?;
    recipe_from_document(doc)
}

fn recipe_from_document(doc: WarehouseDocument) -> Result<ForeignRecipe, ForeignError> {
    let mut residuals = Vec::new();
    let mut dependencies = Vec::new();
    for (index, spec) in doc.info.requires_dist.iter().flatten().enumerate() {
        match parse_pep508(spec) {
            Pep508::SkipExtra { spec } => residuals.push(ForeignResidual {
                category: "pypi-extra".into(),
                severity: ResidualSeverity::Mechanical,
                summary: format!("skipped extra-only requirement {spec}"),
                evidence: Some(spec),
                provenance: None,
            }),
            Pep508::Requirement {
                name,
                pin,
                marker,
                original,
            } => {
                let condition = if let Some(marker) = marker {
                    residuals.push(ForeignResidual {
                        category: "pypi-marker".into(),
                        severity: ResidualSeverity::Judgment,
                        summary: format!(
                            "{name} is gated by environment marker {marker} and does not constrain every profile"
                        ),
                        evidence: Some(original.clone()),
                        provenance: None,
                    });
                    ConditionExpr::Opaque { source: marker }
                } else {
                    ConditionExpr::Always
                };
                // The EasyBuild Python module installs setuptools, pip and
                // wheel itself, so a project that requires one already has it.
                // Emitting a dependency instead sends the solver looking for
                // whatever ships the name, and cppy 1.3.1 ended up depending
                // on ReFrame, which merely carries setuptools as an extension.
                if crate::provides::shipped_with_python(&name) {
                    residuals.push(ForeignResidual {
                        category: "pypi-requirement".into(),
                        severity: ResidualSeverity::Mechanical,
                        summary: format!(
                            "{name} comes with the Python module, so it is not a dependency"
                        ),
                        evidence: Some(original),
                        provenance: None,
                    });
                    continue;
                }
                dependencies.push(ForeignDep {
                    name,
                    pin,
                    role: "run".into(),
                    original_spec: Some(original),
                    condition,
                    provenance: Vec::new(),
                });
            }
            Pep508::Invalid { spec, reason } => residuals.push(ForeignResidual {
                category: "pypi-requirement".into(),
                severity: ResidualSeverity::Judgment,
                summary: format!("could not parse requires_dist[{index}]: {reason}"),
                evidence: Some(spec),
                provenance: None,
            }),
        }
    }
    if let Some(requires_python) = nonempty(doc.info.requires_python.clone()) {
        residuals.push(ForeignResidual {
            category: "pypi-requires-python".into(),
            severity: ResidualSeverity::Judgment,
            summary: format!("requires_python {requires_python} is not encoded as a Python pin"),
            evidence: Some(requires_python),
            provenance: None,
        });
    }

    let sdist = doc.urls.iter().find(|url| {
        url.packagetype
            .as_deref()
            .is_some_and(|kind| kind.eq_ignore_ascii_case("sdist"))
    });
    let source = sdist.or_else(|| doc.urls.first());
    let source_url = source.and_then(|url| url.url.clone());
    let source_filename = source.and_then(|url| url.filename.clone());
    let sha256 = source.and_then(|url| warehouse_sha256(url.digests.sha256.as_deref()));
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

    if let Some(build_system) = &doc.build_system {
        for spec in &build_system.requires {
            match parse_pep508(spec) {
                Pep508::Requirement {
                    name,
                    pin,
                    original,
                    ..
                } => {
                    if crate::provides::ignored_build_requirement(&name) {
                        continue;
                    }
                    if crate::provides::shipped_with_python(&name) {
                        residuals.push(ForeignResidual {
                            category: "pypi-requirement".into(),
                            severity: ResidualSeverity::Mechanical,
                            summary: format!(
                                "{name} comes with the Python module, so it is not a dependency"
                            ),
                            evidence: Some(original),
                            provenance: None,
                        });
                        continue;
                    }
                    dependencies.push(ForeignDep {
                        name,
                        pin,
                        role: "build".into(),
                        original_spec: Some(original),
                        condition: ConditionExpr::Always,
                        provenance: Vec::new(),
                    });
                }
                Pep508::SkipExtra { .. } | Pep508::Invalid { .. } => {}
            }
        }
    }

    let homepage = nonempty(doc.info.home_page).or_else(|| nonempty(doc.info.project_url));
    if !dependencies
        .iter()
        .any(|dep| dep.name.eq_ignore_ascii_case("python"))
    {
        dependencies.insert(
            0,
            ForeignDep {
                name: "Python".into(),
                pin: None,
                role: "run".into(),
                original_spec: Some("Python (implicit for PythonBundle)".into()),
                condition: ConditionExpr::Always,
                provenance: Vec::new(),
            },
        );
    }

    Ok(ForeignRecipe {
        format: ForeignFormat::Pypi,
        name: doc.info.name,
        version: doc.info.version,
        homepage,
        source_url,
        source_filename,
        sha256,
        sources,
        summary: doc.info.summary,
        classifiers: doc.info.classifiers,
        description: None,
        license: doc.info.license,
        dependencies,
        build_system_hints: pypi_build_system_hints(doc.build_system.as_ref()),
        configopts: None,
        patches: Vec::new(),
        variants: Vec::new(),
        rules: Vec::new(),
        notes: vec!["parsed from offline PyPI metadata".into()],
        residuals,
    })
}

enum Pep508 {
    SkipExtra {
        spec: String,
    },
    Requirement {
        name: String,
        pin: Option<String>,
        marker: Option<String>,
        original: String,
    },
    Invalid {
        spec: String,
        reason: String,
    },
}

fn pypi_build_system_hints(build_system: Option<&WarehouseBuildSystem>) -> Vec<String> {
    let mut hints = vec!["python-bundle".into(), "pip".into()];
    let Some(build_system) = build_system else {
        return hints;
    };
    let backend = build_system.build_backend.as_deref().unwrap_or("");
    if backend.contains("meson") {
        hints.extend(["meson".into(), "mesonpy".into()]);
    }
    if build_system
        .requires
        .iter()
        .any(|spec| spec.to_ascii_lowercase().contains("meson"))
        && !hints.iter().any(|hint| hint == "mesonpy")
    {
        hints.extend(["meson".into(), "mesonpy".into()]);
    }
    hints
}

fn warehouse_sha256(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.len() == 64 && value.chars().all(|character| character.is_ascii_hexdigit()) {
        Some(value.to_ascii_lowercase())
    } else {
        None
    }
}

fn strip_inline_comment(line: &str) -> &str {
    let mut in_quote = None;
    for (index, character) in line.char_indices() {
        match (character, in_quote) {
            ('#', None) => return line[..index].trim_end(),
            ('\'' | '"', None) => in_quote = Some(character),
            (quote, Some(open)) if quote == open => in_quote = None,
            _ => {}
        }
    }
    line
}

/// Extras are a name suffix (`pkg[extra]==1.2`), not a terminator.
fn strip_pep508_extras(req: &str) -> String {
    let Some(open) = req.find('[') else {
        return req.to_string();
    };
    let Some(rel) = req[open..].find(']') else {
        return req.to_string();
    };
    let close = open + rel;
    let mut stripped = String::new();
    stripped.push_str(req[..open].trim_end());
    stripped.push_str(req[close + 1..].trim_start());
    stripped
}

fn parse_pep508(spec: &str) -> Pep508 {
    let original = spec.trim().to_string();
    if original.is_empty() {
        return Pep508::Invalid {
            spec: original,
            reason: "empty requirement".into(),
        };
    }
    let (req, marker) = match original.split_once(';') {
        Some((req, marker)) => (req.trim(), Some(marker.trim().to_string())),
        None => (original.as_str(), None),
    };
    if marker
        .as_deref()
        .is_some_and(|marker| marker.to_ascii_lowercase().contains("extra"))
    {
        return Pep508::SkipExtra { spec: original };
    }
    let req = strip_pep508_extras(req);
    if req.contains('@') {
        return Pep508::Invalid {
            spec: original,
            reason: "direct URL/VCS reference is not a named pin".into(),
        };
    }
    let (name, pin) = split_name_and_pin(&req);
    if name.is_empty() {
        return Pep508::Invalid {
            spec: original,
            reason: "missing package name".into(),
        };
    }
    Pep508::Requirement {
        name,
        pin,
        marker,
        original,
    }
}

fn parse_requirements_txt(text: &str) -> Result<ForeignRecipe, ForeignError> {
    let mut residuals = Vec::new();
    let mut specs = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = strip_inline_comment(line.trim());
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('-') {
            residuals.push(ForeignResidual {
                category: "pypi-requirement".into(),
                severity: ResidualSeverity::Judgment,
                summary: format!("requirements.txt:{index}: option {line} is not a package spec"),
                evidence: Some(line.to_string()),
                provenance: None,
            });
            continue;
        }
        match parse_pep508(line) {
            Pep508::Requirement {
                name,
                pin,
                marker,
                original,
            } => {
                specs.push((name, pin, original, marker));
            }
            Pep508::SkipExtra { spec } => {
                return Err(ForeignError::Parse(format!(
                    "requirements.txt:{index}: extra marker not supported: {spec}"
                )));
            }
            Pep508::Invalid { spec, reason } => {
                return Err(ForeignError::Parse(format!(
                    "requirements.txt:{index}: {reason}: {spec}"
                )));
            }
        }
    }
    let Some((name, pin, _, _)) = specs.first().cloned() else {
        return Err(ForeignError::Parse(
            "requirements.txt has no package specs".into(),
        ));
    };
    let version = pin
        .as_deref()
        .and_then(exact_version)
        .unwrap_or_else(|| "0.0.0".to_string());
    if pin.as_deref().and_then(exact_version).is_none() {
        residuals.push(ForeignResidual {
            category: "pypi-version".into(),
            severity: ResidualSeverity::Judgment,
            summary: format!("root {name} has no exact pin; emitted version {version}"),
            evidence: pin.clone(),
            provenance: None,
        });
    }
    let mut dependencies = vec![ForeignDep {
        name: "Python".into(),
        pin: None,
        role: "run".into(),
        original_spec: Some("Python (implicit for PythonBundle)".into()),
        condition: ConditionExpr::Always,
        provenance: Vec::new(),
    }];
    for (dep_name, dep_pin, original, marker) in specs.into_iter().skip(1) {
        if crate::provides::shipped_with_python(&dep_name) {
            residuals.push(ForeignResidual {
                category: "pypi-requirement".into(),
                severity: ResidualSeverity::Mechanical,
                summary: format!(
                    "{dep_name} comes with the Python module, so it is not a dependency"
                ),
                evidence: Some(original),
                provenance: None,
            });
            continue;
        }
        let condition = if let Some(marker) = marker {
            residuals.push(ForeignResidual {
                category: "pypi-marker".into(),
                severity: ResidualSeverity::Judgment,
                summary: format!(
                    "{dep_name} is gated by environment marker {marker} and does not constrain every profile"
                ),
                evidence: Some(original.clone()),
                provenance: None,
            });
            ConditionExpr::Opaque { source: marker }
        } else {
            ConditionExpr::Always
        };
        dependencies.push(ForeignDep {
            name: dep_name,
            pin: dep_pin,
            role: "run".into(),
            original_spec: Some(original),
            condition,
            provenance: Vec::new(),
        });
    }
    Ok(ForeignRecipe {
        format: ForeignFormat::Pypi,
        name,
        version,
        homepage: None,
        source_url: None,
        source_filename: None,
        sha256: None,
        sources: Vec::new(),
        summary: None,
        description: None,
        license: None,
        dependencies,
        build_system_hints: vec!["python-bundle".into(), "pip".into()],
        configopts: None,
        patches: Vec::new(),
        variants: Vec::new(),
        rules: Vec::new(),
        notes: vec!["parsed from requirements.txt".into()],
        residuals,
        classifiers: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warehouse_json_reads_requires_dist() {
        let recipe = parse_pypi_str(
            r#"{
              "info": {
                "name": "beautifulsoup4",
                "version": "4.12.3",
                "home_page": "https://www.crummy.com/software/BeautifulSoup/",
                "summary": "Screen-scraping library",
                "license": "MIT",
                "requires_dist": [
                  "soupsieve>=1.6.1",
                  "lxml ; extra == 'lxml'"
                ]
              },
              "urls": [{
                "packagetype": "sdist",
                "url": "https://files.pythonhosted.org/packages/bs4/beautifulsoup4-4.12.3.tar.gz",
                "filename": "beautifulsoup4-4.12.3.tar.gz",
                "digests": {"sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}
              }]
            }"#,
        )
        .expect("parse");
        assert_eq!(recipe.name, "beautifulsoup4");
        assert_eq!(recipe.version, "4.12.3");
        assert_eq!(recipe.dependencies[0].name, "Python");
        assert_eq!(recipe.dependencies[1].name, "soupsieve");
        assert_eq!(recipe.dependencies[1].pin.as_deref(), Some(">=1.6.1"));
        assert!(recipe
            .residuals
            .iter()
            .any(|residual| residual.category == "pypi-extra"));
        assert_eq!(
            recipe.sha256.as_deref(),
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );
    }

    #[test]
    fn extras_do_not_eat_the_version_pin() {
        let recipe = parse_pypi_str("demo[html]==4.12.3\n").expect("parse");
        assert_eq!(recipe.name, "demo");
        assert_eq!(recipe.version, "4.12.3");
        assert!(
            recipe
                .residuals
                .iter()
                .all(|residual| residual.category != "pypi-version"),
            "{:?}",
            recipe.residuals
        );
        let recipe = parse_pypi_str(
            r#"{
              "info": {
                "name": "app",
                "version": "1.0",
                "requires_dist": ["requests[security]>=2.31.0"]
              },
              "urls": []
            }"#,
        )
        .expect("parse");
        let dep = recipe
            .dependencies
            .iter()
            .find(|dep| dep.name == "requests")
            .expect("requests");
        assert_eq!(dep.pin.as_deref(), Some(">=2.31.0"));
        let err = parse_pypi_str("demo[foo] @ https://example.invalid/demo-1.0.tar.gz\n")
            .expect_err("direct url");
        assert!(
            err.to_string().contains("URL") || err.to_string().contains("direct"),
            "{err}"
        );
    }

    #[test]
    fn warehouse_sha256_must_be_sixty_four_hex() {
        let recipe = parse_pypi_str(
            r#"{
              "info": {"name": "demo", "version": "1.0"},
              "urls": [{
                "packagetype": "sdist",
                "url": "https://example.invalid/demo-1.0.tar.gz",
                "filename": "demo-1.0.tar.gz",
                "digests": {"sha256": "not-a-digest"}
              }]
            }"#,
        )
        .expect("parse");
        assert_eq!(recipe.sha256, None);
        assert!(recipe.sources.iter().all(|source| source.sha256.is_none()));
    }

    #[test]
    fn warehouse_json_accepts_null_requires_dist() {
        let recipe = parse_pypi_str(
            r#"{
              "info": {
                "name": "numpy",
                "version": "2.5.2",
                "summary": "array library",
                "requires_dist": null
              },
              "urls": [{
                "packagetype": "sdist",
                "url": "https://files.pythonhosted.org/packages/numpy/numpy-2.5.2.tar.gz",
                "filename": "numpy-2.5.2.tar.gz",
                "digests": {"sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}
              }]
            }"#,
        )
        .expect("parse");
        assert_eq!(recipe.name, "numpy");
        assert_eq!(recipe.dependencies.len(), 1);
        assert_eq!(recipe.dependencies[0].name, "Python");
    }

    #[test]
    fn requirements_txt_uses_first_spec_as_root() {
        let recipe = parse_pypi_str("beautifulsoup4==4.12.3\nsoupsieve==2.6\n").expect("parse");
        assert_eq!(recipe.name, "beautifulsoup4");
        assert_eq!(recipe.version, "4.12.3");
        assert_eq!(recipe.dependencies[0].name, "Python");
        assert_eq!(recipe.dependencies[1].name, "soupsieve");
        assert_eq!(recipe.dependencies[1].pin.as_deref(), Some("==2.6"));
    }

    #[test]
    fn requirements_txt_strips_inline_comments() {
        let recipe = parse_pypi_str("beautifulsoup4==4.12.3  # latest\n").expect("parse");
        assert_eq!(recipe.version, "4.12.3");
    }

    #[test]
    fn a_direct_url_requirement_is_not_a_package_name() {
        let err = parse_pypi_str("pkg @ https://example.invalid/pkg-1.0.tar.gz\n")
            .expect_err("direct ref");
        assert!(err.to_string().contains("direct URL"), "{err}");
    }

    #[test]
    fn requirements_txt_skips_python_shipped_names() {
        let recipe = parse_pypi_str("demo==1.0\nsetuptools==69.0.3\npip==24.0\n").expect("parse");
        assert!(
            recipe
                .dependencies
                .iter()
                .all(|dep| !crate::provides::shipped_with_python(&dep.name)),
            "{:?}",
            recipe.dependencies
        );
        assert!(
            recipe.residuals.iter().any(|residual| {
                residual.category == "pypi-requirement" && residual.summary.contains("setuptools")
            }),
            "{:?}",
            recipe.residuals
        );
    }

    #[test]
    fn requirements_txt_platform_marker_is_opaque() {
        let recipe =
            parse_pypi_str("demo==1.0\npywin32>=1.0; sys_platform == 'win32'\n").expect("parse");
        let pywin = recipe
            .dependencies
            .iter()
            .find(|dep| dep.name == "pywin32")
            .expect("pywin32");
        assert!(
            matches!(pywin.condition, ConditionExpr::Opaque { .. }),
            "{pywin:?}"
        );
        assert!(
            recipe
                .residuals
                .iter()
                .any(|residual| residual.category == "pypi-marker"),
            "{:?}",
            recipe.residuals
        );
    }

    #[test]
    fn a_platform_marker_is_opaque_not_always() {
        let recipe = parse_pypi_str(
            r#"{
              "info": {
                "name": "demo",
                "version": "1.0",
                "requires_dist": ["pywin32>=1.0; sys_platform == 'win32'"],
                "requires_python": ">=3.9"
              },
              "urls": []
            }"#,
        )
        .expect("parse");
        let pywin = recipe
            .dependencies
            .iter()
            .find(|dep| dep.name == "pywin32")
            .expect("pywin32");
        assert!(matches!(pywin.condition, ConditionExpr::Opaque { .. }));
        assert!(recipe
            .residuals
            .iter()
            .any(|residual| residual.category == "pypi-requires-python"));
    }

    #[test]
    fn warehouse_array_extra_is_a_run_dep() {
        let recipe = parse_pypi_str(
            r#"[
              {
                "info": {
                  "name": "eon-akmc",
                  "version": "3.1.0",
                  "requires_dist": ["numpy>=1.26.4"]
                },
                "urls": []
              },
              {
                "info": {
                  "name": "PyYAML",
                  "version": "6.0.2",
                  "requires_dist": []
                },
                "urls": []
              }
            ]"#,
        )
        .expect("parse");
        assert_eq!(recipe.name, "eon-akmc");
        assert!(
            recipe.dependencies.iter().any(|dep| dep.name == "PyYAML"
                && dep.pin.as_deref() == Some("==6.0.2")
                && dep.role == "run"),
            "{:?}",
            recipe.dependencies
        );
    }

    #[test]
    fn warehouse_build_system_adds_mesonpy_hint_and_build_dep() {
        let recipe = parse_pypi_str(
            r#"{
              "info": {
                "name": "eon-akmc",
                "version": "3.1.0",
                "requires_dist": []
              },
              "build_system": {
                "build-backend": "mesonpy",
                "requires": ["setuptools>=61", "meson-python", "numpy"]
              },
              "urls": []
            }"#,
        )
        .expect("parse");
        assert!(
            recipe
                .build_system_hints
                .iter()
                .any(|hint| hint == "mesonpy"),
            "{:?}",
            recipe.build_system_hints
        );
        assert!(
            recipe
                .dependencies
                .iter()
                .any(|dep| dep.name == "meson-python" && dep.role == "build"),
            "{:?}",
            recipe.dependencies
        );
        assert!(
            !recipe
                .dependencies
                .iter()
                .any(|dep| dep.name.eq_ignore_ascii_case("numpy") && dep.role == "build"),
            "numpy stays a robot provide, not a build extra: {:?}",
            recipe.dependencies
        );
        assert!(
            !recipe
                .dependencies
                .iter()
                .any(|dep| dep.name.eq_ignore_ascii_case("setuptools")),
            "setuptools is shipped with Python: {:?}",
            recipe.dependencies
        );
    }
}
