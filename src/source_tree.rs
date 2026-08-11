//! Source-tree overlay and static import scan.
//!
//! Wrap names and PEP 518 requires come from the sdist. Undeclared
//! imports become residuals, never silent SAT edges.

use crate::foreign::{ForeignDep, ForeignRecipe, ForeignResidual};
use crate::package::{ConditionExpr, ResidualSeverity};
use crate::provides::overlay_package_identity;
use std::path::Path;

/// Enrich a PyPI recipe from a dump's neighbouring source tree.
pub fn enrich_from_source_tree(recipe: &mut ForeignRecipe, dump: &Path) {
    let Some(parent) = dump.parent() else {
        return;
    };
    let tree = find_source_tree(dump, parent, &recipe.name, &recipe.version);
    let Some(tree) = tree else {
        return;
    };
    overlay_pyproject(recipe, &tree.join("pyproject.toml"));
    overlay_meson_wraps(recipe, &tree.join("subprojects"));
    scan_python_imports(recipe, &tree);
}

fn find_source_tree(
    dump: &Path,
    parent: &Path,
    name: &str,
    version: &str,
) -> Option<std::path::PathBuf> {
    let identity = format!("{name}-{version}");
    let mut candidates = Vec::new();
    if let Some(stem) = dump.file_stem().and_then(|stem| stem.to_str()) {
        candidates.push(parent.join(stem));
        if stem != identity {
            return candidates.into_iter().find(|path| path.is_dir());
        }
    }
    candidates.push(parent.join(&identity));
    candidates.push(parent.join(name));
    candidates.push(parent.join(format!("{}-{version}", name.replace('-', "_"))));
    candidates.into_iter().find(|path| path.is_dir())
}

fn overlay_pyproject(recipe: &mut ForeignRecipe, path: &Path) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let Ok(value) = text.parse::<toml::Value>() else {
        return;
    };
    let Some(build) = value.get("build-system") else {
        return;
    };
    let backend = build
        .get("build-backend")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    if backend.contains("meson") {
        push_hint(recipe, "meson");
        push_hint(recipe, "mesonpy");
    }
    if backend.contains("hatchling") {
        push_hint(recipe, "hatchling");
    }
    let Some(requires) = build.get("requires").and_then(|value| value.as_array()) else {
        return;
    };
    for spec in requires {
        let Some(spec) = spec.as_str() else {
            continue;
        };
        let name = spec
            .split(|ch: char| {
                ch == '>' || ch == '<' || ch == '=' || ch == '!' || ch == ' ' || ch == '['
            })
            .next()
            .unwrap_or(spec)
            .trim();
        if name.is_empty()
            || name.eq_ignore_ascii_case("numpy")
            || name.eq_ignore_ascii_case("python")
        {
            continue;
        }
        push_dep(recipe, name, "build", spec);
    }
}

fn overlay_meson_wraps(recipe: &mut ForeignRecipe, subprojects: &Path) {
    let Ok(entries) = std::fs::read_dir(subprojects) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("wrap") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        push_hint(recipe, "meson");
        push_dep(recipe, stem, "build", &format!("meson.wrap:{stem}"));
    }
}

fn scan_python_imports(recipe: &mut ForeignRecipe, tree: &Path) {
    let mut stack = vec![tree.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().and_then(|name| name.to_str()) == Some("subprojects") {
                    continue;
                }
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|ext| ext.to_str()) != Some("py") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            for name in python_imports(&text) {
                if declared(recipe, &name) {
                    continue;
                }
                let already = recipe.residuals.iter().any(|residual| {
                    residual.category == "undeclared-import" && residual.summary.contains(&name)
                });
                if already {
                    continue;
                }
                recipe.residuals.push(ForeignResidual {
                    category: "undeclared-import".into(),
                    severity: ResidualSeverity::Judgment,
                    summary: format!("undeclared import {name}"),
                    evidence: Some(path.display().to_string()),
                    provenance: None,
                });
            }
        }
    }
}

