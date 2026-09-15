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
use serde::de::{self, Deserializer, Visitor};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt;

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
    #[serde(default, alias = "Depends", deserialize_with = "deserialize_r_list")]
    depends: Vec<String>,
    #[serde(default, alias = "Imports", deserialize_with = "deserialize_r_list")]
    imports: Vec<String>,
    #[serde(default, alias = "LinkingTo", deserialize_with = "deserialize_r_list")]
    linking_to: Vec<String>,
    #[serde(default, alias = "SystemRequirements")]
    system_requirements: Option<String>,
}

fn deserialize_r_list<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    struct RListVisitor;
    impl<'de> Visitor<'de> for RListVisitor {
        type Value = Vec<String>;
        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a Depends/Imports/LinkingTo string, array, or object")
        }
        fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
            Ok(split_r_list(value))
        }
        fn visit_string<E: de::Error>(self, value: String) -> Result<Self::Value, E> {
            Ok(split_r_list(&value))
        }
        fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut items = Vec::new();
            while let Some(item) = seq.next_element::<String>()? {
                if !item.trim().is_empty() {
                    items.push(item);
                }
            }
            Ok(items)
        }
        fn visit_map<A: de::MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut items = Vec::new();
            while let Some((name, pin)) = map.next_entry::<String, serde_json::Value>()? {
                match pin {
                    serde_json::Value::Null => items.push(name),
                    serde_json::Value::String(pin) if pin.is_empty() => items.push(name),
                    serde_json::Value::String(pin) => items.push(format!("{name} ({pin})")),
                    other => items.push(format!("{name} ({other})")),
                }
            }
            Ok(items)
        }
        fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(Vec::new())
        }
        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(Vec::new())
        }
    }
    deserializer.deserialize_any(RListVisitor)
}

fn parse_cran_json(text: &str) -> Result<ForeignRecipe, ForeignError> {
    let doc: CranJson = serde_json::from_str(text)
        .map_err(|error| ForeignError::Parse(format!("cran json: {error}")))?;
    let mut recipe = recipe_from_fields(CranFields {
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
    })?;
    record_system_requirements(&mut recipe, doc.system_requirements.as_deref());
    Ok(recipe)
}

fn parse_description(text: &str) -> Result<ForeignRecipe, ForeignError> {
    let stanza_count = debian_package_stanza_count(text);
    if stanza_count > 1 {
        return Err(ForeignError::Parse(format!(
            "PACKAGES index has {stanza_count} stanzas; pass one DESCRIPTION or a package list"
        )));
    }
    let fields = parse_debian_control(text);
    let package = fields
        .get("package")
        .cloned()
        .ok_or_else(|| ForeignError::Parse("DESCRIPTION is missing Package".into()))?;
    let version = fields
        .get("version")
        .cloned()
        .ok_or_else(|| ForeignError::Parse("DESCRIPTION is missing Version".into()))?;
    let mut recipe = recipe_from_fields(CranFields {
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
    })?;
    record_system_requirements(
        &mut recipe,
        fields.get("systemrequirements").map(String::as_str),
    );
    Ok(recipe)
}

fn record_system_requirements(recipe: &mut ForeignRecipe, sysreq: Option<&str>) {
    let Some(sysreq) = sysreq.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };
    recipe.residuals.push(ForeignResidual {
        category: "cran-system-requirements".into(),
        severity: ResidualSeverity::Judgment,
        summary: format!("SystemRequirements not encoded: {sysreq}"),
        evidence: Some(sysreq.to_string()),
        provenance: None,
    });
}

