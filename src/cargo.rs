//! Offline Cargo.toml / crates.io adapter.
//!
//! Same leftover model as PyPI: existing robot `Rust` / `maturin` modules
//! are provides. A crate that is not in the robot becomes its own recipe
//! (`PythonPackage` when it binds Python via PyO3/maturin, otherwise
//! `Crate`). The adapter never invents a SHA-256.

use crate::ecosystem::nonempty;
use crate::foreign::{
    ForeignDep, ForeignError, ForeignFormat, ForeignRecipe, ForeignResidual, ForeignSource,
};
use crate::package::{ConditionExpr, ResidualSeverity};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

/// Shell prelude that isolates host Cargo/rustc when EasyBuild runs cargo
/// inside EESSI-extend.
///
/// Host `sccache`, `clang`, and `mold` are not in the EESSI module graph.
/// foss wrappers can drop the compat `ld` that `collect2` needs. The cargo
/// target triple and compat arch come from the build host (`rustc -vV`,
/// `uname -m`), not from a plan-time x86_64 literal. An empty `EESSI_VERSION`
/// skips the compat PATH and `-B` prepend; it does not default a version and
/// must not abort the chain. Cargo leftovers and mesonpy wraps that invoke
/// cargo both emit this string.
pub fn eessi_cargo_host_isolation() -> &'static str {
    concat!(
        "unset RUSTC_WRAPPER CARGO_BUILD_RUSTC_WRAPPER RUSTC_WORKSPACE_WRAPPER RUSTFLAGS CARGO_ENCODED_RUSTFLAGS && ",
        r#"eval $(env | awk -F= '/^CARGO_TARGET_[A-Z0-9_]*_(LINKER|RUSTFLAGS)=/{print "unset "$1}') && "#,
        "export CARGO_HOME=%(builddir)s/.cargohome && ",
        "export LINKER=${CC:-gcc} && ",
        r#"_triple=$(rustc -vV 2>/dev/null | awk "/^host:/{print \$2}") && "#,
        r#"_triple_env=$(printf %s "$_triple" | tr "[:lower:]-" "[:upper:]_") && "#,
        r#"if [ -n "$_triple_env" ]; then eval export CARGO_TARGET_${_triple_env}_LINKER=${CC:-gcc}; fi && "#,
        r#"_arch=$(uname -m) && "#,
        r#"if [ -n "${EESSI_VERSION:-}" ]; then "#,
        r#"_ebld=/cvmfs/software.eessi.io/versions/${EESSI_VERSION}/compat/linux/${_arch}/usr/bin; "#,
        r#"export PATH="$_ebld:$PATH"; "#,
        r#"_bflag="-C link-arg=-B$_ebld/"; "#,
        r#"else _ebld=; _bflag=; fi && "#,
        r#"_libflags=$( [ -n "${LIBRARY_PATH:-}" ] && printf -- '-L %s ' $(echo "$LIBRARY_PATH" | tr ':' ' '); : ) && "#,
        r#"export RUSTFLAGS="${_bflag} ${_libflags}" && "#,
    )
}

/// Parse a Cargo.toml document or a crates.io API JSON document.
pub fn parse_cargo_str(text: &str) -> Result<ForeignRecipe, ForeignError> {
    let trimmed = text.trim();
    if trimmed.starts_with('{') {
        parse_crates_io_json(trimmed)
    } else {
        parse_cargo_toml(trimmed)
    }
}

fn parse_cargo_toml(text: &str) -> Result<ForeignRecipe, ForeignError> {
    let value: toml::Value = toml::from_str(text)
        .map_err(|error| ForeignError::Parse(format!("cargo toml: {error}")))?;
    let package = value
        .get("package")
        .ok_or_else(|| ForeignError::Parse("cargo toml missing [package]".into()))?;
    let name = toml_string(package, "name")
        .ok_or_else(|| ForeignError::Parse("cargo toml missing package.name".into()))?;
    let version =
        toml_inherit_string(package, value.get("workspace"), "version").ok_or_else(|| {
            ForeignError::Parse(
                "cargo toml package.version is missing or workspace-inherited".into(),
            )
        })?;
    let homepage = toml_string(package, "homepage").or_else(|| toml_string(package, "repository"));
    let summary = toml_string(package, "description");
    let license = toml_string(package, "license");
    let crate_deps = cargo_deps(&value);
    let python = is_python_crate(&value, &crate_deps);
    recipe(CrateFields {
        raw_name: name,
        version,
        homepage,
        summary,
        license,
        source_url: None,
        source_filename: None,
        sha256: None,
        python,
        crate_deps,
        module_name: maturin_module_name(&value),
        note: "parsed from Cargo.toml",
    })
}

fn parse_crates_io_json(text: &str) -> Result<ForeignRecipe, ForeignError> {
    if let Ok(doc) = serde_json::from_str::<CratesIoDocument>(text) {
        if !doc.versions.is_empty() {
            let version = pick_crates_io_version(&doc)
                .ok_or_else(|| ForeignError::Parse("crates.io json has no versions".into()))?;
            return crates_io_recipe(&doc.krate, version);
        }
    }
    let value: Value = serde_json::from_str(text)
        .map_err(|error| ForeignError::Parse(format!("crates.io json: {error}")))?;
    parse_crates_io_version_document(&value)
}

