//! Which easyblock classes a changed Python file reaches.
//!
//! The class hierarchy is the `class` statement, not an import of the base:
//! `EB_GROMACS(CMakeMake)` is reached by a change to `CMakeMake`. A framework
//! module reaches the easyblocks that import it, and then their subclasses.
//! Easyconfig matching is layered on top of this index.

use crate::build_order::{module_name, reverse_closure, reverse_uses, Choice, ModuleKey};
use crate::eb_easyblock::encode_class_name;
use crate::eb_parse::{easyblock_sites, EasyblockSites, ResolvedEasyconfig};
use rustpython_parser::ast::{self, Stmt};
use rustpython_parser::Parse;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Schema of [`ReachReport`].
pub const REACH_SCHEMA_VERSION: u32 = 1;

/// What a class declares about itself, read from its source.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClassFact {
    /// Base classes as written, last component or dotted.
    pub bases: Vec<String>,
    /// `self.cfg['exts_defaultclass'] = '...'` in the class body.
    pub exts_default: Option<String>,
    /// `default_easyblock` assigned in `extra_options`.
    pub component_default: Option<String>,
}

/// Easyblock classes and the modules that import them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EasyblockIndex {
    /// Class name to what its source declares.
    pub classes: BTreeMap<String, ClassFact>,
    /// Python module to the classes defined in files that import it.
    pub importers: BTreeMap<String, BTreeSet<String>>,
}

/// Index every `.py` file under `roots`. Earlier roots win a class name.
pub fn index_roots(roots: &[PathBuf]) -> Result<EasyblockIndex, String> {
    let mut files = Vec::new();
    for root in roots {
        collect_py(root, &mut files)
            .map_err(|error| format!("read {}: {error}", root.display()))?;
    }
    let mut index = EasyblockIndex::default();
    for path in files {
        let source = std::fs::read_to_string(&path)
            .map_err(|error| format!("read {}: {error}", path.display()))?;
        let module = python_module(&path);
        let (classes, imports) =
            read_python(&source, &path.display().to_string(), module.as_deref())
                .map_err(|error| format!("parse {}: {error}", path.display()))?;
        let names: Vec<String> = classes.iter().map(|(name, _)| name.clone()).collect();
        for (name, fact) in classes {
            index.classes.entry(name).or_insert(fact);
        }
        if let Some(module) = module.as_deref() {
            for imported in imports {
                if imported != module {
                    index
                        .importers
                        .entry(imported)
                        .or_default()
                        .extend(names.iter().cloned());
                }
            }
        }
    }
    Ok(index)
}

/// Proper subclasses of `class_name`, including subclasses of subclasses.
pub fn descendants(index: &EasyblockIndex, class_name: &str) -> BTreeSet<String> {
    let mut children: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (name, fact) in &index.classes {
        for base in &fact.bases {
            children
                .entry(base.as_str())
                .or_default()
                .push(name.as_str());
            if let Some(short) = base.rsplit('.').next() {
                if short != base {
                    children.entry(short).or_default().push(name.as_str());
                }
            }
        }
    }
    let mut out = BTreeSet::new();
    let mut stack = vec![class_name];
    let mut seen = HashSet::new();
    while let Some(name) = stack.pop() {
        if !seen.insert(name.to_string()) {
            continue;
        }
        if let Some(kids) = children.get(name) {
            for kid in kids {
                if *kid != class_name {
                    out.insert((*kid).to_string());
                    stack.push(kid);
                }
            }
        }
    }
    out
}

/// Classes named by a change, and those plus every subclass.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClassReach {
    /// Defined in the changed file, or defined in a file that imports it.
    pub changed: BTreeSet<String>,
    /// `changed` plus subclasses.
    pub reached: BTreeSet<String>,
}

