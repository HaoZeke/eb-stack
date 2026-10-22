//! Which easyblock classes a changed Python file reaches.
//!
//! The class hierarchy is the `class` statement, not an import of the base:
//! `EB_GROMACS(CMakeMake)` is reached by a change to `CMakeMake`. A framework
//! module reaches the easyblocks that import it, and then their subclasses.
//! Easyconfig matching is layered on top of this index.

use rustpython_parser::ast::{self, Stmt};
use rustpython_parser::Parse;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

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

/// Classes a changed file defines, classes that import its module, and subclasses.
pub fn affected_classes(
    index: &EasyblockIndex,
    path: &Path,
    source: &str,
) -> Result<BTreeSet<String>, String> {
    let module = python_module(path);
    let (classes, _) = read_python(source, &path.display().to_string(), module.as_deref())?;
    let mut direct: BTreeSet<String> = classes.into_iter().map(|(name, _)| name).collect();
    if let Some(module) = module {
        if let Some(users) = index.importers.get(&module) {
            direct.extend(users.iter().cloned());
        }
    }
    let mut reached = direct.clone();
    for name in &direct {
        reached.extend(descendants(index, name));
    }
    Ok(reached)
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
    for stmt in stmts {
        let body: &[Stmt] = match stmt {
            Stmt::FunctionDef(function) => &function.body,
            _ => continue,
        };
        for inner in body {
            let Stmt::Assign(assign) = inner else {
                continue;
            };
            let mut names = Vec::new();
            for target in &assign.targets {
                subscript_keys(target, &mut names);
            }
            if names.iter().any(|name| name == key) {
                if let Some(value) = string_constant(&assign.value) {
                    found = Some(value);
                }
            }
        }
    }
    found
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
