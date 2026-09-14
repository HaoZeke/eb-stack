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
    let tag = lua_nested_string(text, "source", "tag");
    let source_file = lua_nested_string(text, "source", "file");
    let homepage = lua_nested_string(text, "description", "homepage")
        .or_else(|| lua_nested_string(text, "description", "url"));
    let summary = lua_nested_string(text, "description", "summary");
    let license = lua_nested_string(text, "description", "license");
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
    for spec in lua_string_list(text, "build_dependencies") {
        let mut parts = spec.split_whitespace();
        let dep_name = parts.next().unwrap_or(&spec);
        let pin = parts.collect::<Vec<_>>().join(" ");
        dependencies.push(ForeignDep {
            name: dep_name.to_string(),
            pin: (!pin.is_empty()).then_some(pin),
            role: "build".into(),
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
                filename: source_file.clone().or_else(|| {
                    url.split(['?', '#'])
                        .next()
                        .unwrap_or(url)
                        .rsplit('/')
                        .next()
                        .map(ToString::to_string)
                }),
                sha256: sha256.clone(),
                tag: tag.clone(),
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
        license,
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
    let body = lua_table_body(text, table)?;
    for line in body.lines() {
        let trimmed = line.trim().trim_end_matches(',');
        for needle in [format!("{key} ="), format!("{key}=")] {
            if let Some(idx) = trimmed.find(&needle) {
                return unquote(trimmed[idx + needle.len()..].trim());
            }
        }
    }
    None
}

fn lua_table_body(text: &str, table: &str) -> Option<String> {
    let prefix = format!("{table} =");
    let mut waiting = false;
    let mut collecting = false;
    let mut depth = 0i32;
    let mut body = String::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if !collecting {
            if let Some(rest) = trimmed.strip_prefix(&prefix) {
                if let Some(idx) = rest.find('{') {
                    collecting = true;
                    depth = 1;
                    body.push_str(&rest[idx + 1..]);
                    body.push('\n');
                    depth += rest[idx + 1..].chars().filter(|c| *c == '{').count() as i32;
                    depth -= rest[idx + 1..].chars().filter(|c| *c == '}').count() as i32;
                    if depth <= 0 {
                        return Some(body);
                    }
                } else {
                    waiting = true;
                }
            } else if waiting && trimmed.starts_with('{') {
                collecting = true;
                waiting = false;
                depth = 1;
                body.push_str(&trimmed[1..]);
                body.push('\n');
                depth += trimmed[1..].chars().filter(|c| *c == '{').count() as i32;
                depth -= trimmed[1..].chars().filter(|c| *c == '}').count() as i32;
                if depth <= 0 {
                    return Some(body);
                }
            }
            continue;
        }
        depth += trimmed.chars().filter(|c| *c == '{').count() as i32;
        depth -= trimmed.chars().filter(|c| *c == '}').count() as i32;
        body.push_str(trimmed);
        body.push('\n');
        if depth <= 0 {
            return Some(body);
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
    loop {
        let double = rest.find('"');
        let single = rest.find('\'');
        let (idx, quote) = match (double, single) {
            (Some(d), Some(s)) if s < d => (s, '\''),
            (Some(d), _) => (d, '"'),
            (None, Some(s)) => (s, '\''),
            (None, None) => break,
        };
        rest = &rest[idx + 1..];
        if let Some(end) = rest.find(quote) {
            items.push(rest[..end].to_string());
            rest = &rest[end + 1..];
        } else {
            break;
        }
    }
}

fn unquote(text: &str) -> Option<String> {
    let text = text.trim().trim_end_matches(',').trim();
    if let Some(inner) = text.strip_prefix('"') {
        if let Some(end) = inner.find('"') {
            return Some(inner[..end].to_string());
        }
    }
    if let Some(inner) = text.strip_prefix('\'') {
        if let Some(end) = inner.find('\'') {
            return Some(inner[..end].to_string());
        }
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

    #[test]
    fn one_line_source_table_and_single_quoted_deps() {
        let recipe = parse_luarocks_str(
            r#"
package = 'lfs'
version = '1.8.0-1'
source = { url = "https://example.invalid/lfs-1.8.0.tar.gz", hash = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" }
dependencies = {
  'lua >= 5.1',
  'bit32'
}
"#,
        )
        .expect("parse");
        assert_eq!(
            recipe.source_url.as_deref(),
            Some("https://example.invalid/lfs-1.8.0.tar.gz")
        );
        assert!(recipe.dependencies.iter().any(|dep| dep.name == "bit32"));
        assert_eq!(
            recipe.sha256.as_deref(),
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );
    }
}