fn parse_crates_io_version_document(value: &Value) -> Result<ForeignRecipe, ForeignError> {
    let version: CratesIoVersion =
        serde_json::from_value(value.get("version").cloned().ok_or_else(|| {
            ForeignError::Parse("crates.io version document missing version".into())
        })?)
        .map_err(|error| ForeignError::Parse(format!("crates.io json: {error}")))?;
    let name = version
        .krate
        .clone()
        .or_else(|| {
            value
                .pointer("/version/crate")
                .and_then(|value| value.as_str())
                .map(str::to_string)
        })
        .ok_or_else(|| {
            ForeignError::Parse("crates.io version document missing crate name".into())
        })?;
    let krate = CratesIoCrate {
        name,
        id: String::new(),
        description: version.description.clone(),
        homepage: version.homepage.clone(),
        repository: version.repository.clone(),
        max_version: None,
        max_stable_version: None,
    };
    crates_io_recipe(&krate, &version)
}

fn pick_crates_io_version(doc: &CratesIoDocument) -> Option<&CratesIoVersion> {
    let wanted = nonempty(doc.krate.max_stable_version.clone())
        .or_else(|| nonempty(doc.krate.max_version.clone()));
    if let Some(wanted) = wanted {
        if let Some(entry) = doc.versions.iter().find(|entry| entry.num == wanted) {
            return Some(entry);
        }
    }
    doc.versions
        .iter()
        .filter(|entry| !entry.num.contains('-'))
        .max_by(|left, right| crate::version::cmp_version(&left.num, &right.num))
        .or_else(|| doc.versions.first())
}

fn crates_io_recipe(
    krate: &CratesIoCrate,
    version: &CratesIoVersion,
) -> Result<ForeignRecipe, ForeignError> {
    let url = nonempty(version.url.clone()).unwrap_or_else(|| {
        format!(
            "https://static.crates.io/crates/{}/{}-{}.crate",
            krate.name, krate.name, version.num
        )
    });
    let filename = nonempty(version.filename.clone())
        .unwrap_or_else(|| format!("{}-{}.crate", krate.name, version.num));
    let python = version_is_python_crate(&krate.name, version);
    recipe(CrateFields {
        raw_name: krate.name.clone(),
        version: version.num.clone(),
        homepage: nonempty(krate.homepage.clone()).or_else(|| nonempty(krate.repository.clone())),
        summary: nonempty(krate.description.clone()),
        license: nonempty(version.license.clone()),
        source_url: Some(url),
        source_filename: Some(filename),
        sha256: crates_io_sha256(version.checksum.as_deref()),
        python,
        crate_deps: crates_io_deps(version),
        module_name: None,
        note: "parsed from crates.io JSON",
    })
}

fn version_is_python_crate(crate_name: &str, version: &CratesIoVersion) -> bool {
    if crate::provides::is_python_marker_crate(crate_name) {
        return false;
    }
    if version_has_required_python_marker(version) {
        return true;
    }
    crates_io_default_enables_python(&version.features)
        || version
            .deps
            .iter()
            .chain(version.dependencies.iter())
            .any(|dep| {
                dep.optional
                    && dep.is_runtime_or_build()
                    && dep
                        .crate_name()
                        .is_some_and(|name| default_feature_items_enable(&version.features, name))
                    && dep
                        .crate_name()
                        .is_some_and(crate::provides::is_python_marker_crate)
            })
}

fn version_has_required_python_marker(version: &CratesIoVersion) -> bool {
    version
        .deps
        .iter()
        .chain(version.dependencies.iter())
        .any(|dep| {
            !dep.optional
                && dep.is_runtime_or_build()
                && dep
                    .crate_name()
                    .is_some_and(crate::provides::is_python_marker_crate)
        })
}

fn crates_io_default_enables_python(features: &BTreeMap<String, Vec<String>>) -> bool {
    default_feature_walk(features, is_python_feature_item)
}

fn default_feature_items_enable(features: &BTreeMap<String, Vec<String>>, dep_name: &str) -> bool {
    default_feature_walk(features, |item| feature_names_dep(item, dep_name))
}

fn default_feature_walk(
    features: &BTreeMap<String, Vec<String>>,
    mut matches: impl FnMut(&str) -> bool,
) -> bool {
    let Some(default) = features.get("default") else {
        return false;
    };
    default.iter().any(|feat| {
        matches(feat)
            || features
                .get(feat)
                .is_some_and(|items| items.iter().any(|item| matches(item)))
    })
}

#[derive(Debug, Deserialize)]
struct CratesIoDocument {
    #[serde(rename = "crate")]
    krate: CratesIoCrate,
    #[serde(default)]
    versions: Vec<CratesIoVersion>,
}

