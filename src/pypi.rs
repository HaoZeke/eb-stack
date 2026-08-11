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
    let value: Value = serde_json::from_str(text)
        .map_err(|error| ForeignError::Parse(format!("pypi json: {error}")))?;
    match value {
        Value::Array(entries) => {
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
            Ok(root)
        }
        Value::Object(_) => recipe_from_warehouse(&value),
        _ => Err(ForeignError::Parse(
            "pypi json must be an object or an array of objects".into(),
        )),
    }
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
    requires_dist: Vec<String>,
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
struct WarehouseDocument {
    info: WarehouseInfo,
    #[serde(default)]
    urls: Vec<WarehouseUrl>,
}

fn recipe_from_warehouse(value: &Value) -> Result<ForeignRecipe, ForeignError> {
    let doc: WarehouseDocument = serde_json::from_value(value.clone())
        .map_err(|error| ForeignError::Parse(format!("pypi warehouse: {error}")))?;
    let mut residuals = Vec::new();
    let mut dependencies = Vec::new();
    for (index, spec) in doc.info.requires_dist.iter().enumerate() {
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
                if let Some(marker) = marker {
                    residuals.push(ForeignResidual {
                        category: "pypi-marker".into(),
                        severity: ResidualSeverity::Judgment,
                        summary: format!("kept {name} with environment marker {marker}"),
                        evidence: Some(original.clone()),
                        provenance: None,
                    });
                }
                dependencies.push(ForeignDep {
                    name,
                    pin,
                    role: "run".into(),
                    original_spec: Some(original),
                    condition: ConditionExpr::Always,
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

    let sdist = doc.urls.iter().find(|url| {
        url.packagetype
            .as_deref()
            .is_some_and(|kind| kind.eq_ignore_ascii_case("sdist"))
    });
    let source = sdist.or_else(|| doc.urls.first());
    let source_url = source.and_then(|url| url.url.clone());
    let source_filename = source.and_then(|url| url.filename.clone());
    let sha256 = source.and_then(|url| url.digests.sha256.clone());
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

    let homepage = nonempty(doc.info.home_page).or_else(|| nonempty(doc.info.project_url));

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
        description: None,
        license: doc.info.license,
        dependencies,
        build_system_hints: vec!["python-bundle".into(), "pip".into()],
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
    let req = req.split_once('[').map(|(name, _)| name).unwrap_or(req);
    let (name, pin) = split_name_and_pin(req);
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
    let mut specs = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('-') {
            continue;
        }
        match parse_pep508(line) {
            Pep508::Requirement {
                name,
                pin,
                original,
                ..
            } => {
                specs.push((name, pin, original));
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
    let Some((name, pin, _)) = specs.first().cloned() else {
        return Err(ForeignError::Parse(
            "requirements.txt has no package specs".into(),
        ));
    };
    let version = pin
        .as_deref()
        .and_then(exact_version)
        .unwrap_or_else(|| "0.0.0".to_string());
    let mut residuals = Vec::new();
    if pin.as_deref().and_then(exact_version).is_none() {
        residuals.push(ForeignResidual {
            category: "pypi-version".into(),
            severity: ResidualSeverity::Judgment,
            summary: format!("root {name} has no exact pin; emitted version {version}"),
            evidence: pin.clone(),
            provenance: None,
        });
    }
    let dependencies = specs
        .into_iter()
        .skip(1)
        .map(|(dep_name, dep_pin, original)| ForeignDep {
            name: dep_name,
            pin: dep_pin,
            role: "run".into(),
            original_spec: Some(original),
            condition: ConditionExpr::Always,
            provenance: Vec::new(),
        })
        .collect();
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
    })
}

fn split_name_and_pin(spec: &str) -> (String, Option<String>) {
    let spec = spec.trim();
    let spec = spec.trim_matches(|c| c == '(' || c == ')');
    let mut cut = spec.len();
    for (index, character) in spec.char_indices() {
        if matches!(character, '<' | '>' | '=' | '!' | '~') {
            cut = index;
            break;
        }
    }
    let name = spec[..cut].trim().to_string();
    let pin = spec[cut..].trim();
    let pin = if pin.is_empty() {
        None
    } else {
        Some(pin.to_string())
    };
    (name, pin)
}

fn exact_version(pin: &str) -> Option<String> {
    let pin = pin.trim();
    pin.strip_prefix("==")
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn nonempty(value: Option<String>) -> Option<String> {
    value.and_then(|text| {
        let text = text.trim();
        (!text.is_empty()).then(|| text.to_string())
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
        assert_eq!(recipe.dependencies.len(), 1);
        assert_eq!(recipe.dependencies[0].name, "soupsieve");
        assert_eq!(recipe.dependencies[0].pin.as_deref(), Some(">=1.6.1"));
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
    fn requirements_txt_uses_first_spec_as_root() {
        let recipe = parse_pypi_str("beautifulsoup4==4.12.3\nsoupsieve==2.6\n").expect("parse");
        assert_eq!(recipe.name, "beautifulsoup4");
        assert_eq!(recipe.version, "4.12.3");
        assert_eq!(recipe.dependencies[0].name, "soupsieve");
        assert_eq!(recipe.dependencies[0].pin.as_deref(), Some("==2.6"));
    }
}
