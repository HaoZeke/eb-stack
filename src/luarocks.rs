//! Offline LuaRocks rockspec adapter.

use crate::foreign::{
    ForeignDep, ForeignError, ForeignFormat, ForeignRecipe, ForeignResidual, ForeignSource,
};
use crate::package::{ConditionExpr, ResidualSeverity};

/// Parse a `*.rockspec` body.
pub fn parse_luarocks_str(text: &str) -> Result<ForeignRecipe, ForeignError> {
    let name = lua_string(text, "package")
        .ok_or_else(|| ForeignError::Parse("rockspec missing package".into()))?;
    let version = lua_string(text, "version")
        .ok_or_else(|| ForeignError::Parse("rockspec missing version".into()))?;
    let version = version
        .split_once('-')
        .map(|(ver, _)| ver)
        .unwrap_or(&version)
        .to_string();
    let url = lua_nested_string(text, "source", "url");
    let sha256 = lua_nested_string(text, "source", "hash")
        .or_else(|| lua_nested_string(text, "source", "sha256"));
    let homepage = lua_nested_string(text, "description", "homepage")
        .or_else(|| lua_nested_string(text, "description", "url"));
    let summary = lua_nested_string(text, "description", "summary");
    let mut lua_pin = None;
    let mut dependencies = Vec::new();
    for spec in lua_string_list(text, "dependencies") {
        let mut parts = spec.split_whitespace();
        let dep_name = parts.next().unwrap_or(&spec);
        let pin = parts.collect::<Vec<_>>().join(" ");
        let pin = (!pin.is_empty()).then_some(pin);
        if dep_name.eq_ignore_ascii_case("lua") {
            lua_pin = pin;
            continue;
        }
        dependencies.push(ForeignDep {
            name: dep_name.to_string(),
            pin,
            role: "run".into(),
            original_spec: Some(spec),
            condition: ConditionExpr::Always,
            provenance: Vec::new(),
        });
    }
    dependencies.insert(
        0,
        ForeignDep {
            name: "Lua".into(),
            pin: lua_pin,
            role: "run".into(),
            original_spec: Some("Lua (implicit for LuaRocks)".into()),
            condition: ConditionExpr::Always,
            provenance: Vec::new(),
        },
    );
    let sources = url
        .as_ref()
        .map(|url| {
            vec![ForeignSource {
                url: Some(url.clone()),
                filename: url
                    .split(['?', '#'])
                    .next()
                    .unwrap_or(url)
                    .rsplit('/')
                    .next()
                    .map(ToString::to_string),
                sha256: sha256.clone(),
                ..ForeignSource::default()
            }]
        })
        .unwrap_or_default();
    Ok(ForeignRecipe {
        format: ForeignFormat::Luarocks,
        name,
        version,
        homepage,
        source_url: url.clone(),
        source_filename: url.as_ref().and_then(|url| {
            url.split(['?', '#'])
                .next()
                .unwrap_or(url)
                .rsplit('/')
                .next()
                .map(ToString::to_string)
        }),
        sha256: sha256.clone(),
        sources,
        summary,
        description: None,
        license: None,
        dependencies,
        build_system_hints: vec!["luarocks".into()],
        configopts: None,
        patches: Vec::new(),
        variants: Vec::new(),
        rules: Vec::new(),
        notes: vec!["parsed from a rockspec".into()],
        residuals: if sha256.is_none() {
            vec![ForeignResidual {
                category: "luarocks-checksum".into(),
                severity: ResidualSeverity::Judgment,
                summary: "rockspec source has no hash/sha256".into(),
                evidence: url.clone(),
                provenance: None,
            }]
        } else {
            Vec::new()
        },
        classifiers: Vec::new(),
    })
}

fn lua_string(text: &str, key: &str) -> Option<String> {
    for line in text.lines() {
        let trimmed = line.trim();
        let prefix = format!("{key} =");
        if let Some(rest) = trimmed.strip_prefix(&prefix) {
            return unquote(rest.trim());
        }
    }
    None
}

fn lua_nested_string(text: &str, table: &str, key: &str) -> Option<String> {
    let mut in_table = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with(&format!("{table} =")) && trimmed.contains('{') {
            in_table = true;
        }
        if in_table {
            if let Some(rest) = trimmed.strip_prefix(&format!("{key} =")) {
                return unquote(rest.trim().trim_end_matches(','));
            }
            if trimmed == "}" {
                return None;
            }
        }
    }
    None
}

fn lua_string_list(text: &str, key: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut in_list = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with(&format!("{key} =")) && trimmed.contains('{') {
            in_list = true;
            if let Some(start) = trimmed.find('{') {
                collect_quoted(&trimmed[start + 1..], &mut items);
            }
            if trimmed.contains('}') {
                break;
            }
            continue;
        }
        if in_list {
            if trimmed.contains('}') {
                collect_quoted(trimmed, &mut items);
                break;
            }
            collect_quoted(trimmed, &mut items);
        }
    }
    items
}

fn collect_quoted(text: &str, items: &mut Vec<String>) {
    let mut rest = text;
    while let Some(idx) = rest.find('"') {
        rest = &rest[idx + 1..];
        if let Some(end) = rest.find('"') {
            items.push(rest[..end].to_string());
            rest = &rest[end + 1..];
        } else {
            break;
        }
    }
}

fn unquote(text: &str) -> Option<String> {
    let text = text.trim().trim_end_matches(',').trim();
    if let Some(inner) = text
        .strip_prefix('"')
        .and_then(|text| text.strip_suffix('"'))
    {
        return Some(inner.to_string());
    }
    if let Some(inner) = text
        .strip_prefix('\'')
        .and_then(|text| text.strip_suffix('\''))
    {
        return Some(inner.to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rockspec_reads_package_and_deps() {
        let recipe = parse_luarocks_str(
            r#"
package = "lfs"
version = "1.8.0-1"
source = {
  url = "https://example.invalid/lfs-1.8.0.tar.gz"
}
dependencies = {
  "lua >= 5.1",
  "bit32"
}
"#,
        )
        .expect("parse");
        assert_eq!(recipe.name, "lfs");
        assert_eq!(recipe.version, "1.8.0");
        assert!(recipe.dependencies.iter().any(|dep| dep.name == "bit32"));
        let lua = recipe
            .dependencies
            .iter()
            .find(|dep| dep.name == "Lua")
            .expect("Lua");
        assert_eq!(lua.pin.as_deref(), Some(">= 5.1"));
        assert_eq!(recipe.format, ForeignFormat::Luarocks);
    }
}
