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
    candidates
        .into_iter()
        .find(|path| path.is_dir())
        .map(|path| descend_into_sdist_root(&path))
}

/// The directory an sdist actually keeps its `pyproject.toml` in.
///
/// A PyPI sdist unpacks to one directory named after the release, so a tree
/// extracted into `ingest/pypi/archspec-0.2.5/` holds
/// `archspec-0.2.5/pyproject.toml` inside it. Reading the outer directory
/// finds no build system at all, and the recipe then states no build
/// dependency: archspec is built with poetry-core and upstream's recipe says
/// so.
fn descend_into_sdist_root(tree: &Path) -> std::path::PathBuf {
    if tree.join("pyproject.toml").is_file() || tree.join("setup.py").is_file() {
        return tree.to_path_buf();
    }
    let Ok(entries) = std::fs::read_dir(tree) else {
        return tree.to_path_buf();
    };
    let mut directories = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir());
    let Some(only) = directories.next() else {
        return tree.to_path_buf();
    };
    if directories.next().is_some() {
        return tree.to_path_buf();
    }
    if only.join("pyproject.toml").is_file() || only.join("setup.py").is_file() {
        return only;
    }
    tree.to_path_buf()
}

fn overlay_pyproject(recipe: &mut ForeignRecipe, path: &Path) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    // `parse::<toml::Value>()` reads a TOML *value*, not a document, so a
    // pyproject.toml fails at its first table header and every build
    // requirement a project states was being dropped in silence.
    let Ok(value) = toml::from_str::<toml::Value>(&text) else {
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
    // The backend itself, so the plan can make it a build dependency: a
    // project built with poetry-core needs the `poetry` module at build time,
    // and upstream's archspec recipe names it.
    if !backend.is_empty() {
        push_hint(recipe, &format!("backend:{backend}"));
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
        // The interpreter and the array package are the build backend's own
        // floor rather than a dependency this recipe states, and which names
        // those are is policy: data/overlay-policy.toml holds the list.
        if name.is_empty() || crate::provides::ignored_build_requirement(name) {
            continue;
        }
        // The Python module installs setuptools, pip and wheel itself, so
        // naming one as a build dependency sends the solver looking for
        // whatever ships it. Upstream's coverage and cppy recipes name
        // neither, and both declare setuptools as their backend.
        if crate::provides::shipped_with_python(name) {
            continue;
        }
        // A backend and the module that carries it are one name to a recipe:
        // `poetry-core` is built by `poetry`, and emitting both asks for the
        // same thing twice.
        let name = crate::provides::aliased_module_name(name);
        push_dep(recipe, &name, "build", spec);
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
            if let Some(name) = rest.split([' ', '.', ',']).next() {
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
            classifiers: Vec::new(),
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

#[cfg(test)]
mod sdist_layout_tests {
    use super::*;

    #[test]
    fn a_pyproject_inside_the_sdist_root_is_found() {
        let temp = tempfile::tempdir().expect("temp");
        let ingest = temp.path().join("pypi");
        let outer = ingest.join("archspec-0.2.5");
        let inner = outer.join("archspec-0.2.5");
        std::fs::create_dir_all(&inner).expect("dirs");
        std::fs::write(
            inner.join("pyproject.toml"),
            "[build-system]\nrequires = [\"poetry-core>=1.0.0\"]\nbuild-backend = \"poetry.core.masonry.api\"\n",
        )
        .expect("write");
        let dump = ingest.join("archspec-0.2.5.json");
        std::fs::write(&dump, "{}").expect("dump");
        let found = find_source_tree(&dump, &ingest, "archspec", "0.2.5");
        assert_eq!(found.as_deref(), Some(inner.as_path()));
    }
}

#[cfg(test)]
mod sdist_overlay_tests {
    use super::*;
    use crate::foreign::ForeignFormat;

    #[test]
    fn overlay_pyproject_reads_a_poetry_backend() {
        let temp = tempfile::tempdir().expect("temp");
        let path = temp.path().join("pyproject.toml");
        std::fs::write(
            &path,
            "[build-system]\nrequires = [\"poetry-core>=1.0.0\"]\nbuild-backend = \"poetry.core.masonry.api\"\n",
        )
        .expect("write");
        let mut recipe = ForeignRecipe {
            format: ForeignFormat::Pypi,
            name: "archspec".into(),
            version: "0.2.5".into(),
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
            classifiers: Vec::new(),
        };
        let text = std::fs::read_to_string(&path).expect("read back");
        let parsed = toml::from_str::<toml::Value>(&text);
        assert!(parsed.is_ok(), "parse: {parsed:?}");
        let value = parsed.expect("parsed");
        assert!(
            value.get("build-system").is_some(),
            "no build-system table in {value:?}"
        );
        overlay_pyproject(&mut recipe, &path);
        assert!(
            recipe.build_system_hints.iter().any(|hint| hint.starts_with("backend:")),
            "hints {:?} deps {:?}",
            recipe.build_system_hints,
            recipe.dependencies
        );
    }

    #[test]
    fn a_poetry_backend_becomes_a_hint() {
        let temp = tempfile::tempdir().expect("temp");
        let ingest = temp.path().join("pypi");
        let inner = ingest.join("archspec-0.2.5").join("archspec-0.2.5");
        std::fs::create_dir_all(&inner).expect("dirs");
        std::fs::write(
            inner.join("pyproject.toml"),
            "[build-system]\nrequires = [\"poetry-core>=1.0.0\"]\nbuild-backend = \"poetry.core.masonry.api\"\n",
        )
        .expect("write");
        let dump = ingest.join("archspec-0.2.5.json");
        std::fs::write(&dump, "{}").expect("dump");
        let mut recipe = ForeignRecipe {
            format: ForeignFormat::Pypi,
            name: "archspec".into(),
            version: "0.2.5".into(),
            homepage: None,
            source_url: None,
            source_filename: None,
            sha256: None,
            sources: Vec::new(),
            summary: None,
            description: None,
            license: None,
            dependencies: Vec::new(),
            build_system_hints: vec!["python-bundle".into(), "pip".into()],
            configopts: None,
            patches: Vec::new(),
            variants: Vec::new(),
            rules: Vec::new(),
            notes: Vec::new(),
            residuals: Vec::new(),
            classifiers: Vec::new(),
        };
        let found = find_source_tree(&dump, &ingest, "archspec", "0.2.5");
        assert!(
            found
                .as_deref()
                .is_some_and(|path| path.join("pyproject.toml").is_file()),
            "resolved tree: {found:?}"
        );
        enrich_from_source_tree(&mut recipe, &dump);
        assert!(
            recipe
                .build_system_hints
                .iter()
                .any(|hint| hint.starts_with("backend:")),
            "{:?}",
            recipe.build_system_hints
        );
        assert!(
            recipe
                .dependencies
                .iter()
                .any(|dependency| dependency.name.contains("poetry")),
            "{:?}",
            recipe.dependencies
        );
    }
}