fn python_imports(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("import ") {
            if let Some(name) = rest
                .split(|ch: char| ch == ' ' || ch == '.' || ch == ',')
                .next()
            {
                push_import(&mut names, name);
            }
        }
        if let Some(rest) = trimmed.strip_prefix("from ") {
            if let Some(name) = rest.split_whitespace().next() {
                if let Some(root) = name.split('.').next() {
                    push_import(&mut names, root);
                }
            }
        }
    }
    names
}

fn push_import(names: &mut Vec<String>, name: &str) {
    let name = name.trim();
    if name.is_empty() || name.starts_with('.') {
        return;
    }
    if !names.iter().any(|existing| existing == name) {
        names.push(name.to_string());
    }
}

fn declared(recipe: &ForeignRecipe, import_name: &str) -> bool {
    let identity = import_identity(import_name);
    if identity == overlay_package_identity(&recipe.name) {
        return true;
    }
    recipe
        .dependencies
        .iter()
        .any(|dep| import_identity(&dep.name) == identity)
}

fn import_identity(name: &str) -> String {
    let identity = overlay_package_identity(name);
    match identity.as_str() {
        "yaml" => overlay_package_identity("PyYAML"),
        other => other.to_string(),
    }
}

fn push_hint(recipe: &mut ForeignRecipe, hint: &str) {
    if !recipe
        .build_system_hints
        .iter()
        .any(|existing| existing == hint)
    {
        recipe.build_system_hints.push(hint.into());
    }
}

fn push_dep(recipe: &mut ForeignRecipe, name: &str, role: &str, spec: &str) {
    if declared(recipe, name) {
        return;
    }
    recipe.dependencies.push(ForeignDep {
        name: name.to_string(),
        pin: None,
        role: role.into(),
        original_spec: Some(spec.to_string()),
        condition: ConditionExpr::Always,
        provenance: Vec::new(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::foreign::ForeignFormat;

    fn empty_recipe() -> ForeignRecipe {
        ForeignRecipe {
            format: ForeignFormat::Pypi,
            name: "demo".into(),
            version: "1.0.0".into(),
            homepage: None,
            source_url: None,
            source_filename: None,
            sha256: None,
            sources: Vec::new(),
            summary: None,
            description: None,
            license: None,
            dependencies: Vec::new(),
            build_system_hints: Vec::new(),
            configopts: None,
            patches: Vec::new(),
            variants: Vec::new(),
            rules: Vec::new(),
            notes: Vec::new(),
            residuals: Vec::new(),
        }
    }

    #[test]
    fn wrap_file_becomes_a_build_dep() {
        let root = tempfile::tempdir().expect("temp");
        let tree = root.path().join("demo-1.0.0");
        std::fs::create_dir_all(tree.join("subprojects")).expect("dirs");
        std::fs::write(tree.join("subprojects/quill.wrap"), "[wrap-file]\n").expect("wrap");
        let dump = root.path().join("demo-1.0.0.json");
        std::fs::write(&dump, "{}").expect("dump");
        let mut recipe = empty_recipe();
        enrich_from_source_tree(&mut recipe, &dump);
        assert!(
            recipe
                .dependencies
                .iter()
                .any(|dep| dep.name == "quill" && dep.role == "build"),
            "{:?}",
            recipe.dependencies
        );
    }

    #[test]
    fn undeclared_import_is_a_residual_not_a_dep() {
        let root = tempfile::tempdir().expect("temp");
        let tree = root.path().join("demo-1.0.0");
        std::fs::create_dir_all(tree.join("pkg")).expect("dirs");
        std::fs::write(tree.join("pkg/mod.py"), "import yaml\n").expect("py");
        let dump = root.path().join("demo-1.0.0.json");
        std::fs::write(&dump, "{}").expect("dump");
        let mut recipe = empty_recipe();
        enrich_from_source_tree(&mut recipe, &dump);
        assert!(
            recipe
                .residuals
                .iter()
                .any(|residual| residual.category == "undeclared-import"
                    && residual.summary.contains("yaml")),
            "{:?}",
            recipe.residuals
        );
        assert!(
            !recipe.dependencies.iter().any(|dep| dep.name == "yaml"),
            "{:?}",
            recipe.dependencies
        );
    }
}
