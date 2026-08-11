//! Offline Raku META6.json adapter.

use crate::foreign::{ForeignDep, ForeignError, ForeignFormat, ForeignRecipe, ForeignSource};
use crate::package::ConditionExpr;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Meta6 {
    name: String,
    version: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    license: Option<String>,
    #[serde(default)]
    source_url: Option<String>,
    #[serde(default, alias = "source-url")]
    source_url_kebab: Option<String>,
    #[serde(default)]
    depends: Vec<String>,
    #[serde(default, rename = "build-depends")]
    build_depends: Vec<String>,
}

/// Parse a META6.json body.
pub fn parse_raku_str(text: &str) -> Result<ForeignRecipe, ForeignError> {
    let doc: Meta6 = serde_json::from_str(text)
        .map_err(|error| ForeignError::Parse(format!("META6.json: {error}")))?;
    let url = doc.source_url.clone().or(doc.source_url_kebab.clone());
    let mut dependencies = Vec::new();
    for spec in &doc.depends {
        dependencies.push(dep(spec, "run"));
    }
    for spec in &doc.build_depends {
        dependencies.push(dep(spec, "build"));
    }
    let sources = url
        .as_ref()
        .map(|url| {
            vec![ForeignSource {
                url: Some(url.clone()),
                filename: url.rsplit('/').next().map(ToString::to_string),
                ..ForeignSource::default()
            }]
        })
        .unwrap_or_default();
    Ok(ForeignRecipe {
        format: ForeignFormat::Raku,
        name: doc.name,
        version: doc.version,
        homepage: None,
        source_url: url.clone(),
        source_filename: url
            .as_ref()
            .and_then(|url| url.rsplit('/').next().map(ToString::to_string)),
        sha256: None,
        sources,
        summary: doc.description.clone(),
        description: doc.description,
        license: doc.license,
        dependencies,
        build_system_hints: vec!["raku".into()],
        configopts: None,
        patches: Vec::new(),
        variants: Vec::new(),
        rules: Vec::new(),
        notes: vec!["parsed from META6.json".into()],
        residuals: Vec::new(),
    })
}

fn dep(spec: &str, role: &str) -> ForeignDep {
    let name = spec
        .split([' ', '<', '>'])
        .next()
        .unwrap_or(spec)
        .to_string();
    ForeignDep {
        name,
        pin: None,
        role: role.into(),
        original_spec: Some(spec.to_string()),
        condition: ConditionExpr::Always,
        provenance: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta6_reads_depends() {
        let recipe = parse_raku_str(
            r#"{
              "name": "Demo",
              "version": "0.1.0",
              "depends": ["JSON::Fast"],
              "build-depends": ["App::Prove6"]
            }"#,
        )
        .expect("parse");
        assert_eq!(recipe.name, "Demo");
        assert!(recipe
            .dependencies
            .iter()
            .any(|dep| dep.name == "JSON::Fast" && dep.role == "run"));
        assert!(recipe
            .dependencies
            .iter()
            .any(|dep| dep.name == "App::Prove6" && dep.role == "build"));
        assert_eq!(recipe.format, ForeignFormat::Raku);
    }
}
