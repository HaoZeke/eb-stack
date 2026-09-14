//! Offline Raku META6.json adapter.

use crate::foreign::{ForeignDep, ForeignError, ForeignFormat, ForeignRecipe, ForeignSource};
use crate::package::ConditionExpr;
use serde::Deserialize;
use std::collections::BTreeMap;

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
    depends: Meta6Depends,
    #[serde(default, rename = "build-depends")]
    build_depends: Meta6Depends,
    #[serde(default, rename = "test-depends")]
    test_depends: Meta6Depends,
}

#[derive(Debug, Default, Deserialize)]
#[serde(untagged)]
enum Meta6Depends {
    #[default]
    Missing,
    List(Vec<Meta6DepSpec>),
    Table(BTreeMap<String, Vec<Meta6DepSpec>>),
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Meta6DepSpec {
    String(String),
    Object {
        name: String,
        #[serde(default)]
        version: Option<String>,
    },
}

/// Parse a META6.json body.
pub fn parse_raku_str(text: &str) -> Result<ForeignRecipe, ForeignError> {
    let doc: Meta6 = serde_json::from_str(text)
        .map_err(|error| ForeignError::Parse(format!("META6.json: {error}")))?;
    let url = doc.source_url.clone().or(doc.source_url_kebab.clone());
    let mut dependencies = Vec::new();
    for (spec, role) in doc
        .depends
        .into_pairs("run")
        .into_iter()
        .chain(doc.build_depends.into_pairs("build"))
        .chain(doc.test_depends.into_pairs("test"))
    {
        dependencies.push(dep(&spec, role));
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
        classifiers: Vec::new(),
    })
}

impl Meta6Depends {
    fn into_pairs(self, default_role: &'static str) -> Vec<(String, &'static str)> {
        match self {
            Self::Missing => Vec::new(),
            Self::List(specs) => specs
                .into_iter()
                .map(|spec| (spec.into_string(), default_role))
                .collect(),
            Self::Table(table) => {
                let mut out = Vec::new();
                for (key, specs) in table {
                    let role = match key.as_str() {
                        "build" => "build",
                        "test" => "test",
                        _ => "run",
                    };
                    for spec in specs {
                        out.push((spec.into_string(), role));
                    }
                }
                out
            }
        }
    }
}

impl Meta6DepSpec {
    fn into_string(self) -> String {
        match self {
            Self::String(spec) => spec,
            Self::Object { name, version } => match version {
                Some(version) => format!("{name}:ver<{version}>"),
                None => name,
            },
        }
    }
}

fn dep(spec: &str, role: &str) -> ForeignDep {
    let pin = raku_adverb(spec, "ver");
    let name = raku_module_name(spec);
    ForeignDep {
        name,
        pin,
        role: role.into(),
        original_spec: Some(spec.to_string()),
        condition: ConditionExpr::Always,
        provenance: Vec::new(),
    }
}

fn raku_adverb(spec: &str, key: &str) -> Option<String> {
    let angled = format!(":{key}<");
    let paren = format!(":{key}(");
    let rest = spec
        .split_once(&angled)
        .or_else(|| spec.split_once(&paren))?
        .1;
    rest.split(['>', ')'])
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn raku_module_name(spec: &str) -> String {
    let cut = ["ver", "auth", "api", "from"]
        .into_iter()
        .filter_map(|key| {
            spec.find(&format!(":{key}<"))
                .or_else(|| spec.find(&format!(":{key}(")))
        })
        .min()
        .unwrap_or(spec.len());
    spec[..cut]
        .split([' ', '<', '>'])
        .next()
        .unwrap_or(&spec[..cut])
        .trim()
        .trim_end_matches(':')
        .to_string()
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
            .any(|dep| dep.name == "JSON::Fast" && dep.role == "run" && dep.pin.is_none()));
        assert!(recipe
            .dependencies
            .iter()
            .any(|dep| dep.name == "App::Prove6" && dep.role == "build"));
        assert_eq!(recipe.format, ForeignFormat::Raku);
    }

    #[test]
    fn meta6_ver_adverb_is_a_pin_not_the_name() {
        let recipe = parse_raku_str(
            r#"{
              "name": "Demo",
              "version": "0.1.0",
              "depends": ["JSON::Fast:ver<0.10+>"]
            }"#,
        )
        .expect("parse");
        let dep = recipe
            .dependencies
            .iter()
            .find(|dep| dep.name == "JSON::Fast")
            .expect("JSON::Fast");
        assert_eq!(dep.pin.as_deref(), Some("0.10+"));
    }

    #[test]
    fn meta6_auth_and_api_adverbs_are_not_the_name() {
        let recipe = parse_raku_str(
            r#"{
              "name": "Demo",
              "version": "0.1.0",
              "depends": [
                "URI:auth<cpan:TIMOTIMO>:ver<0.3.0+>",
                "JSON::Fast:auth<cpan:TIMOTIMO>",
                "Foo:api<1>:ver<0.1>",
                "libfoo:from<native>"
              ]
            }"#,
        )
        .expect("parse");
        let names: Vec<&str> = recipe
            .dependencies
            .iter()
            .map(|dep| dep.name.as_str())
            .collect();
        assert!(names.contains(&"URI"), "{names:?}");
        assert!(names.contains(&"JSON::Fast"), "{names:?}");
        assert!(names.contains(&"Foo"), "{names:?}");
        assert!(names.contains(&"libfoo"), "{names:?}");
        assert!(
            !names.iter().any(|name| name.contains(":auth")),
            "{names:?}"
        );
        let uri = recipe
            .dependencies
            .iter()
            .find(|dep| dep.name == "URI")
            .expect("URI");
        assert_eq!(uri.pin.as_deref(), Some("0.3.0+"));
    }

    #[test]
    fn meta6_hash_depends_and_test_depends_parse() {
        let recipe = parse_raku_str(
            r#"{
              "name": "Demo",
              "version": "0.1.0",
              "depends": { "runtime": ["JSON::Fast"], "build": ["App::Mi6"] },
              "test-depends": ["Test::META"]
            }"#,
        )
        .expect("parse");
        assert!(recipe
            .dependencies
            .iter()
            .any(|dep| dep.name == "JSON::Fast" && dep.role == "run"));
        assert!(recipe
            .dependencies
            .iter()
            .any(|dep| dep.name == "App::Mi6" && dep.role == "build"));
        assert!(recipe
            .dependencies
            .iter()
            .any(|dep| dep.name == "Test::META" && dep.role == "test"));
    }
}