/// Classes a changed file defines, classes that import its module, and subclasses.
pub fn affected_classes(
    index: &EasyblockIndex,
    path: &Path,
    source: &str,
) -> Result<ClassReach, String> {
    let module = python_module(path);
    let (classes, _) = read_python(source, &path.display().to_string(), module.as_deref())?;
    let mut changed: BTreeSet<String> = classes.into_iter().map(|(name, _)| name).collect();
    if let Some(module) = module {
        if let Some(users) = index.importers.get(&module) {
            changed.extend(users.iter().cloned());
        }
    }
    let mut reached = changed.clone();
    for name in &changed {
        reached.extend(descendants(index, name));
    }
    Ok(ClassReach { changed, reached })
}

/// `easybuild.tools.filetools` for `.../easybuild/tools/filetools.py`.
pub fn python_module(path: &Path) -> Option<String> {
    let text = path.to_string_lossy().replace('\\', "/");
    let at = text.find("easybuild/")?;
    let rest = text[at..].strip_suffix(".py")?;
    let rest = rest.strip_suffix("/__init__").unwrap_or(rest);
    Some(rest.replace('/', "."))
}

type ParsedPython = (Vec<(String, ClassFact)>, BTreeSet<String>);

fn read_python(source: &str, path: &str, module: Option<&str>) -> Result<ParsedPython, String> {
    let suite = ast::Suite::parse(source, path).map_err(|error| error.to_string())?;
    let mut imports = BTreeSet::new();
    collect_imports(&suite, module.unwrap_or(""), &mut imports);
    let classes = suite
        .iter()
        .filter_map(|stmt| match stmt {
            Stmt::ClassDef(class) => Some(class),
            _ => None,
        })
        .map(|class| {
            (
                class.name.to_string(),
                ClassFact {
                    bases: class.bases.iter().filter_map(dotted_name).collect(),
                    exts_default: assigned_string(&class.body, "exts_defaultclass"),
                    component_default: assigned_string(&class.body, "default_easyblock"),
                },
            )
        })
        .collect();
    Ok((classes, imports))
}

fn dotted_name(expr: &ast::Expr) -> Option<String> {
    match expr {
        ast::Expr::Name(name) => Some(name.id.to_string()),
        ast::Expr::Attribute(attr) => {
            dotted_name(&attr.value).map(|base| format!("{base}.{}", attr.attr))
        }
        _ => None,
    }
}

fn assigned_string(stmts: &[Stmt], key: &str) -> Option<String> {
    let mut found = None;
    collect_assigned(stmts, key, &mut found);
    found
}

fn collect_assigned(stmts: &[Stmt], key: &str, found: &mut Option<String>) {
    for stmt in stmts {
        let nested: &[Stmt] = match stmt {
            Stmt::FunctionDef(function) => &function.body,
            Stmt::If(branch) => {
                collect_assigned(&branch.body, key, found);
                &branch.orelse
            }
            Stmt::With(with) => &with.body,
            Stmt::For(node) => &node.body,
            Stmt::While(node) => &node.body,
            Stmt::Try(node) => {
                collect_assigned(&node.body, key, found);
                collect_assigned(&node.orelse, key, found);
                for handler in &node.handlers {
                    let ast::ExceptHandler::ExceptHandler(handler) = handler;
                    collect_assigned(&handler.body, key, found);
                }
                &node.finalbody
            }
            Stmt::Assign(assign) => {
                let mut names = Vec::new();
                for target in &assign.targets {
                    subscript_keys(target, &mut names);
                }
                if names.iter().any(|name| name == key) {
                    if let Some(value) = string_constant(&assign.value) {
                        *found = Some(value);
                    }
                }
                continue;
            }
            _ => continue,
        };
        collect_assigned(nested, key, found);
    }
}

fn subscript_keys(expr: &ast::Expr, keys: &mut Vec<String>) {
    if let ast::Expr::Subscript(subscript) = expr {
        if let Some(key) = string_constant(&subscript.slice) {
            keys.push(key);
        }
        subscript_keys(&subscript.value, keys);
    }
}