#[derive(Debug, Deserialize)]
struct CratesIoCrate {
    name: String,
    #[serde(default)]
    #[allow(dead_code)]
    id: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    homepage: Option<String>,
    #[serde(default)]
    repository: Option<String>,
    #[serde(default)]
    max_version: Option<String>,
    #[serde(default)]
    max_stable_version: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CratesIoVersion {
    num: String,
    #[serde(default)]
    checksum: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    filename: Option<String>,
    #[serde(default)]
    deps: Vec<CratesIoDep>,
    #[serde(default)]
    dependencies: Vec<CratesIoDep>,
    #[serde(default)]
    features: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    license: Option<String>,
    #[serde(default, rename = "crate")]
    krate: Option<String>,
    #[serde(default)]
    homepage: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    repository: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CratesIoDep {
    #[serde(default)]
    name: Option<String>,
    #[serde(default, rename = "crate_id")]
    crate_id: Option<String>,
    #[serde(default)]
    optional: bool,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    req: Option<String>,
}

fn crates_io_deps(version: &CratesIoVersion) -> Vec<CargoDep> {
    version
        .deps
        .iter()
        .chain(version.dependencies.iter())
        .filter_map(|dep| {
            let name = dep.crate_name()?.to_string();
            Some(CargoDep {
                key: name.clone(),
                name,
                req: dep.req.as_deref().and_then(cargo_version_req),
                kind: CargoDepKind::Registry,
                optional: dep.optional,
            })
        })
        .collect()
}

impl CratesIoDep {
    fn crate_name(&self) -> Option<&str> {
        self.name.as_deref().or(self.crate_id.as_deref())
    }

    fn is_runtime_or_build(&self) -> bool {
        matches!(
            self.kind.as_deref(),
            None | Some("") | Some("normal") | Some("build")
        )
    }
}

/// One crate's metadata, from Cargo.toml or from the crates.io index.
///
/// The two inputs describe the same thing with different fields present, so
/// they meet here rather than in an eleven-argument call whose order is the
/// only thing keeping the strings apart.
struct CrateFields<'a> {
    raw_name: String,
    version: String,
    homepage: Option<String>,
    summary: Option<String>,
    license: Option<String>,
    source_url: Option<String>,
    source_filename: Option<String>,
    sha256: Option<String>,
    python: bool,
    crate_deps: Vec<CargoDep>,
    module_name: Option<String>,
    note: &'a str,
}

fn recipe(fields: CrateFields<'_>) -> Result<ForeignRecipe, ForeignError> {
    let CrateFields {
        raw_name,
        version,
        homepage,
        summary,
        license,
        source_url,
        source_filename,
        sha256,
        python,
        crate_deps,
        module_name,
        note,
    } = fields;
    let mut residuals = Vec::new();
    // A PyO3 crate publishes under a crate name and imports under a module
    // name. The manifest can state it; otherwise the mapping is policy data.
    let module_name = module_name.or_else(|| {
        python
            .then(|| crate::provides::python_module_for_crate(&raw_name))
            .flatten()
    });
    let name = if let Some(module_name) = module_name {
        residuals.push(ForeignResidual {
            category: "cargo-python-name".into(),
            severity: ResidualSeverity::Judgment,
            summary: format!("PyO3 crate {raw_name} is the Python module {module_name}"),
            evidence: Some(raw_name.clone()),
            provenance: None,
        });
        module_name
    } else {
        raw_name
    };
    let mut dependencies = vec![
        ForeignDep {
            name: "Rust".into(),
            pin: None,
            role: "build".into(),
            original_spec: Some("Rust (implicit for Cargo leftovers)".into()),
            condition: ConditionExpr::Always,
            provenance: Vec::new(),
        },
        ForeignDep {
            name: "binutils".into(),
            pin: None,
            role: "build".into(),
            original_spec: Some("binutils (ld for cargo build scripts)".into()),
            condition: ConditionExpr::Always,
            provenance: Vec::new(),
        },
    ];
    if python {
        dependencies.push(ForeignDep {
            name: "Python".into(),
            pin: None,
            role: "run".into(),
            original_spec: Some("Python (implicit for PyO3/maturin crates)".into()),
            condition: ConditionExpr::Always,
            provenance: Vec::new(),
        });
        dependencies.push(ForeignDep {
            name: "maturin".into(),
            pin: None,
            role: "build".into(),
            original_spec: Some("maturin (implicit for PyO3 crates)".into()),
            condition: ConditionExpr::Always,
            provenance: Vec::new(),
        });
    }
    // Crate dependencies are linked out of the crate graph rather than loaded
    // as modules, so they are recorded rather than solved. What they say still
    // matters to a reviewer: the requirement, and whether the crate can build
    // from its published tarball at all.
    for dep in crate_deps {
        let requirement = dep
            .req
            .as_deref()
            .map_or_else(|| "unconstrained".to_string(), |req| format!("`{req}`"));
        let (category, severity, summary) = if dep.optional {
            (
                "cargo-optional-dep",
                ResidualSeverity::Mechanical,
                format!(
                    "Cargo dependency {} {requirement} is optional and stays off the required crate graph",
                    dep.name
                ),
            )
        } else {
            match dep.kind {
                CargoDepKind::Registry => (
                    "cargo-dep",
                    ResidualSeverity::Mechanical,
                    format!(
                        "Cargo dependency {} {requirement} stays inside the crate graph",
                        dep.name
                    ),
                ),
                CargoDepKind::Path => (
                    "cargo-path-dep",
                    ResidualSeverity::Judgment,
                    format!(
                        "Cargo dependency {} is a path dependency {requirement}: it is not in the \
                     published crate, so the released tarball cannot build on its own",
                        dep.name
                    ),
                ),
                CargoDepKind::Git => (
                    "cargo-git-dep",
                    ResidualSeverity::Judgment,
                    format!(
                        "Cargo dependency {} is a git dependency {requirement}: the build would \
                     fetch it, which an offline build cannot do",
                        dep.name
                    ),
                ),
            }
        };
        residuals.push(ForeignResidual {
            category: category.into(),
            severity,
            summary,
            evidence: Some(dep.name),
            provenance: None,
        });
    }
    let sources = if source_url.is_some() || sha256.is_some() {
        vec![ForeignSource {
            url: source_url.clone(),
            filename: source_filename.clone(),
            sha256: sha256.clone(),
            git: None,
            tag: None,
            commit: None,
            target_directory: None,
            condition: ConditionExpr::Always,
        }]
    } else {
        Vec::new()
    };
    let mut build_system_hints = vec!["cargo".into()];
    if python {
        build_system_hints.extend(["python".into(), "maturin".into(), "pip".into()]);
    } else {
        build_system_hints.push("crate".into());
    }
    Ok(ForeignRecipe {
        format: ForeignFormat::Cargo,
        name,
        version,
        homepage,
        source_url,
        source_filename,
        sha256,
        sources,
        summary,
        description: None,
        license,
        dependencies,
        build_system_hints,
        configopts: None,
        patches: Vec::new(),
        variants: Vec::new(),
        rules: Vec::new(),
        notes: vec![note.into()],
        residuals,
        classifiers: Vec::new(),
    })
}

fn maturin_module_name(value: &toml::Value) -> Option<String> {
    let maturin = value
        .get("package")
        .and_then(|package| package.get("metadata"))
        .and_then(|meta| meta.get("maturin"))?;
    toml_string(maturin, "module-name").or_else(|| toml_string(maturin, "name"))
}

fn is_python_crate(value: &toml::Value, deps: &[CargoDep]) -> bool {
    if value
        .get("package")
        .and_then(|package| toml_string(package, "name"))
        .or_else(|| toml_string(value, "name"))
        .is_some_and(|name| crate::provides::is_python_marker_crate(&name))
    {
        return false;
    }
    if value
        .get("package")
        .and_then(|package| package.get("metadata"))
        .and_then(|meta| meta.get("maturin"))
        .is_some()
    {
        return true;
    }
    if toml_default_enables_python(value) {
        return true;
    }
    deps.iter().any(|dep| {
        crate::provides::is_python_marker_crate(&dep.name)
            && (!dep.optional || default_features_enable(value, dep))
    })
}

fn toml_default_enables_python(value: &toml::Value) -> bool {
    let Some(features) = value.get("features").and_then(toml::Value::as_table) else {
        return false;
    };
    let Some(default) = features.get("default").and_then(toml::Value::as_array) else {
        return false;
    };
    default.iter().filter_map(toml::Value::as_str).any(|feat| {
        is_python_feature_item(feat)
            || features
                .get(feat)
                .and_then(toml::Value::as_array)
                .is_some_and(|items| {
                    items
                        .iter()
                        .filter_map(toml::Value::as_str)
                        .any(is_python_feature_item)
                })
    })
}

fn default_features_enable(value: &toml::Value, dep: &CargoDep) -> bool {
    let Some(features) = value.get("features").and_then(toml::Value::as_table) else {
        return false;
    };
    let Some(default) = features.get("default").and_then(toml::Value::as_array) else {
        return false;
    };
    let names = [dep.key.as_str(), dep.name.as_str()];
    default.iter().filter_map(toml::Value::as_str).any(|feat| {
        names.iter().any(|name| feature_names_dep(feat, name))
            || features
                .get(feat)
                .and_then(toml::Value::as_array)
                .is_some_and(|items| {
                    items
                        .iter()
                        .filter_map(toml::Value::as_str)
                        .any(|item| names.iter().any(|name| feature_names_dep(item, name)))
                })
    })
}

/// Cargo feature item that names a dependency: `name`, `dep:name`, or `name/feat`.
fn feature_crate_name(item: &str) -> &str {
    let item = item.strip_prefix("dep:").unwrap_or(item);
    let crate_name = item.split_once('/').map_or(item, |(name, _)| name);
    crate_name.strip_suffix('?').unwrap_or(crate_name)
}

fn feature_names_dep(item: &str, dep_name: &str) -> bool {
    feature_crate_name(item) == dep_name
}

fn is_python_feature_item(item: &str) -> bool {
    let name = feature_crate_name(item);
    crate::provides::is_python_marker_crate(name) || name.eq_ignore_ascii_case("extension-module")
}

/// Where a Cargo dependency comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CargoDepKind {
    /// A crates.io release, resolvable from the published tarball.
    Registry,
    /// A sibling directory. Not in the published crate, so the tarball cannot
    /// build on its own.
    Path,
    /// A git revision, which the build would have to fetch.
    Git,
}

