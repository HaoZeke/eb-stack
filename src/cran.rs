//! Offline CRAN DESCRIPTION / package-list adapter.
//!
//! Default tests never fetch the CRAN PACKAGES index. The parser accepts:
//!
//! 1. a DESCRIPTION file (`Package:`, `Version:`, `Depends:`, `Imports:`);
//! 2. a JSON object with the same fields;
//! 3. a package list (`jsonlite==1.8.8`, one spec per line).
//!
//! Base-R packages are dropped. `R (>= x)` becomes a dependency on EasyBuild
//! `R` with the version constraint preserved.

use crate::ecosystem::{exact_version, split_name_and_pin};

/// Where CRAN publishes every source release.
const CRAN_CONTRIB: &str = "https://cran.r-project.org/src/contrib";
use crate::foreign::{
    ForeignDep, ForeignError, ForeignFormat, ForeignRecipe, ForeignResidual, ForeignSource,
};
use crate::package::{ConditionExpr, ResidualSeverity};
use serde::Deserialize;
use std::collections::BTreeMap;

/// Parse a DESCRIPTION file, CRAN JSON object, or package-list body.
pub fn parse_cran_str(text: &str) -> Result<ForeignRecipe, ForeignError> {
    let trimmed = text.trim();
    if trimmed.starts_with('{') {
        parse_cran_json(trimmed)
    } else if looks_like_description(trimmed) {
        parse_description(trimmed)
    } else {
        parse_package_list(trimmed)
    }
}

fn looks_like_description(text: &str) -> bool {
    text.lines().any(|line| {
        let lower = line.to_ascii_lowercase();
        lower.starts_with("package:") || lower.starts_with("version:")
    })
}

#[derive(Debug, Deserialize)]
struct CranJson {
    #[serde(alias = "Package")]
    package: String,
    #[serde(alias = "Version")]
    version: String,
    #[serde(default, alias = "Title")]
    title: Option<String>,
    #[serde(default, alias = "Description")]
    description: Option<String>,
    #[serde(default, alias = "License")]
    license: Option<String>,
    #[serde(default, alias = "URL")]
    url: Option<String>,
    #[serde(default, alias = "Depends")]
    depends: Vec<String>,
    #[serde(default, alias = "Imports")]
    imports: Vec<String>,
    #[serde(default, alias = "LinkingTo")]
    linking_to: Vec<String>,
}

fn parse_cran_json(text: &str) -> Result<ForeignRecipe, ForeignError> {
    let doc: CranJson = serde_json::from_str(text)
        .map_err(|error| ForeignError::Parse(format!("cran json: {error}")))?;
    recipe_from_fields(CranFields {
        name: doc.package,
        version: doc.version,
        title: doc.title,
        description: doc.description,
        license: doc.license,
        url: doc.url,
        depends: &doc.depends,
        imports: &doc.imports,
        linking_to: &doc.linking_to,
        note: "parsed from CRAN JSON",
    })
}

fn parse_description(text: &str) -> Result<ForeignRecipe, ForeignError> {
    let fields = parse_debian_control(text);
    let package = fields
        .get("package")
        .cloned()
        .ok_or_else(|| ForeignError::Parse("DESCRIPTION is missing Package".into()))?;
    let version = fields
        .get("version")
        .cloned()
        .ok_or_else(|| ForeignError::Parse("DESCRIPTION is missing Version".into()))?;
    recipe_from_fields(CranFields {
        name: package,
        version,
        title: fields.get("title").cloned(),
        description: fields.get("description").cloned(),
        license: fields.get("license").cloned(),
        url: fields.get("url").cloned(),
        depends: &split_r_list(fields.get("depends").map(String::as_str).unwrap_or("")),
        imports: &split_r_list(fields.get("imports").map(String::as_str).unwrap_or("")),
        linking_to: &split_r_list(fields.get("linkingto").map(String::as_str).unwrap_or("")),
        note: "parsed from DESCRIPTION",
    })
}