fn parse_package_list(text: &str) -> Result<ForeignRecipe, ForeignError> {
    let mut specs = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = strip_inline_comment(line.trim());
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
    let mut recipe = recipe_from_fields(CranFields {
        name: name.clone(),
        version: version.clone(),
        title: None,
        description: None,
        license: None,
        url: None,
        depends: &[],
        imports: &extras,
        linking_to: &[],
        note: "parsed from CRAN package list",
    })?;
    if pin.as_deref().and_then(exact_version).is_none() {
        recipe.residuals.push(ForeignResidual {
            category: "cran-version".into(),
            severity: ResidualSeverity::Judgment,
            summary: format!("root {name} has no exact pin; emitted version {version}"),
            evidence: pin.clone(),
            provenance: None,
        });
    }
    Ok(recipe)
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
    let mut dependencies = Vec::new();
    for (role, entries) in [("run", depends), ("run", imports), ("run", linking_to)] {
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
    if !dependencies
        .iter()
        .any(|dep| dep.name.eq_ignore_ascii_case("R"))
    {
        dependencies.insert(
            0,
            ForeignDep {
                name: "R".into(),
                pin: None,
                role: "run".into(),
                original_spec: Some("R (implicit for RPackage)".into()),
                condition: ConditionExpr::Always,
                provenance: Vec::new(),
            },
        );
    }

    // DESCRIPTION's URL field is the project's home page, which is usually a
    // repository or a documentation site and is not where the tarball lives.
    // The current CRAN release lives at contrib/; older ones live under
    // Archive/{name}/. The emitter lists both source_urls so EasyBuild can
    // try each. URL stays the homepage it is.
    let homepage = url
        .as_ref()
        .and_then(|value| first_http_url(value))
        .or_else(|| Some(format!("https://cran.r-project.org/package={name}")));
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
        homepage,
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
        classifiers: Vec::new(),
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

fn first_http_url(value: &str) -> Option<String> {
    value
        .split(|character: char| character == ',' || character.is_whitespace())
        .map(str::trim)
        .find(|item| item.starts_with("http"))
        .map(ToString::to_string)
}

fn debian_package_stanza_count(text: &str) -> usize {
    let mut count = 0;
    let mut has_package = false;
    for line in text.lines() {
        if line.trim().is_empty() {
            if has_package {
                count += 1;
                has_package = false;
            }
            continue;
        }
        if line.to_ascii_lowercase().starts_with("package:") {
            has_package = true;
        }
    }
    if has_package {
        count += 1;
    }
    count
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
    let mut items = Vec::new();
    let mut current = String::new();
    let mut depth = 0u32;
    for ch in value.chars() {
        match ch {
            '(' => {
                depth = depth.saturating_add(1);
                current.push(ch);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                current.push(ch);
            }
            ',' if depth == 0 => {
                let item = current.trim();
                if !item.is_empty() {
                    items.push(item.to_string());
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    let item = current.trim();
    if !item.is_empty() {
        items.push(item.to_string());
    }
    items
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
        let r_deps: Vec<_> = recipe
            .dependencies
            .iter()
            .filter(|dep| dep.name == "R")
            .collect();
        assert_eq!(
            r_deps.len(),
            1,
            "Depends R replaces the implicit unpinned R"
        );
        assert_eq!(r_deps[0].pin.as_deref(), Some(">= 3.1.0"));
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
    fn split_r_list_keeps_commas_inside_version_pins() {
        assert_eq!(
            split_r_list("R (>= 3.1.0, < 4.0), jsonlite"),
            ["R (>= 3.1.0, < 4.0)", "jsonlite"]
        );
        let recipe = parse_cran_str(
            "Package: demo\n\
             Version: 1.0\n\
             Depends: R (>= 3.1.0, < 4.0), jsonlite\n",
        )
        .expect("parse");
        let r = recipe
            .dependencies
            .iter()
            .find(|dep| dep.name == "R")
            .expect("R");
        assert_eq!(r.pin.as_deref(), Some(">= 3.1.0, < 4.0"));
        assert!(recipe.dependencies.iter().any(|dep| dep.name == "jsonlite"));
        assert!(!recipe
            .dependencies
            .iter()
            .any(|dep| dep.name.starts_with('<')));
    }

    #[test]
    fn package_list_without_exact_pin_records_a_judgment_residual() {
        let recipe = parse_cran_str("jsonlite>=1.8\n").expect("parse");
        assert_eq!(recipe.version, "0.0.0");
        assert!(
            recipe.residuals.iter().any(|residual| {
                residual.category == "cran-version"
                    && residual.severity == ResidualSeverity::Judgment
                    && residual.summary.contains("jsonlite")
            }),
            "unpinned root must not silently invent 0.0.0: {:?}",
            recipe.residuals
        );
    }

    #[test]
    fn package_list_strips_an_inline_hash_comment() {
        let recipe = parse_cran_str("jsonlite==1.8.8 # pinned\n").expect("parse");
        assert_eq!(recipe.version, "1.8.8");
        assert!(
            recipe
                .source_url
                .as_deref()
                .is_some_and(|url| url.ends_with("jsonlite_1.8.8.tar.gz") && !url.contains('#')),
            "inline comment leaked into the tarball name: {:?}",
            recipe.source_url
        );
    }

    #[test]
    fn package_list_uses_first_spec_as_root() {
        let recipe = parse_cran_str("jsonlite==1.8.8\ncurl==5.2.1\n").expect("parse");
        assert_eq!(recipe.name, "jsonlite");
        assert_eq!(recipe.version, "1.8.8");
        assert_eq!(recipe.dependencies[0].name, "R");
        assert_eq!(recipe.dependencies[1].name, "curl");
        assert_eq!(
            recipe.homepage.as_deref(),
            Some("https://cran.r-project.org/package=jsonlite")
        );
    }

    #[test]
    fn homepage_splits_on_newlines_and_does_not_use_contrib() {
        let recipe = parse_cran_str(
            "{\n  \"Package\": \"jsonlite\",\n  \"Version\": \"1.8.8\",\n  \
             \"URL\": \"https://jeroen.r-universe.dev/jsonlite\\nhttps://arxiv.org/abs/1403.2805\"\n}",
        )
        .expect("parse");
        assert_eq!(
            recipe.homepage.as_deref(),
            Some("https://jeroen.r-universe.dev/jsonlite")
        );
    }

    #[test]
    fn linking_to_is_a_run_dependency() {
        let recipe = parse_cran_str(
            "Package: demo\n\
             Version: 1.0\n\
             LinkingTo: BH\n",
        )
        .expect("parse");
        let bh = recipe
            .dependencies
            .iter()
            .find(|dep| dep.name == "BH")
            .expect("BH");
        assert_eq!(bh.role, "run");
    }

    #[test]
    fn system_requirements_become_a_judgment_residual() {
        let recipe = parse_cran_str(
            "Package: xml2\n\
             Version: 1.3.6\n\
             SystemRequirements: libxml2 >= 2.9\n",
        )
        .expect("parse");
        assert!(
            recipe.residuals.iter().any(|residual| {
                residual.category == "cran-system-requirements"
                    && residual.summary.contains("libxml2")
            }),
            "{:?}",
            recipe.residuals
        );
    }

    #[test]
    fn cran_json_system_requirements_become_a_judgment_residual() {
        let recipe = parse_cran_str(
            r#"{"Package":"xml2","Version":"1.6.0","SystemRequirements":"libxml2 >= 2.9"}"#,
        )
        .expect("parse");
        assert!(
            recipe.residuals.iter().any(|residual| {
                residual.category == "cran-system-requirements"
                    && residual.summary.contains("libxml2")
            }),
            "{:?}",
            recipe.residuals
        );
    }

    #[test]
    fn multi_stanza_packages_index_is_refused() {
        let err =
            parse_cran_str("Package: A\nVersion: 1.0\nDepends: foo\n\nPackage: B\nVersion: 2.0\n")
                .expect_err("PACKAGES");
        assert!(err.to_string().contains("stanzas"), "{err}");
    }

    #[test]
    fn cran_json_accepts_string_and_object_depends() {
        let from_string = parse_cran_str(
            r#"{"Package":"jsonlite","Version":"1.8.8","Depends":"R (>= 3.1.0), methods"}"#,
        )
        .expect("string Depends");
        assert!(from_string
            .dependencies
            .iter()
            .any(|dep| dep.name == "R" && dep.pin.as_deref() == Some(">= 3.1.0")));
        let from_object = parse_cran_str(
            r#"{"Package":"jsonlite","Version":"1.8.8","Depends":{"R":">= 3.1.0"}}"#,
        )
        .expect("object Depends");
        assert!(from_object
            .dependencies
            .iter()
            .any(|dep| dep.name == "R" && dep.pin.as_deref() == Some(">= 3.1.0")));
    }
}