/// One Cargo dependency, with the requirement the manifest states.
#[derive(Debug, Clone)]
struct CargoDep {
    /// Table key Cargo uses in `[features]` (`package =` does not change it).
    key: String,
    name: String,
    /// The requirement, translated into the shared grammar.
    req: Option<String>,
    kind: CargoDepKind,
    optional: bool,
}

/// Cargo's bare version string is a caret requirement.
///
/// `serde = "1.0"` means `^1.0`, not `==1.0`. An explicit operator is already
/// in the shared grammar and passes through.
fn cargo_version_req(spec: &str) -> Option<String> {
    let spec = spec.trim();
    if spec.is_empty() || spec == "*" {
        return None;
    }
    if spec.contains(',') {
        let parts: Vec<String> = spec
            .split(',')
            .filter_map(|part| cargo_version_req(part.trim()))
            .collect();
        return (!parts.is_empty()).then_some(parts.join(","));
    }
    if spec.starts_with(['=', '>', '<', '^', '~']) {
        return Some(spec.to_string());
    }
    if let Some(prefix) = spec.strip_suffix(".*") {
        if prefix.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            return Some(format!(">={prefix}"));
        }
    }
    if spec.starts_with(|character: char| character.is_ascii_digit()) {
        return Some(format!("^{spec}"));
    }
    None
}

fn cargo_deps(value: &toml::Value) -> Vec<CargoDep> {
    let mut deps = Vec::new();
    collect_dep_table(value.get("dependencies"), value, &mut deps);
    collect_dep_table(value.get("build-dependencies"), value, &mut deps);
    if let Some(targets) = value.get("target").and_then(toml::Value::as_table) {
        for (cfg, spec) in targets {
            if target_cfg_is_non_unix(cfg) {
                continue;
            }
            collect_dep_table(spec.get("dependencies"), value, &mut deps);
            collect_dep_table(spec.get("build-dependencies"), value, &mut deps);
        }
    }
    deps
}

