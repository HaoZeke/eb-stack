//! Offline LuaRocks rockspec adapter.

use crate::foreign::{
    ForeignDep, ForeignError, ForeignFormat, ForeignRecipe, ForeignResidual, ForeignSource,
};
use crate::package::{ConditionExpr, ConditionPredicate, ResidualSeverity};

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
    let git = url.as_deref().and_then(git_remote_from_rockspec_url);
    let download_url = if git.is_some() { None } else { url.clone() };
    let filename = source_file
        .clone()
        .or_else(|| download_url.as_deref().and_then(url_basename));
    let mut lua_pin = None;
    let mut dependencies = Vec::new();
    for (spec, condition) in lua_string_list(text, "dependencies") {
        let (dep_name, pin) = split_luarocks_dep(&spec);
        if dep_name.eq_ignore_ascii_case("lua") {
            lua_pin = pin;
            continue;
        }
        dependencies.push(ForeignDep {
            name: dep_name,
            pin,
            role: "run".into(),
            original_spec: Some(spec),
            condition,
            provenance: Vec::new(),
        });
    }
    for (spec, condition) in lua_string_list(text, "build_dependencies") {
        let (dep_name, pin) = split_luarocks_dep(&spec);
        dependencies.push(ForeignDep {
            name: dep_name,
            pin,
            role: "build".into(),
            original_spec: Some(spec),
            condition,
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
    let sources = if download_url.is_some() || git.is_some() {
        vec![ForeignSource {
            url: download_url.clone(),
            filename: filename.clone(),
            sha256: sha256.clone(),
            git: git.clone(),
            tag: tag.clone(),
            ..ForeignSource::default()
        }]
    } else {
        Vec::new()
    };
    Ok(ForeignRecipe {
        format: ForeignFormat::Luarocks,
        name,
        version,
        homepage,
        source_url: download_url.clone(),
        source_filename: filename,
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
        residuals: if sha256.is_none() && git.is_none() {
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

fn split_luarocks_dep(spec: &str) -> (String, Option<String>) {
    let mut parts = spec.split_whitespace();
    let name = parts.next().unwrap_or(spec).to_string();
    let pin = parts.collect::<Vec<_>>().join(" ");
    let pin = (!pin.is_empty()).then(|| canonical_luarocks_pin(&pin));
    (name, pin)
}

/// Rewrite LuaRocks operators into the shared requirement language.
///
/// `~> 1.8` is `>=1.8,<1.9`. `~=` is not-equal, not PEP 440 compatible-release.
fn canonical_luarocks_pin(pin: &str) -> String {
    pin.split(',')
        .map(str::trim)
        .filter(|clause| !clause.is_empty())
        .map(canonical_luarocks_clause)
        .collect::<Vec<_>>()
        .join(",")
}

fn canonical_luarocks_clause(clause: &str) -> String {
    if let Some(version) = strip_luarocks_operator(clause, "~>") {
        return match bump_last_component(&version) {
            Some(next) => format!(">={version},<{next}"),
            None => format!(">={version}"),
        };
    }
    if let Some(version) = strip_luarocks_operator(clause, "~=") {
        return format!("!={version}");
    }
    clause.to_string()
}

fn strip_luarocks_operator(clause: &str, operator: &str) -> Option<String> {
    clause
        .strip_prefix(operator)
        .map(str::trim)
        .filter(|version| !version.is_empty())
        .map(ToString::to_string)
}

fn bump_last_component(version: &str) -> Option<String> {
    let mut components = version
        .split('.')
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    let last = components.last_mut()?;
    *last = last.checked_add(1)?;
    Some(
        components
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join("."),
    )
}

fn git_remote_from_rockspec_url(url: &str) -> Option<String> {
    use crate::artifact_class::{classify_url, ArtifactClass};
    let url = url.trim();
    if classify_url(url) != ArtifactClass::GitCheckout {
        return None;
    }
    let remote = if url.len() >= 4 && url[..4].eq_ignore_ascii_case("git+") {
        &url[4..]
    } else {
        url
    };
    (!remote.is_empty()).then(|| remote.to_string())
}

fn url_basename(url: &str) -> Option<String> {
    url.split(['?', '#'])
        .next()
        .unwrap_or(url)
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .map(ToString::to_string)
}

fn lua_string(text: &str, key: &str) -> Option<String> {
    for line in text.lines() {
        let Some(trimmed) = lua_live_line(line) else {
            continue;
        };
        for prefix in [format!("{key} ="), format!("{key}=")] {
            if let Some(rest) = trimmed.strip_prefix(&prefix) {
                return unquote(rest.trim());
            }
        }
    }
    None
}

fn lua_nested_string(text: &str, table: &str, key: &str) -> Option<String> {
    let body = lua_table_body(text, table)?;
    for line in body.lines() {
        let Some(trimmed) = lua_live_line(line) else {
            continue;
        };
        let trimmed = trimmed.trim_end_matches(',');
        for needle in [format!("{key} ="), format!("{key}=")] {
            if let Some(idx) = trimmed.find(&needle) {
                return unquote(trimmed[idx + needle.len()..].trim());
            }
        }
    }
    None
}

fn lua_table_body(text: &str, table: &str) -> Option<String> {
    let prefixes = [format!("{table} ="), format!("{table}=")];
    let mut waiting = false;
    let mut collecting = false;
    let mut depth = 0i32;
    let mut file_depth = 0i32;
    let mut body = String::new();
    for line in text.lines() {
        let Some(trimmed) = lua_live_line(line) else {
            continue;
        };
        if !collecting {
            if file_depth == 0 {
                if let Some(rest) = prefixes
                    .iter()
                    .find_map(|prefix| trimmed.strip_prefix(prefix.as_str()))
                {
                    if let Some(idx) = rest.find('{') {
                        collecting = true;
                        depth = 1;
                        body.push_str(&rest[idx + 1..]);
                        body.push('\n');
                        depth += lua_brace_delta(&rest[idx + 1..]);
                        if depth <= 0 {
                            return Some(body);
                        }
                        continue;
                    }
                    waiting = true;
                    continue;
                }
                if waiting && trimmed.starts_with('{') {
                    collecting = true;
                    waiting = false;
                    depth = 1;
                    body.push_str(&trimmed[1..]);
                    body.push('\n');
                    depth += lua_brace_delta(&trimmed[1..]);
                    if depth <= 0 {
                        return Some(body);
                    }
                    continue;
                }
                waiting = false;
            }
            file_depth += lua_brace_delta(trimmed);
            continue;
        }
        depth += lua_brace_delta(trimmed);
        body.push_str(trimmed);
        body.push('\n');
        if depth <= 0 {
            return Some(body);
        }
    }
    None
}

enum LuaEntry {
    Item(String),
    Table { key: String, body: String },
}

fn lua_string_list(text: &str, key: &str) -> Vec<(String, ConditionExpr)> {
    let mut items = Vec::new();
    if let Some(body) = lua_table_body(text, key) {
        collect_lua_dep_specs(&body, ConditionExpr::Always, key, &mut items);
    }
    if let Some(body) = lua_table_body(text, "platforms") {
        collect_lua_platform_specs(&body, key, &mut items);
    }
    items
}

fn collect_lua_dep_specs(
    body: &str,
    condition: ConditionExpr,
    key: &str,
    items: &mut Vec<(String, ConditionExpr)>,
) {
    for entry in lua_table_entries(body) {
        match entry {
            LuaEntry::Item(spec) => items.push((spec, condition.clone())),
            LuaEntry::Table {
                key: nested,
                body: nested_body,
            } if nested == "platforms" => {
                collect_lua_platform_specs(&nested_body, key, items);
            }
            LuaEntry::Table {
                key: nested,
                body: nested_body,
            } if nested == key => {
                collect_lua_dep_specs(&nested_body, condition.clone(), key, items);
            }
            LuaEntry::Table { .. } => {}
        }
    }
}

fn collect_lua_platform_specs(body: &str, key: &str, items: &mut Vec<(String, ConditionExpr)>) {
    for entry in lua_table_entries(body) {
        if let LuaEntry::Table {
            key: platform,
            body: nested_body,
        } = entry
        {
            collect_lua_dep_specs(&nested_body, platform_condition(&platform), key, items);
        }
    }
}

fn platform_condition(platform: &str) -> ConditionExpr {
    ConditionExpr::Predicate(ConditionPredicate::Platform {
        name: platform.to_string(),
    })
}

fn lua_table_entries(body: &str) -> Vec<LuaEntry> {
    let mut entries = Vec::new();
    let mut depth = 1i32;
    let mut waiting_key: Option<String> = None;
    let mut collecting_key: Option<String> = None;
    let mut collect_body = String::new();
    let mut collect_depth = 0i32;

    for line in body.lines() {
        let Some(trimmed) = lua_live_line(line) else {
            continue;
        };

        if collecting_key.is_some() {
            collect_body.push_str(trimmed);
            collect_body.push('\n');
            collect_depth += lua_brace_delta(trimmed);
            if collect_depth <= 0 {
                entries.push(LuaEntry::Table {
                    key: collecting_key.take().expect("collecting_key is some"),
                    body: std::mem::take(&mut collect_body),
                });
            }
            continue;
        }

        if let Some(key) = waiting_key.take() {
            if let Some(inner) = trimmed.strip_prefix('{') {
                start_lua_subtable(
                    key,
                    inner,
                    &mut collecting_key,
                    &mut collect_body,
                    &mut collect_depth,
                    &mut entries,
                );
                continue;
            }
        }

        if depth != 1 {
            depth += lua_brace_delta(trimmed);
            if depth <= 0 {
                break;
            }
            continue;
        }

        if let Some((key, rest)) = split_lua_assign(trimmed) {
            if rest.is_empty() {
                waiting_key = Some(key);
                continue;
            }
            if let Some(inner) = rest.strip_prefix('{') {
                start_lua_subtable(
                    key,
                    inner,
                    &mut collecting_key,
                    &mut collect_body,
                    &mut collect_depth,
                    &mut entries,
                );
                continue;
            }
            continue;
        }

        push_lua_items(trimmed, &mut entries);
        depth += lua_brace_delta(trimmed);
        if depth <= 0 {
            break;
        }
    }
    entries
}

fn start_lua_subtable(
    key: String,
    after_brace: &str,
    collecting_key: &mut Option<String>,
    collect_body: &mut String,
    collect_depth: &mut i32,
    entries: &mut Vec<LuaEntry>,
) {
    collect_body.push_str(after_brace);
    collect_body.push('\n');
    *collect_depth = 1 + lua_brace_delta(after_brace);
    if *collect_depth <= 0 {
        entries.push(LuaEntry::Table {
            key,
            body: std::mem::take(collect_body),
        });
        *collecting_key = None;
    } else {
        *collecting_key = Some(key);
    }
}

fn split_lua_assign(trimmed: &str) -> Option<(String, &str)> {
    let eq = trimmed.find('=')?;
    let key = trimmed[..eq].trim();
    if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    Some((
        key.to_string(),
        trimmed[eq + 1..].trim().trim_end_matches(',').trim(),
    ))
}

fn push_lua_items(trimmed: &str, entries: &mut Vec<LuaEntry>) {
    let mut quoted = Vec::new();
    collect_quoted(trimmed, &mut quoted);
    if !quoted.is_empty() {
        entries.extend(quoted.into_iter().map(LuaEntry::Item));
        return;
    }
    for token in lua_bare_idents(trimmed) {
        entries.push(LuaEntry::Item(token));
    }
}

fn lua_bare_idents(text: &str) -> Vec<String> {
    text.replace(['{', '}'], " ")
        .split(',')
        .map(str::trim)
        .filter(|token| {
            !token.is_empty()
                && token
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                && token.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
        .map(ToString::to_string)
        .collect()
}

fn lua_live_line(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with("--") {
        None
    } else {
        Some(trimmed)
    }
}

fn lua_brace_delta(text: &str) -> i32 {
    let opens = text.chars().filter(|c| *c == '{').count() as i32;
    let closes = text.chars().filter(|c| *c == '}').count() as i32;
    opens - closes
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
        assert!(
            recipe
                .residuals
                .iter()
                .any(|residual| residual.category == "luarocks-checksum"),
            "{:?}",
            recipe.residuals
        );
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

    #[test]
    fn assignment_without_a_space_is_still_read() {
        let recipe = parse_luarocks_str(
            r#"
package="lfs"
version="1.0-1"
source={url="https://example.invalid/lfs.tgz"}
dependencies={"bit32"}
"#,
        )
        .expect("parse");
        assert_eq!(recipe.name, "lfs");
        assert_eq!(recipe.version, "1.0");
        assert_eq!(
            recipe.source_url.as_deref(),
            Some("https://example.invalid/lfs.tgz")
        );
        assert!(recipe.dependencies.iter().any(|dep| dep.name == "bit32"));
    }

    #[test]
    fn git_plus_https_source_is_a_checkout() {
        let recipe = parse_luarocks_str(
            r#"
package = "lfs"
version = "1.8.0-1"
source = {
  url = "git+https://github.com/lunarmodules/luafilesystem.git",
  tag = "v1.8.0"
}
"#,
        )
        .expect("parse");
        assert_eq!(recipe.sources.len(), 1);
        assert_eq!(
            recipe.sources[0].git.as_deref(),
            Some("https://github.com/lunarmodules/luafilesystem.git")
        );
        assert!(recipe.sources[0].url.is_none());
        assert_eq!(recipe.sources[0].tag.as_deref(), Some("v1.8.0"));
        assert!(recipe.source_url.is_none());
        assert!(
            recipe
                .residuals
                .iter()
                .all(|residual| residual.category != "luarocks-checksum"),
            "{:?}",
            recipe.residuals
        );
    }

    #[test]
    fn git_scheme_source_is_a_checkout() {
        let recipe = parse_luarocks_str(
            r#"
package = "lfs"
version = "1.8.0-1"
source = {
  url = "git://github.com/lunarmodules/luafilesystem.git",
  tag = "v1.8.0"
}
"#,
        )
        .expect("parse");
        assert_eq!(
            recipe.sources[0].git.as_deref(),
            Some("git://github.com/lunarmodules/luafilesystem.git")
        );
        assert!(recipe.sources[0].url.is_none());
    }

    #[test]
    fn pessimistic_and_not_equal_pins_become_pep_operators() {
        let recipe = parse_luarocks_str(
            r#"
package = "demo"
version = "1.0-1"
source = { url = "https://example.invalid/demo.tgz" }
dependencies = {
  "lfs ~> 1.8",
  "bit32 ~= 1.0"
}
"#,
        )
        .expect("parse");
        let lfs = recipe
            .dependencies
            .iter()
            .find(|dep| dep.name == "lfs")
            .expect("lfs");
        assert_eq!(lfs.pin.as_deref(), Some(">=1.8,<1.9"));
        assert_eq!(lfs.original_spec.as_deref(), Some("lfs ~> 1.8"));
        assert!(crate::version::matches_req(
            "1.8.0",
            lfs.pin.as_deref().unwrap()
        ));
        assert!(!crate::version::matches_req(
            "1.9.0",
            lfs.pin.as_deref().unwrap()
        ));
        let bit32 = recipe
            .dependencies
            .iter()
            .find(|dep| dep.name == "bit32")
            .expect("bit32");
        assert_eq!(bit32.pin.as_deref(), Some("!=1.0"));
        assert!(crate::version::matches_req(
            "1.1",
            bit32.pin.as_deref().unwrap()
        ));
        assert!(!crate::version::matches_req(
            "1.0",
            bit32.pin.as_deref().unwrap()
        ));
    }

    #[test]
    fn live_source_url_wins_over_commented_url() {
        let recipe = parse_luarocks_str(
            r#"
package = "lfs"
version = "1.8.0-1"
source = {
  -- url = "https://old.example/lfs.tgz"
  url = "https://new.example/lfs.tgz",
  tag = "v1.8.0"
}
description = {
  -- summary = "old summary"
  summary = "LuaFileSystem",
  -- homepage = "https://old.example"
  homepage = "https://new.example"
}
"#,
        )
        .expect("parse");
        assert_eq!(
            recipe.source_url.as_deref(),
            Some("https://new.example/lfs.tgz")
        );
        assert_eq!(recipe.sources[0].tag.as_deref(), Some("v1.8.0"));
        assert_eq!(recipe.summary.as_deref(), Some("LuaFileSystem"));
        assert_eq!(recipe.homepage.as_deref(), Some("https://new.example"));
    }

    #[test]
    fn next_line_brace_keeps_bit32_and_platforms_are_not_always() {
        let recipe = parse_luarocks_str(
            r#"
package = "lfs"
version = "1.8.0-1"
source = { url = "https://example.invalid/lfs.tgz" }
dependencies =
{ bit32 }
platforms = {
  unix = {
    dependencies = { posix }
  },
  windows = {
    dependencies = { winapi }
  }
}
"#,
        )
        .expect("parse");
        assert!(
            recipe.dependencies.iter().any(|dep| dep.name == "bit32"),
            "{:?}",
            recipe.dependencies
        );
        let posix = recipe
            .dependencies
            .iter()
            .find(|dep| dep.name == "posix")
            .expect("posix");
        assert_ne!(posix.condition, ConditionExpr::Always, "{posix:?}");
        let winapi = recipe
            .dependencies
            .iter()
            .find(|dep| dep.name == "winapi")
            .expect("winapi");
        assert_ne!(winapi.condition, ConditionExpr::Always, "{winapi:?}");
    }
}