fn string_constant(expr: &ast::Expr) -> Option<String> {
    match expr {
        ast::Expr::Constant(constant) => match &constant.value {
            ast::Constant::Str(text) => Some(text.clone()),
            _ => None,
        },
        _ => None,
    }
}

fn collect_imports(stmts: &[Stmt], module: &str, imports: &mut BTreeSet<String>) {
    for stmt in stmts {
        match stmt {
            Stmt::Import(import) => {
                for alias in &import.names {
                    imports.insert(alias.name.to_string());
                }
            }
            Stmt::ImportFrom(from) => record_from(from, module, imports),
            Stmt::FunctionDef(function) => collect_imports(&function.body, module, imports),
            Stmt::ClassDef(class) => collect_imports(&class.body, module, imports),
            Stmt::If(branch) => {
                collect_imports(&branch.body, module, imports);
                collect_imports(&branch.orelse, module, imports);
            }
            _ => {}
        }
    }
}

fn record_from(from: &ast::StmtImportFrom, module: &str, imports: &mut BTreeSet<String>) {
    let level = from
        .level
        .as_ref()
        .map(|level| level.to_usize())
        .unwrap_or(0);
    let base = if level == 0 {
        String::new()
    } else {
        relative_base(module, level)
    };
    let named = from.module.as_ref().map(|name| name.as_str()).unwrap_or("");
    let joined = match (base.is_empty(), named.is_empty()) {
        (true, _) => named.to_string(),
        (false, true) => base,
        (false, false) => format!("{base}.{named}"),
    };
    if !joined.is_empty() {
        imports.insert(joined.clone());
    }
    for alias in &from.names {
        let name = alias.name.as_str();
        if name != "*" && !joined.is_empty() {
            imports.insert(format!("{joined}.{name}"));
        }
    }
}

fn relative_base(module: &str, level: usize) -> String {
    let mut parts: Vec<&str> = module
        .rsplit_once('.')
        .map(|(package, _)| package)
        .unwrap_or("")
        .split('.')
        .filter(|part| !part.is_empty())
        .collect();
    for _ in 1..level {
        parts.pop();
    }
    parts.join(".")
}

fn collect_py(root: &Path, files: &mut Vec<PathBuf>) -> std::io::Result<()> {
    if root.is_file() {
        files.push(root.to_path_buf());
        return Ok(());
    }
    let mut entries = std::fs::read_dir(root)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort();
    for path in entries {
        if path.is_dir() {
            if path.file_name().is_some_and(|name| name == "__pycache__") {
                continue;
            }
            collect_py(&path, files)?;
        } else if path.extension().is_some_and(|ext| ext == "py") {
            files.push(path);
        }
    }
    Ok(())
}

/// Header on every generated TOML file.
pub const GENERATED_HEADER: &str = "# Generated by `eb-stack easyblock-reach`. Data only: the branch holding this file is\n# regenerated on every run and must never be merged.\n";

/// Why a reach report could not be built.
#[derive(Debug, Error)]
pub enum ReachError {
    /// A parse or argument failure, already worded for the CLI.
    #[error("{0}")]
    Message(String),
    /// Reading an easyconfig or easyblock tree failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// What to resolve.
#[derive(Debug, Clone)]
pub struct ReachQuery {
    /// Easyblock and framework roots. Earlier roots win a class name.
    pub roots: Vec<PathBuf>,
    /// Easyconfig files or directories.
    pub easyconfigs: Vec<PathBuf>,
    /// Changed path and the Python source classes are read from.
    pub changed: Vec<(PathBuf, String)>,
    /// Installed module names (`name/version[-toolchain]versionsuffix`).
    pub installed: BTreeSet<String>,
    /// Also list modules that depend on a reached module.
    pub reverse: bool,
    /// Timestamp stored on the report.
    pub generated_at: String,
}

/// One place an easyconfig uses a reached class.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReachHit {
    /// Easyconfig path.
    pub easyconfig: String,
    /// Module that easyconfig installs.
    pub module: String,
    /// Easyblock class that matched.
    pub class_name: String,
    /// `easyblock`, `implied`, `extension`, `exts_defaultclass`, `exts_classmap`, `component`, or `default_easyblock`.
    pub via: String,
    /// Extension or component name. Empty for the recipe's own class.
    pub detail: String,
    /// True when `module` is in the installed set.
    pub installed: bool,
}