fn target_cfg_is_non_unix(cfg: &str) -> bool {
    let lower = cfg.to_ascii_lowercase();
    lower.contains("windows")
        || lower.contains("win32")
        || lower.contains("macos")
        || lower.contains("osx")
}

fn collect_dep_table(table: Option<&toml::Value>, root: &toml::Value, deps: &mut Vec<CargoDep>) {
    let Some(map) = table.and_then(toml::Value::as_table) else {
        return;
    };
    for (name, spec) in map {
        deps.push(cargo_dep_from_spec(name, spec, root));
    }
}

fn cargo_dep_from_spec(name: &str, spec: &toml::Value, root: &toml::Value) -> CargoDep {
    match spec {
        toml::Value::String(version) => CargoDep {
            key: name.to_string(),
            name: name.to_string(),
            req: cargo_version_req(version),
            kind: CargoDepKind::Registry,
            optional: false,
        },
        toml::Value::Table(entry) => {
            let crate_name = entry
                .get("package")
                .and_then(toml::Value::as_str)
                .unwrap_or(name)
                .to_string();
            let kind = if entry.contains_key("path") {
                CargoDepKind::Path
            } else if entry.contains_key("git") {
                CargoDepKind::Git
            } else {
                CargoDepKind::Registry
            };
            let workspace = entry
                .get("workspace")
                .and_then(toml::Value::as_bool)
                .unwrap_or(false);
            if workspace {
                if let Some(inherited) = root
                    .get("workspace")
                    .and_then(|workspace| workspace.get("dependencies"))
                    .and_then(|dependencies| dependencies.get(name))
                {
                    let mut inherited = cargo_dep_from_spec(name, inherited, root);
                    inherited.key = name.to_string();
                    inherited.optional = entry
                        .get("optional")
                        .and_then(toml::Value::as_bool)
                        .unwrap_or(inherited.optional);
                    if entry.get("package").and_then(toml::Value::as_str).is_some() {
                        inherited.name = crate_name;
                    }
                    return inherited;
                }
            }
            CargoDep {
                key: name.to_string(),
                name: crate_name,
                req: entry
                    .get("version")
                    .and_then(toml::Value::as_str)
                    .and_then(cargo_version_req),
                kind,
                optional: entry
                    .get("optional")
                    .and_then(toml::Value::as_bool)
                    .unwrap_or(false),
            }
        }
        _ => CargoDep {
            key: name.to_string(),
            name: name.to_string(),
            req: None,
            kind: CargoDepKind::Registry,
            optional: false,
        },
    }
}

fn crates_io_sha256(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.len() == 64 && value.chars().all(|character| character.is_ascii_hexdigit()) {
        Some(value.to_ascii_lowercase())
    } else {
        None
    }
}

fn toml_inherit_string(
    table: &toml::Value,
    workspace: Option<&toml::Value>,
    key: &str,
) -> Option<String> {
    if let Some(value) = toml_string(table, key) {
        return Some(value);
    }
    let inherited = table
        .get(key)
        .and_then(toml::Value::as_table)
        .and_then(|entry| entry.get("workspace"))
        .and_then(toml::Value::as_bool)
        .unwrap_or(false);
    if !inherited {
        return None;
    }
    workspace
        .and_then(|workspace| workspace.get("package"))
        .and_then(|package| toml_string(package, key))
}