fn parse_package_list(text: &str) -> Result<ForeignRecipe, ForeignError> {
    let mut specs = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (name, pin) = split_name_and_pin(line);
        if name.is_empty() {
            return Err(ForeignError::Parse(format!(
                "package list:{index}: missing package name"
            )));
        }
        specs.push((name, pin));
    }
    let Some((name, pin)) = specs.first().cloned() else {
        return Err(ForeignError::Parse(
            "package list has no package specs".into(),
        ));
    };
    let version = pin
        .as_deref()
        .and_then(exact_version)
        .unwrap_or_else(|| "0.0.0".to_string());
    let extras: Vec<String> = specs
        .into_iter()
        .skip(1)
        .map(|(dep_name, dep_pin)| match dep_pin {
            Some(pin) => format!("{dep_name} ({pin})"),
            None => dep_name,
        })
        .collect();
    recipe_from_fields(CranFields {
        name,
        version,
        title: None,
        description: None,
        license: None,
        url: None,
        depends: &[],
        imports: &extras,
        linking_to: &[],
        note: "parsed from CRAN package list",
    })
}

/// One CRAN package's metadata, however it was written down.
///
/// DESCRIPTION, the CRAN JSON index and a bare package list all reduce to
/// these fields, so they travel together rather than as ten positional
/// arguments that only the call order keeps aligned.
struct CranFields<'a> {
    name: String,
    version: String,
    title: Option<String>,
    description: Option<String>,
    license: Option<String>,
    url: Option<String>,
    depends: &'a [String],
    imports: &'a [String],
    linking_to: &'a [String],
    note: &'a str,
}

fn recipe_from_fields(fields: CranFields<'_>) -> Result<ForeignRecipe, ForeignError> {
    let CranFields {
        name,
        version,
        title,
        description,
        license,
        url,
        depends,
        imports,
        linking_to,
        note,
    } = fields;
    let mut residuals = Vec::new();
    let mut dependencies = vec![ForeignDep {
        name: "R".into(),
        pin: None,
        role: "run".into(),
        original_spec: Some("R (implicit for RPackage)".into()),
        condition: ConditionExpr::Always,
        provenance: Vec::new(),
    }];
    for (role, entries) in [("run", depends), ("run", imports), ("build", linking_to)] {
        for entry in entries {
            match parse_r_dep(entry) {
                RDep::SkipBase { name } => residuals.push(ForeignResidual {
                    category: "cran-base".into(),
                    severity: ResidualSeverity::Mechanical,
                    summary: format!("skipped base-R package {name}"),
                    evidence: Some(entry.clone()),
                    provenance: None,
                }),
                RDep::Requirement { name, pin } => dependencies.push(ForeignDep {
                    name,
                    pin,
                    role: role.into(),
                    original_spec: Some(entry.clone()),
                    condition: ConditionExpr::Always,
                    provenance: Vec::new(),
                }),
                RDep::Invalid { reason } => residuals.push(ForeignResidual {
                    category: "cran-requirement".into(),
                    severity: ResidualSeverity::Judgment,
                    summary: reason,
                    evidence: Some(entry.clone()),
                    provenance: None,
                }),
            }
        }
    }

    // DESCRIPTION's URL field is the project's home page, which is usually a
    // repository or a documentation site and is not where the tarball lives.
    // CRAN publishes every release at one predictable location, so the source
    // comes from there and URL stays the homepage it is.
    let homepage = url.as_ref().and_then(|value| {
        value
            .split([',', ' '])
            .map(str::trim)
            .find(|item| item.starts_with("http"))
            .map(ToString::to_string)
    });
    let source_url = format!("{CRAN_CONTRIB}/{name}_{version}.tar.gz");
    let sources = vec![ForeignSource {
        url: Some(source_url.clone()),
        filename: Some(format!("{name}_{version}.tar.gz")),
        sha256: None,
        git: None,
        tag: None,
        commit: None,
        target_directory: None,
        condition: ConditionExpr::Always,
    }];

    Ok(ForeignRecipe {
        format: ForeignFormat::Cran,
        name,
        version,
        homepage: homepage.or_else(|| Some(CRAN_CONTRIB.to_string())),
        source_url: Some(source_url),
        source_filename: None,
        sha256: None,
        sources,
        summary: title,
        description,
        license,
        dependencies,
        build_system_hints: vec!["r-bundle".into(), "cran".into()],
        configopts: None,
        patches: Vec::new(),
        variants: Vec::new(),
        rules: Vec::new(),
        notes: vec![note.into()],
        residuals,
    })
}