/// Easyconfigs reached by a change to an easyblock or a framework module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReachReport {
    /// [`REACH_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Copied from the query.
    pub generated_at: String,
    /// Changed Python paths.
    pub changed_files: Vec<String>,
    /// Classes defined in those files, plus classes that import them.
    pub changed_classes: Vec<String>,
    /// `changed_classes` plus subclasses.
    pub reached_classes: Vec<String>,
    /// Distinct easyconfigs in `hits`.
    pub easyconfig_count: usize,
    /// Distinct installed modules in `hits`.
    pub installed_count: usize,
    /// One row per matched class on an easyconfig.
    pub hits: Vec<ReachHit>,
    /// Dependents of a hit. Empty when the query did not ask for them.
    pub reverse: Vec<ReverseRow>,
    /// Easyconfigs that could not be parsed.
    pub skipped: Vec<String>,
}

/// A module that uses a reached module and would need a rebuild.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReverseRow {
    /// Dependent module.
    pub module: String,
    /// Easyconfig that installs it, when the tree has one.
    pub easyconfig: String,
    /// runtime, builddependency, extension, or toolchain.
    pub kind: String,
    /// Intermediate modules, nearest to the reached module first.
    pub via: String,
}

/// Which easyconfigs use a reached class, by the `easyblock` line or `EB_<Name>`.
pub fn reach(query: &ReachQuery) -> Result<ReachReport, ReachError> {
    let index = index_roots(&query.roots).map_err(ReachError::Message)?;
    let mut changed = BTreeSet::new();
    let mut reached = BTreeSet::new();
    let mut changed_files = Vec::new();
    for (path, source) in &query.changed {
        let one = affected_classes(&index, path, source).map_err(ReachError::Message)?;
        changed.extend(one.changed);
        reached.extend(one.reached);
        changed_files.push(path.display().to_string());
    }
    changed_files.sort();
    let mut files = Vec::new();
    for root in &query.easyconfigs {
        collect_eb(root, &mut files)?;
    }
    let mut hits = Vec::new();
    let mut skipped = Vec::new();
    let mut recipes = Vec::new();
    let mut by_path: BTreeMap<String, ModuleKey> = BTreeMap::new();
    let mut by_key: BTreeMap<ModuleKey, String> = BTreeMap::new();
    for path in files {
        let src = std::fs::read_to_string(&path)?;
        let (mut recipe, sites) = match easyblock_sites(&src) {
            Ok(pair) => pair,
            Err(error) => {
                skipped.push(format!("{}: {error}", path.display()));
                continue;
            }
        };
        recipe.easyconfig_path = path.display().to_string();
        let key = ModuleKey::of(&recipe.to_candidate());
        let module = module_name(&key);
        push_recipe_hits(
            &index,
            &reached,
            &recipe,
            &sites,
            &module,
            &query.installed,
            &mut hits,
        );
        by_key
            .entry(key.clone())
            .or_insert_with(|| recipe.easyconfig_path.clone());
        by_path.insert(recipe.easyconfig_path.clone(), key);
        recipes.push(recipe);
    }
    skipped.sort();
    hits.sort_by(|left, right| left.easyconfig.cmp(&right.easyconfig));
    let easyconfig_count = hits
        .iter()
        .map(|hit| hit.easyconfig.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    let installed_count = hits
        .iter()
        .filter(|hit| hit.installed)
        .map(|hit| hit.module.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    let reverse = if query.reverse {
        reverse_rows(&recipes, &by_path, &by_key, &hits)
    } else {
        Vec::new()
    };
    Ok(ReachReport {
        schema_version: REACH_SCHEMA_VERSION,
        generated_at: query.generated_at.clone(),
        changed_files,
        changed_classes: changed.into_iter().collect(),
        reached_classes: reached.into_iter().collect(),
        easyconfig_count,
        installed_count,
        hits,
        reverse,
        skipped,
    })
}

fn push_recipe_hits(
    index: &EasyblockIndex,
    reached: &BTreeSet<String>,
    recipe: &ResolvedEasyconfig,
    sites: &EasyblockSites,
    module: &str,
    installed: &BTreeSet<String>,
    hits: &mut Vec<ReachHit>,
) {
    let top = recipe
        .easyblock
        .clone()
        .unwrap_or_else(|| encode_class_name(&recipe.name));
    let via = if recipe.easyblock.is_some() {
        "easyblock"
    } else {
        "implied"
    };
    consider(reached, hits, recipe, module, installed, (&top, via, ""));
    let imposed_ext = inherited(index, &top, true);
    for ext in &sites.extensions {
        let classmap = sites.exts_classmap.get(&ext.name).map(String::as_str);
        let fallback = imposed_ext
            .as_deref()
            .or(sites.exts_defaultclass.as_deref());
        if let Some((class, via)) = resolve_site(
            index,
            ext.class_name.as_deref(),
            &ext.name,
            fallback,
            ("extension", "exts_defaultclass"),
            classmap,
        ) {
            consider(
                reached,
                hits,
                recipe,
                module,
                installed,
                (&class, via, &ext.name),
            );
        }
    }
    let imposed_comp = inherited(index, &top, false);
    for comp in &sites.components {
        let fallback = sites
            .default_easyblock
            .as_deref()
            .or(imposed_comp.as_deref());
        if let Some((class, via)) = resolve_site(
            index,
            comp.class_name.as_deref(),
            &comp.name,
            fallback,
            ("component", "default_easyblock"),
            None,
        ) {
            consider(
                reached,
                hits,
                recipe,
                module,
                installed,
                (&class, via, &comp.name),
            );
        }
    }
}

fn consider(
    reached: &BTreeSet<String>,
    hits: &mut Vec<ReachHit>,
    recipe: &ResolvedEasyconfig,
    module: &str,
    installed: &BTreeSet<String>,
    site: (&str, &str, &str),
) {
    let (class_name, via, detail) = site;
    if !reached.contains(class_name) {
        return;
    }
    hits.push(ReachHit {
        easyconfig: recipe.easyconfig_path.clone(),
        module: module.to_string(),
        class_name: class_name.to_string(),
        via: via.to_string(),
        detail: detail.to_string(),
        installed: installed.contains(module),
    });
}

fn resolve_site(
    index: &EasyblockIndex,
    explicit: Option<&str>,
    name: &str,
    fallback: Option<&str>,
    vias: (&'static str, &'static str),
    classmap: Option<&str>,
) -> Option<(String, &'static str)> {
    if let Some(class) = explicit {
        return Some((class.to_string(), vias.0));
    }
    let implied = encode_class_name(name);
    if index.classes.contains_key(&implied) {
        return Some((implied, "implied"));
    }
    classmap
        .map(|class| (class.to_string(), "exts_classmap"))
        .or_else(|| fallback.map(|class| (class.to_string(), vias.1)))
}

fn inherited(index: &EasyblockIndex, start: &str, exts: bool) -> Option<String> {
    let mut stack = vec![start.to_string()];
    let mut seen = HashSet::new();
    while let Some(name) = stack.pop() {
        if !seen.insert(name.clone()) {
            continue;
        }
        let Some(fact) = index.classes.get(&name) else {
            continue;
        };
        let value = if exts {
            fact.exts_default.clone()
        } else {
            fact.component_default.clone()
        };
        if let Some(value) = value {
            return Some(value);
        }
        for base in fact.bases.iter().rev() {
            let short = base.rsplit('.').next().unwrap_or(base);
            if index.classes.contains_key(base) {
                stack.push(base.clone());
            } else if short != base && index.classes.contains_key(short) {
                stack.push(short.to_string());
            }
        }
    }
    None
}

fn reverse_rows(
    recipes: &[ResolvedEasyconfig],
    by_path: &BTreeMap<String, ModuleKey>,
    by_key: &BTreeMap<ModuleKey, String>,
    hits: &[ReachHit],
) -> Vec<ReverseRow> {
    let candidates: Vec<_> = recipes
        .iter()
        .map(ResolvedEasyconfig::to_candidate)
        .collect();
    let uses = reverse_uses(&candidates, Choice::Newest);
    let mut starts = Vec::new();
    let mut seen = HashSet::new();
    for hit in hits {
        if let Some(key) = by_path.get(&hit.easyconfig) {
            if seen.insert(key.clone()) {
                starts.push(key.clone());
            }
        }
    }
    reverse_closure(&uses, &starts)
        .into_iter()
        .map(|step| ReverseRow {
            module: module_name(&step.module),
            easyconfig: by_key.get(&step.module).cloned().unwrap_or_default(),
            kind: step.kind.to_string(),
            via: step
                .via
                .iter()
                .map(module_name)
                .collect::<Vec<_>>()
                .join(" -> "),
        })
        .collect()
}

fn collect_eb(root: &Path, files: &mut Vec<PathBuf>) -> Result<(), ReachError> {
    if root.is_file() {
        files.push(root.to_path_buf());
        return Ok(());
    }
    let mut entries = std::fs::read_dir(root)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_eb(&path, files)?;
        } else if path.extension().is_some_and(|ext| ext == "eb") {
            files.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &std::path::Path, rel: &str, text: &str) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn recipe(name: &str, version: &str, easyblock: &str) -> String {
        let line = if easyblock.is_empty() {
            String::new()
        } else {
            format!("easyblock = '{easyblock}'\n")
        };
        format!("name = '{name}'\nversion = '{version}'\ntoolchain = SYSTEM\n{line}")
    }

    #[test]
    fn subclass_implied_name_and_import_reach_recipes() {
        let root = tempfile::tempdir().unwrap();
        let py = root.path().join("py");
        let ecs = root.path().join("eb");
        write(
            &py,
            "easybuild/easyblocks/generic/configuremake.py",
            "class ConfigureMake:\n    pass\n",
        );
        write(
            &py,
            "easybuild/easyblocks/generic/cmakemake.py",
            "from easybuild.tools.filetools import read_file\nclass CMakeMake(ConfigureMake):\n    pass\n",
        );
        write(
            &py,
            "easybuild/easyblocks/g/gromacs.py",
            "class EB_GROMACS(CMakeMake):\n    pass\n",
        );
        write(
            &py,
            "easybuild/tools/filetools.py",
            "def read_file():\n    return None\n",
        );
        write(&ecs, "gromacs.eb", &recipe("GROMACS", "2024.1", ""));
        write(&ecs, "zlib.eb", &recipe("zlib", "1.3", "CMakeMake"));
        write(&ecs, "make.eb", &recipe("make", "1", "ConfigureMake"));
        write(
            &ecs,
            "wrapper.eb",
            "name = 'wrapper'\nversion = '1'\ntoolchain = SYSTEM\ndependencies = [('GROMACS', '2024.1')]\n",
        );
        let cmake = py.join("easybuild/easyblocks/generic/cmakemake.py");
        let report = reach(&ReachQuery {
            roots: vec![py.clone()],
            easyconfigs: vec![ecs.clone()],
            changed: vec![(cmake.clone(), std::fs::read_to_string(&cmake).unwrap())],
            installed: BTreeSet::from(["zlib/1.3".into()]),
            reverse: true,
            generated_at: "2026-10-22T00:00:00Z".into(),
        })
        .unwrap();
        assert!(report
            .reached_classes
            .iter()
            .any(|name| name == "EB_GROMACS"));
        let gromacs = report
            .hits
            .iter()
            .find(|hit| hit.easyconfig.ends_with("gromacs.eb"))
            .unwrap();
        assert_eq!(gromacs.via, "implied");
        assert_eq!(gromacs.class_name, "EB_GROMACS");
        assert!(!gromacs.installed);
        let zlib = report
            .hits
            .iter()
            .find(|hit| hit.easyconfig.ends_with("zlib.eb"))
            .unwrap();
        assert_eq!(zlib.via, "easyblock");
        assert!(zlib.installed);
        assert!(report
            .hits
            .iter()
            .all(|hit| !hit.easyconfig.ends_with("make.eb")));
        assert_eq!(report.easyconfig_count, 2);
        assert_eq!(report.installed_count, 1);
        let wrapper = report
            .reverse
            .iter()
            .find(|row| row.module == "wrapper/1")
            .unwrap();
        assert_eq!(wrapper.kind, "runtime");
        assert!(wrapper.via.is_empty());
        assert!(wrapper.easyconfig.ends_with("wrapper.eb"));
        let tools = py.join("easybuild/tools/filetools.py");
        let via_import = reach(&ReachQuery {
            roots: vec![py],
            easyconfigs: vec![ecs],
            changed: vec![(tools.clone(), std::fs::read_to_string(&tools).unwrap())],
            installed: BTreeSet::new(),
            reverse: false,
            generated_at: "t".into(),
        })
        .unwrap();
        assert!(via_import
            .changed_classes
            .iter()
            .any(|name| name == "CMakeMake"));
        assert!(via_import
            .hits
            .iter()
            .any(|hit| hit.easyconfig.ends_with("gromacs.eb")));
        assert!(via_import
            .hits
            .iter()
            .all(|hit| !hit.easyconfig.ends_with("make.eb")));
    }

    fn has(report: &ReachReport, spec: &str) -> bool {
        let mut parts = spec.split(' ');
        let (file, via, class_name, detail) = (
            parts.next().unwrap(),
            parts.next().unwrap(),
            parts.next().unwrap(),
            parts.next().unwrap(),
        );
        report.hits.iter().any(|hit| {
            hit.easyconfig.ends_with(file)
                && hit.via == via
                && hit.class_name == class_name
                && hit.detail == detail
        })
    }

    #[test]
    fn extensions_and_components_use_class_and_easyconfig_defaults() {
        let root = tempfile::tempdir().unwrap();
        let py = root.path().join("py");
        let ecs = root.path().join("eb");
        write(
            &py,
            "easybuild/easyblocks/generic/pythonpackage.py",
            "class PythonPackage:\n    pass\n",
        );
        write(
            &py,
            "easybuild/easyblocks/generic/pythonbundle.py",
            "class PythonBundle:\n    def extra_options():\n        extra_vars['default_easyblock'][0] = 'PythonPackage'\n    def __init__(self):\n        self.cfg['exts_defaultclass'] = 'PythonPackage'\nclass CargoPythonBundle(PythonBundle):\n    pass\nclass TensorFlow:\n    def __init__(self):\n        with self.cfg.disable_templating():\n            self.cfg['exts_defaultclass'] = 'PythonPackage'\n",
        );
        write(
            &py,
            "easybuild/easyblocks/generic/others.py",
            "class Bundle:\n    pass\nclass OctavePackage:\n    pass\nclass Cargo:\n    pass\nclass ConfigureMake:\n    pass\nclass CMakeMake(ConfigureMake):\n    pass\nclass EB_Zlib:\n    pass\n",
        );
        write(
            &ecs,
            "bundle.eb",
            "name = 'bundle'\nversion = '1'\ntoolchain = SYSTEM\neasyblock = 'PythonBundle'\nexts_list = [\n    ('numpy', '1.26'),\n    ('special', '1.0', {'easyblock': 'Cargo'}),\n]\ncomponents = [\n    ('zlib', '1.3', {'easyblock': 'CMakeMake'}),\n    ('bare', '0.1'),\n]\n",
        );
        write(
            &ecs,
            "over.eb",
            "name = 'over'\nversion = '1'\ntoolchain = SYSTEM\neasyblock = 'PythonBundle'\nexts_defaultclass = 'PerlModule'\ndefault_easyblock = 'ConfigureMake'\nexts_list = [('numpy', '1')]\ncomponents = [('bare', '1')]\n",
        );
        write(
            &ecs,
            "tf.eb",
            "name = 'tf'\nversion = '1'\ntoolchain = SYSTEM\neasyblock = 'TensorFlow'\nexts_list = [('numpy', '1')]\n",
        );
        write(
            &ecs,
            "sub.eb",
            "name = 'sub'\nversion = '1'\ntoolchain = SYSTEM\neasyblock = 'CargoPythonBundle'\nexts_list = [('numpy', '1')]\n",
        );
        write(
            &ecs,
            "octave.eb",
            "name = 'octave'\nversion = '1'\ntoolchain = SYSTEM\neasyblock = 'Bundle'\nexts_defaultclass = 'OctavePackage'\nexts_classmap = {'special': 'Cargo'}\ndefault_easyblock = 'ConfigureMake'\nexts_list = [\n    ('plain', '1'),\n    ('special', '1'),\n    ('Zlib', '1'),\n]\ncomponents = [('zlib', '1')]\n",
        );
        let pkg = py.join("easybuild/easyblocks/generic/pythonpackage.py");
        let py_hits = reach(&ReachQuery {
            roots: vec![py.clone()],
            easyconfigs: vec![ecs.clone()],
            changed: vec![(pkg.clone(), std::fs::read_to_string(&pkg).unwrap())],
            installed: BTreeSet::new(),
            reverse: false,
            generated_at: "t".into(),
        })
        .unwrap();
        assert!(has(
            &py_hits,
            "bundle.eb exts_defaultclass PythonPackage numpy"
        ));
        assert!(has(
            &py_hits,
            "bundle.eb default_easyblock PythonPackage bare"
        ));
        assert!(has(
            &py_hits,
            "over.eb exts_defaultclass PythonPackage numpy"
        ));
        assert!(has(
            &py_hits,
            "sub.eb exts_defaultclass PythonPackage numpy"
        ));
        assert!(has(&py_hits, "tf.eb exts_defaultclass PythonPackage numpy"));
        assert!(!has(
            &py_hits,
            "over.eb default_easyblock PythonPackage bare"
        ));
        assert!(!has(&py_hits, "bundle.eb extension Cargo special"));
        let others = py.join("easybuild/easyblocks/generic/others.py");
        let other_hits = reach(&ReachQuery {
            roots: vec![py],
            easyconfigs: vec![ecs],
            changed: vec![(others.clone(), std::fs::read_to_string(&others).unwrap())],
            installed: BTreeSet::new(),
            reverse: false,
            generated_at: "t".into(),
        })
        .unwrap();
        assert!(has(
            &other_hits,
            "octave.eb exts_defaultclass OctavePackage plain"
        ));
        assert!(has(&other_hits, "octave.eb exts_classmap Cargo special"));
        assert!(has(&other_hits, "octave.eb implied EB_Zlib Zlib"));
        assert!(has(
            &other_hits,
            "octave.eb default_easyblock ConfigureMake zlib"
        ));
        assert!(has(&other_hits, "bundle.eb component CMakeMake zlib"));
        assert!(has(&other_hits, "bundle.eb extension Cargo special"));
        assert!(has(
            &other_hits,
            "over.eb default_easyblock ConfigureMake bare"
        ));
    }
}