fn toml_string(value: &toml::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(toml::Value::as_str)
        .map(ToString::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cargo_toml_pyo3_is_python_package() {
        let recipe = parse_cargo_str(
            r#"
[package]
name = "readcon-core"
version = "0.13.1"
description = "CON reader"
license = "MIT"
repository = "https://github.com/lode-org/readcon-core"

[dependencies]
pyo3 = "0.22"
"#,
        )
        .expect("parse");
        assert_eq!(recipe.name, "readcon");
        assert_eq!(recipe.version, "0.13.1");
        assert!(recipe.dependencies.iter().any(|dep| dep.name == "Rust"));
        assert!(recipe.dependencies.iter().any(|dep| dep.name == "Python"));
        assert!(recipe.dependencies.iter().any(|dep| dep.name == "maturin"));
        assert!(recipe
            .build_system_hints
            .iter()
            .any(|hint| hint == "maturin"));
    }

    #[test]
    fn crates_io_json_reads_checksum() {
        let recipe = parse_cargo_str(
            r#"{
              "crate": {
                "id": "demo",
                "name": "demo",
                "max_version": "1.2.3",
                "max_stable_version": "1.2.3",
                "description": "demo crate",
                "repository": "https://example.invalid/demo"
              },
              "versions": [{
                "num": "1.2.3",
                "checksum": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
              }]
            }"#,
        )
        .expect("parse");
        assert_eq!(recipe.name, "demo");
        assert_eq!(recipe.version, "1.2.3");
        assert_eq!(
            recipe.sha256.as_deref(),
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );
        assert!(!recipe.dependencies.iter().any(|dep| dep.name == "Python"));
    }

    #[test]
    fn same_document_workspace_inherit_is_applied() {
        let recipe = parse_cargo_str(
            r#"
[workspace.package]
version = "1.2.3"

[workspace.dependencies]
serde = "1.0"
core = { path = "crates/core" }

[package]
name = "demo"
version.workspace = true

[dependencies]
serde = { workspace = true }
core = { workspace = true }
"#,
        )
        .expect("parse");
        assert_eq!(recipe.version, "1.2.3");
        assert!(
            recipe.residuals.iter().any(|residual| {
                residual.category == "cargo-dep"
                    && residual.summary.contains("serde")
                    && residual.summary.contains("`^1.0`")
            }),
            "{:?}",
            recipe.residuals
        );
        assert!(
            recipe
                .residuals
                .iter()
                .any(|residual| residual.category == "cargo-path-dep"
                    && residual.summary.contains("core")),
            "{:?}",
            recipe.residuals
        );
    }

    #[test]
    fn default_extension_module_feature_is_a_python_crate() {
        let recipe = parse_cargo_str(
            r#"
[package]
name = "demo"
version = "1.0.0"

[dependencies]
pyo3 = { version = "0.22", optional = true }

[features]
default = ["extension-module"]
extension-module = ["pyo3/extension-module"]
"#,
        )
        .expect("parse");
        assert!(
            recipe.dependencies.iter().any(|dep| dep.name == "Python"),
            "{:?}",
            recipe.dependencies
        );
        assert!(
            recipe.dependencies.iter().any(|dep| dep.name == "maturin"),
            "{:?}",
            recipe.dependencies
        );
    }

    #[test]
    fn windows_target_pyo3_does_not_make_a_python_crate() {
        let recipe = parse_cargo_str(
            r#"
[package]
name = "demo"
version = "1.0.0"

[target.'cfg(windows)'.dependencies]
pyo3 = "0.22"
"#,
        )
        .expect("parse");
        assert!(
            recipe
                .dependencies
                .iter()
                .all(|dep| dep.name != "Python" && dep.name != "maturin"),
            "{:?}",
            recipe.dependencies
        );
    }

    #[test]
    fn default_enabled_optional_pyo3_is_a_python_crate() {
        let recipe = parse_cargo_str(
            r#"
[package]
name = "demo"
version = "1.0.0"

[dependencies]
pyo3 = { version = "0.22", optional = true }

[features]
default = ["pyo3"]
"#,
        )
        .expect("parse");
        assert!(
            recipe.dependencies.iter().any(|dep| dep.name == "Python"),
            "{:?}",
            recipe.dependencies
        );
        assert!(
            recipe.dependencies.iter().any(|dep| dep.name == "maturin"),
            "{:?}",
            recipe.dependencies
        );
        assert!(
            recipe
                .build_system_hints
                .iter()
                .any(|hint| hint == "maturin"),
            "{:?}",
            recipe.build_system_hints
        );
    }

    #[test]
    fn default_enabled_pyo3_feature_path_is_a_python_crate() {
        let recipe = parse_cargo_str(
            r#"
[package]
name = "demo"
version = "1.0.0"

[dependencies]
pyo3 = { version = "0.22", optional = true }

[features]
default = ["extension-module"]
extension-module = ["pyo3/extension-module"]
"#,
        )
        .expect("parse");
        assert!(
            recipe.dependencies.iter().any(|dep| dep.name == "Python"),
            "{:?}",
            recipe.dependencies
        );
        assert!(
            recipe.dependencies.iter().any(|dep| dep.name == "maturin"),
            "{:?}",
            recipe.dependencies
        );
        assert!(
            recipe
                .build_system_hints
                .iter()
                .any(|hint| hint == "maturin"),
            "{:?}",
            recipe.build_system_hints
        );
    }

    #[test]
    fn empty_default_features_leave_optional_pyo3_as_a_crate() {
        let recipe = parse_cargo_str(
            r#"
[package]
name = "demo"
version = "1.0.0"

[dependencies]
pyo3 = { version = "0.22", optional = true }

[features]
default = []
extension-module = ["pyo3/extension-module"]
"#,
        )
        .expect("parse");
        assert!(
            recipe
                .dependencies
                .iter()
                .all(|dep| dep.name != "Python" && dep.name != "maturin"),
            "{:?}",
            recipe.dependencies
        );
        assert!(
            recipe
                .build_system_hints
                .iter()
                .all(|hint| hint != "maturin" && hint != "python"),
            "{:?}",
            recipe.build_system_hints
        );
    }

    #[test]
    fn package_rename_default_feature_key_enables_pyo3() {
        let recipe = parse_cargo_str(
            r#"
[package]
name = "demo"
version = "1.0.0"

[dependencies]
py = { package = "pyo3", version = "0.22", optional = true }

[features]
default = ["py"]
"#,
        )
        .expect("parse");
        assert!(
            recipe.dependencies.iter().any(|dep| dep.name == "Python"),
            "{:?}",
            recipe.dependencies
        );
        assert!(
            recipe.dependencies.iter().any(|dep| dep.name == "maturin"),
            "{:?}",
            recipe.dependencies
        );
        assert!(
            recipe
                .build_system_hints
                .iter()
                .any(|hint| hint == "maturin"),
            "{:?}",
            recipe.build_system_hints
        );
    }

    #[test]
    fn optional_pyo3_does_not_make_a_python_crate() {
        let recipe = parse_cargo_str(
            r#"
[package]
name = "demo"
version = "1.0.0"

[dependencies]
pyo3 = { version = "0.22", optional = true }
"#,
        )
        .expect("parse");
        assert!(
            recipe
                .dependencies
                .iter()
                .all(|dep| dep.name != "Python" && dep.name != "maturin"),
            "{:?}",
            recipe.dependencies
        );
        assert!(
            recipe
                .residuals
                .iter()
                .any(|residual| residual.category == "cargo-optional-dep"
                    && residual.summary.contains("pyo3")),
            "{:?}",
            recipe.residuals
        );
    }

    #[test]
    fn optional_cargo_deps_are_not_required_residuals() {
        let recipe = parse_cargo_str(
            r#"
[package]
name = "demo"
version = "1.0.0"

[dependencies]
serde = { version = "1.0", optional = true }
"#,
        )
        .expect("parse");
        assert!(
            recipe
                .residuals
                .iter()
                .any(|residual| residual.category == "cargo-optional-dep"
                    && residual.summary.contains("serde")),
            "{:?}",
            recipe.residuals
        );
        assert!(
            recipe
                .residuals
                .iter()
                .all(|residual| residual.category != "cargo-dep"
                    || !residual.summary.contains("serde")),
            "{:?}",
            recipe.residuals
        );
    }

    #[test]
    fn crates_io_checksum_must_be_sixty_four_hex() {
        let recipe = parse_cargo_str(
            r#"{
              "crate": {"id": "demo", "name": "demo", "max_version": "1.0.0"},
              "versions": [{"num": "1.0.0", "checksum": "not-a-digest"}]
            }"#,
        )
        .expect("parse");
        assert_eq!(recipe.sha256, None);
        assert!(recipe.sources.iter().all(|source| source.sha256.is_none()));
    }

    #[test]
    fn eessi_cargo_isolation_is_host_derived_not_x86_64() {
        let prelude = eessi_cargo_host_isolation();
        assert!(prelude.contains("unset RUSTC_WRAPPER"));
        assert!(prelude.contains("uname -m"));
        assert!(prelude.contains("compat/linux/${_arch}/usr/bin"));
        assert!(
            prelude.contains("link-arg=-B$_ebld/"),
            "GCC -B prefix must end with /:\n{prelude}"
        );
        assert!(prelude.contains("LINKER=${CC:-gcc}"));
        assert!(
            !prelude.contains("X86_64"),
            "triple and compat arch are runtime, not a plan-time x86_64 literal:\n{prelude}"
        );
        assert!(
            !prelude.contains("2025.06"),
            "compat version comes from EESSI_VERSION, not a default:\n{prelude}"
        );
        assert!(
            prelude.contains("LIBRARY_PATH"),
            "empty LIBRARY_PATH must not always emit -L:\n{prelude}"
        );
    }

    #[test]
    fn eessi_cargo_isolation_survives_unset_eessi_version() {
        let prelude = eessi_cargo_host_isolation();
        assert!(
            !prelude.contains("EESSI_VERSION:?") && !prelude.contains("${EESSI_VERSION:?"),
            "unset EESSI_VERSION must not abort:\n{prelude}"
        );
        assert!(
            !prelude.contains("2025.06"),
            "compat version comes from EESSI_VERSION, not a default:\n{prelude}"
        );
        let script = format!(
            "{}echo SURVIVED",
            prelude.replace("%(builddir)s", "/tmp/eb-stack-cargo-iso")
        );
        let output = std::process::Command::new("bash")
            .arg("-c")
            .arg(&script)
            .env_remove("EESSI_VERSION")
            .env_remove("LIBRARY_PATH")
            .output()
            .expect("bash");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success() && stdout.contains("SURVIVED"),
            "status={} stdout={stdout:?} stderr={stderr:?}",
            output.status
        );
    }

    #[test]
    fn eessi_cargo_isolation_survives_unset_library_path() {
        let prelude =
            eessi_cargo_host_isolation().replace("%(builddir)s", "/tmp/eb-stack-cargo-iso");
        let script = format!("{prelude}echo SURVIVED");
        let output = std::process::Command::new("bash")
            .arg("-c")
            .arg(&script)
            .env("EESSI_VERSION", "2025.06")
            .env_remove("LIBRARY_PATH")
            .output()
            .expect("bash");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success() && stdout.contains("SURVIVED"),
            "status={} stdout={stdout:?} stderr={stderr:?}",
            output.status
        );
    }

    #[test]
    fn crates_io_feature_keys_are_not_required_python_deps() {
        let recipe = parse_cargo_str(
            r#"{
              "crate": {
                "id": "pyo3",
                "name": "pyo3",
                "max_version": "0.22.0",
                "max_stable_version": "0.22.0"
              },
              "versions": [{
                "num": "0.22.0",
                "features": { "extension-module": [], "abi3": [] }
              }]
            }"#,
        )
        .expect("parse");
        assert!(
            recipe
                .dependencies
                .iter()
                .all(|dep| dep.name != "Python" && dep.name != "maturin"),
            "{:?}",
            recipe.dependencies
        );
        assert!(
            recipe
                .build_system_hints
                .iter()
                .all(|hint| hint != "maturin" && hint != "python"),
            "{:?}",
            recipe.build_system_hints
        );
    }

    #[test]
    fn crates_io_default_feature_pyo3_without_deps_is_python() {
        let recipe = parse_cargo_str(
            r#"{
              "crate": {
                "id": "demo",
                "name": "demo",
                "max_version": "1.0.0"
              },
              "versions": [{
                "num": "1.0.0",
                "features": { "default": ["pyo3"], "pyo3": [] }
              }]
            }"#,
        )
        .expect("parse");
        assert!(
            recipe.dependencies.iter().any(|dep| dep.name == "Python"),
            "{:?}",
            recipe.dependencies
        );
        assert!(
            recipe.dependencies.iter().any(|dep| dep.name == "maturin"),
            "{:?}",
            recipe.dependencies
        );
        assert!(
            recipe
                .build_system_hints
                .iter()
                .any(|hint| hint == "maturin"),
            "{:?}",
            recipe.build_system_hints
        );
    }

    #[test]
    fn crates_io_default_extension_module_without_deps_is_python() {
        let recipe = parse_cargo_str(
            r#"{
              "crate": {
                "id": "demo",
                "name": "demo",
                "max_version": "1.0.0"
              },
              "versions": [{
                "num": "1.0.0",
                "features": { "default": ["extension-module"], "extension-module": [] }
              }]
            }"#,
        )
        .expect("parse");
        assert!(
            recipe.dependencies.iter().any(|dep| dep.name == "Python"),
            "{:?}",
            recipe.dependencies
        );
        assert!(
            recipe.dependencies.iter().any(|dep| dep.name == "maturin"),
            "{:?}",
            recipe.dependencies
        );
        assert!(
            recipe
                .build_system_hints
                .iter()
                .any(|hint| hint == "maturin"),
            "{:?}",
            recipe.build_system_hints
        );
    }

    #[test]
    fn crates_io_pyo3_crate_default_macros_stays_a_crate() {
        let recipe = parse_cargo_str(
            r#"{
              "crate": {
                "id": "pyo3",
                "name": "pyo3",
                "max_version": "0.22.0",
                "max_stable_version": "0.22.0"
              },
              "versions": [{
                "num": "0.22.0",
                "features": { "default": ["macros"], "macros": [], "extension-module": [] }
              }]
            }"#,
        )
        .expect("parse");
        assert!(
            recipe
                .dependencies
                .iter()
                .all(|dep| dep.name != "Python" && dep.name != "maturin"),
            "{:?}",
            recipe.dependencies
        );
        assert!(
            recipe
                .build_system_hints
                .iter()
                .all(|hint| hint != "maturin" && hint != "python"),
            "{:?}",
            recipe.build_system_hints
        );
    }

    #[test]
    fn pyo3_with_required_pyo3_ffi_stays_a_crate() {
        let toml = parse_cargo_str(
            r#"
[package]
name = "pyo3"
version = "0.22.0"

[dependencies]
pyo3-ffi = "0.22"
"#,
        )
        .expect("toml");
        assert!(
            toml.dependencies
                .iter()
                .all(|dep| dep.name != "Python" && dep.name != "maturin"),
            "{:?}",
            toml.dependencies
        );

        let json = parse_cargo_str(
            r#"{
              "crate": { "id": "pyo3", "name": "pyo3", "max_version": "0.22.0" },
              "versions": [{
                "num": "0.22.0",
                "deps": [{ "name": "pyo3-ffi", "optional": false, "kind": "normal" }]
              }]
            }"#,
        )
        .expect("json");
        assert!(
            json.dependencies
                .iter()
                .all(|dep| dep.name != "Python" && dep.name != "maturin"),
            "{:?}",
            json.dependencies
        );
    }

    #[test]
    fn crates_io_version_document_keeps_homepage_and_summary() {
        let recipe = parse_cargo_str(
            r#"{
              "version": {
                "crate": "serde",
                "num": "1.0.210",
                "homepage": "https://serde.rs",
                "description": "A generic serialization/deserialization framework",
                "checksum": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
              }
            }"#,
        )
        .expect("parse");
        assert_eq!(recipe.homepage.as_deref(), Some("https://serde.rs"));
        assert_eq!(
            recipe.summary.as_deref(),
            Some("A generic serialization/deserialization framework")
        );
    }

    #[test]
    fn crates_io_version_document_records_declared_deps() {
        let recipe = parse_cargo_str(
            r#"{
              "version": {
                "crate": "demo",
                "num": "1.0.0",
                "deps": [{ "name": "serde", "optional": false, "kind": "normal", "req": "^1.0" }]
              }
            }"#,
        )
        .expect("parse");
        assert!(
            recipe.residuals.iter().any(|residual| {
                residual.category == "cargo-dep" && residual.summary.contains("serde")
            }),
            "{:?}",
            recipe.residuals
        );
    }

    #[test]
    fn crates_io_required_marker_dep_is_python() {
        let recipe = parse_cargo_str(
            r#"{
              "crate": {
                "id": "demo",
                "name": "demo",
                "max_version": "1.0.0"
              },
              "versions": [{
                "num": "1.0.0",
                "deps": [{ "name": "pyo3", "optional": false, "kind": "normal" }]
              }]
            }"#,
        )
        .expect("parse");
        assert!(
            recipe.dependencies.iter().any(|dep| dep.name == "Python"),
            "{:?}",
            recipe.dependencies
        );
        assert!(
            recipe.dependencies.iter().any(|dep| dep.name == "maturin"),
            "{:?}",
            recipe.dependencies
        );
    }

    #[test]
    fn crates_io_pinned_version_document_parses() {
        let recipe = parse_cargo_str(
            r#"{
              "version": {
                "crate": "demo",
                "num": "1.2.3",
                "license": "MIT",
                "checksum": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
              }
            }"#,
        )
        .expect("pinned version document");
        assert_eq!(recipe.name, "demo");
        assert_eq!(recipe.version, "1.2.3");
        assert_eq!(recipe.license.as_deref(), Some("MIT"));
    }

    #[test]
    fn cargo_toml_skips_dev_dependencies() {
        let recipe = parse_cargo_str(
            r#"
[package]
name = "demo"
version = "1.0.0"

[dev-dependencies]
pyo3 = "0.22"
"#,
        )
        .expect("parse");
        assert!(
            !recipe.dependencies.iter().any(|dep| dep.name == "Python"),
            "dev-only pyo3 must not classify the crate as PythonPackage"
        );
    }

    #[test]
    fn maturin_module_name_is_the_recipe_name() {
        let recipe = parse_cargo_str(
            r#"
[package]
name = "readcon-core"
version = "0.13.1"

[package.metadata.maturin]
module-name = "readcon"
"#,
        )
        .expect("parse");
        assert_eq!(recipe.name, "readcon");
    }

    #[test]
    fn cargo_version_req_keeps_comma_and_wildcard_floors() {
        assert_eq!(
            cargo_version_req(">=1.0, <2.0").as_deref(),
            Some(">=1.0,<2.0")
        );
        assert_eq!(cargo_version_req("1.*").as_deref(), Some(">=1"));
    }
}