enum RDep {
    SkipBase { name: String },
    Requirement { name: String, pin: Option<String> },
    Invalid { reason: String },
}

fn parse_r_dep(entry: &str) -> RDep {
    let entry = entry.trim();
    if entry.is_empty() {
        return RDep::Invalid {
            reason: "empty R dependency".into(),
        };
    }
    let (name_part, pin) = if let Some((name, rest)) = entry.split_once('(') {
        let pin = rest.trim().trim_end_matches(')').trim();
        (name.trim(), Some(pin.to_string()))
    } else {
        (entry, None)
    };
    if name_part.is_empty() {
        return RDep::Invalid {
            reason: "missing R package name".into(),
        };
    }
    if is_base_r(name_part) {
        return RDep::SkipBase {
            name: name_part.to_string(),
        };
    }
    RDep::Requirement {
        name: name_part.to_string(),
        pin,
    }
}

fn is_base_r(name: &str) -> bool {
    matches!(
        name,
        "base"
            | "compiler"
            | "datasets"
            | "graphics"
            | "grDevices"
            | "grid"
            | "methods"
            | "parallel"
            | "splines"
            | "stats"
            | "stats4"
            | "tcltk"
            | "tools"
            | "translations"
            | "utils"
    )
}

fn parse_debian_control(text: &str) -> BTreeMap<String, String> {
    let mut fields = BTreeMap::new();
    let mut current_key = None;
    let mut current_value = String::new();
    let flush = |fields: &mut BTreeMap<String, String>, key: &Option<String>, value: &str| {
        if let Some(key) = key {
            fields.insert(key.clone(), value.trim().to_string());
        }
    };
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix([' ', '\t']) {
            if !current_value.is_empty() {
                current_value.push(' ');
            }
            current_value.push_str(rest.trim());
            continue;
        }
        flush(&mut fields, &current_key, &current_value);
        current_key = None;
        current_value.clear();
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        current_key = Some(key.trim().to_ascii_lowercase().replace('-', ""));
        current_value = value.trim().to_string();
    }
    flush(&mut fields, &current_key, &current_value);
    fields
}

fn split_r_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToString::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn description_skips_base_r_and_keeps_imports() {
        let recipe = parse_cran_str(
            "Package: jsonlite\n\
             Version: 1.8.8\n\
             Title: JSON Parser\n\
             Depends: methods, R (>= 3.1.0)\n\
             Imports: somethingelse\n\
             License: MIT\n\
             URL: https://arxiv.org/abs/1403.2805\n",
        )
        .expect("parse");
        assert_eq!(recipe.name, "jsonlite");
        assert_eq!(recipe.version, "1.8.8");
        assert!(recipe
            .dependencies
            .iter()
            .any(|dep| dep.name == "R" && dep.pin.as_deref() == Some(">= 3.1.0")));
        assert!(recipe
            .dependencies
            .iter()
            .any(|dep| dep.name == "somethingelse"));
        assert!(recipe
            .residuals
            .iter()
            .any(|residual| residual.summary.contains("methods")));
        assert!(!recipe.dependencies.iter().any(|dep| dep.name == "methods"));
    }

    #[test]
    fn package_list_uses_first_spec_as_root() {
        let recipe = parse_cran_str("jsonlite==1.8.8\ncurl==5.2.1\n").expect("parse");
        assert_eq!(recipe.name, "jsonlite");
        assert_eq!(recipe.version, "1.8.8");
        assert_eq!(recipe.dependencies[0].name, "R");
        assert_eq!(recipe.dependencies[1].name, "curl");
    }
}
