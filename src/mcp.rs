//! MCP stdio surface for the version-one package, recipe, stack, target, and campaign workflows.

use crate::campaign::{
    claim_finding, resolve_finding, run_campaign, CampaignRequest, CampaignStatus,
    FindingResolution,
};
use crate::domain::Toolchain;
use crate::eb_parse::{
    check_recipe_deps, packaging_gate, parse_easyconfig_trees, resolve_easyconfig_file,
};
use crate::eb_style::{format_style_file, lint_style};
use crate::foreign::ForeignFormat;
use crate::package::{StackPolicy, STACK_POLICY_SCHEMA_VERSION};
use crate::package_catalog::{resolve_package_catalog_layers, PackageCatalogLayer};
use crate::package_closure::{plan_package_closure, write_package_closure};
use crate::package_config::PackageConfigLayer;
use crate::package_workflow::{
    inspect_new_package, plan_new_package, plan_package_bump, write_package_bundle,
    BumpPackageRequest, NewPackageRequest, PackageBundle,
};
use crate::target::{doctor_target, resolve_target_layers, BuildTarget, TargetConfigLayer};
use crate::{
    load_json_file, solve_from_easyconfigs_with_baseline_version_and_extras, SolveExtraOut,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

pub fn run_server<R: BufRead, W: Write>(reader: R, mut writer: W) -> std::io::Result<()> {
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(&line) {
            Ok(message) => handle_message(&message),
            Err(error) => Some(json_rpc_error(
                Value::Null,
                -32700,
                format!("parse error: {error}"),
            )),
        };
        if let Some(response) = response {
            serde_json::to_writer(&mut writer, &response)?;
            writer.write_all(b"\n")?;
            writer.flush()?;
        }
    }
    Ok(())
}

pub fn handle_message(message: &Value) -> Option<Value> {
    let id = message.get("id").cloned().unwrap_or(Value::Null);
    let method = message.get("method").and_then(Value::as_str).unwrap_or("");
    match method {
        "notifications/initialized" | "notifications/cancelled" => None,
        "initialize" => Some(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": "2024-11-05",
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": "eb-stack", "version": env!("CARGO_PKG_VERSION")}
            }
        })),
        "ping" => Some(json!({"jsonrpc": "2.0", "id": id, "result": {}})),
        "tools/list" => Some(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {"tools": tool_catalog()}
        })),
        "tools/call" => {
            let params = message.get("params").cloned().unwrap_or_else(|| json!({}));
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let result = call_tool(name, &arguments);
            Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": match result {
                    Ok(value) => tool_success(value),
                    Err(error) => tool_error(error),
                }
            }))
        }
        _ if message.get("id").is_none() => None,
        _ => Some(json_rpc_error(
            id,
            -32601,
            format!("method not found: {method}"),
        )),
    }
}

fn tool_catalog() -> Vec<Value> {
    vec![
        tool_with_optional(
            "eb_package_inspect",
            "Parse a conda-forge or Spack recipe into a canonical build manifest and planned CycloneDX SBOM.",
            &[
                ("source", "string"),
                ("toolchain_version", "string"),
                ("out_dir", "string"),
            ],
            &[
                ("format", "string"),
                ("toolchain_name", "string"),
                ("package_configs", "array"),
            ],
        ),
        tool_with_optional(
            "eb_package_plan",
            "Resolve package profiles with Resolvo and emit manifest, SBOM, profile locks, and EasyBuild recipes.",
            &[
                ("source", "string"),
                ("toolchain_version", "string"),
                ("easyconfigs", "array"),
                ("stack_policy", "string"),
                ("out_dir", "string"),
            ],
            &[
                ("format", "string"),
                ("toolchain_name", "string"),
                ("package_configs", "array"),
                ("source_checksums", "array"),
                ("package_catalogs", "array"),
            ],
        ),
        tool(
            "eb_package_bump",
            "Retarget an EasyBuild recipe and emit its canonical SBOM, Resolvo lock, and recipe bundle.",
            &[
                ("source", "string"),
                ("toolchain_name", "string"),
                ("toolchain_version", "string"),
                ("easyconfigs", "array"),
                ("out_dir", "string"),
            ],
        ),
        tool(
            "eb_recipe_check",
            "Check EasyBuild packaging metadata and robot dependency resolution.",
            &[("recipe", "string"), ("easyconfigs", "array")],
        ),
        tool(
            "eb_recipe_format",
            "Mechanically format EasyBuild E501 findings and report any residual lines.",
            &[("recipe", "string")],
        ),
        tool(
            "eb_stack_solve",
            "Solve a jointly consistent EasyBuild stack lock and optional reports.",
            &[
                ("easyconfigs", "array"),
                ("policy", "string"),
                ("lock_out", "string"),
            ],
        ),
        tool(
            "eb_target_list",
            "Resolve and list layered public build-target TOML configuration.",
            &[("configs", "array")],
        ),
        tool(
            "eb_target_doctor",
            "Check target transport, executor, runtime, and EasyBuild command layers.",
            &[("configs", "array"), ("target", "string")],
        ),
        tool(
            "eb_campaign_run",
            "Start or resume a persisted build campaign on a named target.",
            &[
                ("bundle", "string"),
                ("configs", "array"),
                ("target", "string"),
                ("state", "string"),
            ],
        ),
        tool(
            "eb_campaign_status",
            "Read persisted campaign findings and claim ladder.",
            &[("state", "string")],
        ),
        tool(
            "eb_campaign_finding_claim",
            "Claim one open campaign finding for an OMP worker.",
            &[("state", "string"), ("id", "string"), ("owner", "string")],
        ),
        tool(
            "eb_campaign_finding_resolve",
            "Resolve an owned campaign finding with durable action and evidence.",
            &[
                ("state", "string"),
                ("id", "string"),
                ("owner", "string"),
                ("action", "string"),
                ("evidence", "string"),
                ("changes", "array"),
            ],
        ),
    ]
}

fn tool(name: &str, description: &str, required: &[(&str, &str)]) -> Value {
    tool_with_optional(name, description, required, &[])
}

fn tool_with_optional(
    name: &str,
    description: &str,
    required: &[(&str, &str)],
    optional: &[(&str, &str)],
) -> Value {
    let properties = required
        .iter()
        .chain(optional.iter())
        .map(|(name, kind)| {
            let schema = if *kind == "array" {
                json!({"type": "array", "items": {"type": "string"}})
            } else {
                json!({"type": kind})
            };
            ((*name).to_string(), schema)
        })
        .collect::<serde_json::Map<_, _>>();
    json!({
        "name": name,
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": properties,
            "required": required.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
            "additionalProperties": true
        }
    })
}

fn call_tool(name: &str, arguments: &Value) -> Result<Value, String> {
    match name {
        "eb_package_inspect" => package_inspect(arguments),
        "eb_package_plan" => package_plan(arguments),
        "eb_package_bump" => package_bump(arguments),
        "eb_recipe_check" => recipe_check(arguments),
        "eb_recipe_format" => recipe_format(arguments),
        "eb_stack_solve" => stack_solve(arguments),
        "eb_target_list" => target_list(arguments),
        "eb_target_doctor" => target_doctor(arguments),
        "eb_campaign_run" => campaign_run(arguments),
        "eb_campaign_status" => campaign_status(arguments),
        "eb_campaign_finding_claim" => campaign_finding_claim(arguments),
        "eb_campaign_finding_resolve" => campaign_finding_resolve(arguments),
        _ => Err(format!("unknown tool: {name}")),
    }
}

fn package_inspect(arguments: &Value) -> Result<Value, String> {
    let source = required_path(arguments, "source")?;
    let toolchain = toolchain(arguments)?;
    let configs = package_layers(arguments)?;
    let (plan, sbom) =
        inspect_new_package(&source, foreign_format(arguments)?, &toolchain, &configs)
            .map_err(|error| error.to_string())?;
    let output = required_path(arguments, "out_dir")?;
    let written = write_package_bundle(
        &PackageBundle {
            plan: plan.clone(),
            sbom,
            locks: Vec::new(),
            easyconfigs: Vec::new(),
        },
        &output,
    )
    .map_err(|error| error.to_string())?;
    Ok(json!({
        "package": plan.package.name,
        "version": plan.package.version,
        "manifest": written.manifest,
        "sbom": written.sbom,
        "claims": {"resolves": false, "builds": false, "binary_verified": false}
    }))
}

fn package_plan(arguments: &Value) -> Result<Value, String> {
    let stack_policy = load_stack_policy(&required_path(arguments, "stack_policy")?)?;
    let request = NewPackageRequest {
        source: required_path(arguments, "source")?,
        format: foreign_format(arguments)?,
        toolchain: toolchain(arguments)?,
        source_checksums: string_array(arguments, "source_checksums")?,
        package_layers: package_layers(arguments)?,
        easyconfig_roots: path_array(arguments, "easyconfigs")?,
        stack_policy,
    };
    let output = required_path(arguments, "out_dir")?;
    let catalog_paths = string_array(arguments, "package_catalogs")?
        .into_iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    if catalog_paths.is_empty() {
        let bundle = plan_new_package(&request).map_err(|error| error.to_string())?;
        let written = write_package_bundle(&bundle, &output).map_err(|error| error.to_string())?;
        return Ok(json!({
            "package": bundle.plan.package.name,
            "version": bundle.plan.package.version,
            "manifest": written.manifest,
            "sbom": written.sbom,
            "locks": written.locks,
            "easyconfigs": written.easyconfigs,
            "patches": written.patches,
            "claims": {"resolves": true, "builds": false, "binary_verified": false}
        }));
    }

    let mut layers = Vec::with_capacity(catalog_paths.len());
    for path in catalog_paths {
        layers.push(PackageCatalogLayer::from_path(&path).map_err(|error| error.to_string())?);
    }
    let catalog = resolve_package_catalog_layers(&layers).map_err(|error| error.to_string())?;
    let closure = plan_package_closure(&request, &catalog).map_err(|error| error.to_string())?;
    let package = closure.root.plan.package.name.clone();
    let version = closure.root.plan.package.version.clone();
    let written = write_package_closure(&closure, &output).map_err(|error| error.to_string())?;
    let companions = written
        .companions
        .iter()
        .map(|companion| {
            json!({
                "manifest": companion.manifest,
                "sbom": companion.sbom,
                "locks": companion.locks,
                "easyconfigs": companion.easyconfigs,
                "patches": companion.patches,
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "package": package,
        "version": version,
        "manifest": written.root.manifest,
        "sbom": written.root.sbom,
        "locks": written.root.locks,
        "easyconfigs": written.root.easyconfigs,
        "patches": written.root.patches,
        "companions": companions,
        "closure_plan": written.closure_plan,
        "closure_sbom": written.closure_sbom,
        "build_order": written.build_order,
        "claims": {"resolves": true, "builds": false, "binary_verified": false}
    }))
}

fn package_bump(arguments: &Value) -> Result<Value, String> {
    let target = toolchain(arguments)?;
    let stack_policy = if let Some(path) = optional_path(arguments, "stack_policy") {
        load_stack_policy(&path)?
    } else {
        StackPolicy {
            schema_version: STACK_POLICY_SCHEMA_VERSION,
            name: "unconstrained".into(),
            toolchain: target.clone(),
            pins: Vec::new(),
            exclusions: Vec::new(),
        }
    };
    let bundle = plan_package_bump(&BumpPackageRequest {
        source: required_path(arguments, "source")?,
        toolchain: target,
        version: optional_string(arguments, "version"),
        source_checksum: optional_string(arguments, "source_checksum"),
        easyconfig_roots: path_array(arguments, "easyconfigs")?,
        hierarchy_fixture: optional_path(arguments, "hierarchy_fixture"),
        overrides: string_map(arguments, "dependencies")?,
        stack_policy,
    })
    .map_err(|error| error.to_string())?;
    let written = write_package_bundle(&bundle, &required_path(arguments, "out_dir")?)
        .map_err(|error| error.to_string())?;
    Ok(json!({
        "package": bundle.plan.package.name,
        "version": bundle.plan.package.version,
        "manifest": written.manifest,
        "sbom": written.sbom,
        "locks": written.locks,
        "easyconfigs": written.easyconfigs,
        "claims": {"resolves": true, "builds": false, "binary_verified": false}
    }))
}

fn recipe_check(arguments: &Value) -> Result<Value, String> {
    let recipe = resolve_easyconfig_file(&required_path(arguments, "recipe")?)
        .map_err(|error| error.to_string())?;
    let roots = path_array(arguments, "easyconfigs")?;
    let root_refs = roots.iter().map(PathBuf::as_path).collect::<Vec<_>>();
    let tree = parse_easyconfig_trees(&root_refs).map_err(|error| error.to_string())?;
    let check = check_recipe_deps(&recipe, &tree.candidates);
    let required_options = string_array(arguments, "require_configopts")?;
    let option_refs = required_options
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let packaging = packaging_gate(&recipe, &option_refs);
    let resolves = check.ok() && packaging.is_ok();
    let packaging_errors = packaging.err().unwrap_or_default();
    Ok(json!({
        "recipe": recipe.easyconfig_path,
        "resolves": resolves,
        "dependency_check": check,
        "packaging_errors": packaging_errors,
        "claims": {"resolves": resolves, "builds": false, "binary_verified": false}
    }))
}

fn recipe_format(arguments: &Value) -> Result<Value, String> {
    let recipe = required_path(arguments, "recipe")?;
    let before = std::fs::read_to_string(&recipe).map_err(|error| error.to_string())?;
    let before_findings = lint_style(&before);
    let result = format_style_file(&recipe, optional_path(arguments, "out").as_deref())
        .map_err(|error| error.to_string())?;
    Ok(json!({
        "recipe": recipe,
        "before": before_findings,
        "rewritten": result.lines_rewritten,
        "remaining": result.remaining
    }))
}

fn stack_solve(arguments: &Value) -> Result<Value, String> {
    let roots = path_array(arguments, "easyconfigs")?;
    let root_refs = roots.iter().map(PathBuf::as_path).collect::<Vec<_>>();
    let policy = required_path(arguments, "policy")?;
    let lock_out = required_path(arguments, "lock_out")?;
    let baseline = optional_path(arguments, "baseline_easyconfigs");
    let lock = solve_from_easyconfigs_with_baseline_version_and_extras(
        &root_refs,
        &policy,
        baseline.as_deref(),
        optional_string(arguments, "baseline_toolchain_version").as_deref(),
        &lock_out,
        optional_path(arguments, "sbom_out").as_deref(),
        SolveExtraOut {
            build_list_out: optional_path(arguments, "build_list_out").as_deref(),
            stack_diff_out: optional_path(arguments, "stack_diff_out").as_deref(),
        },
    )
    .map_err(|error| error.to_string())?;
    Ok(json!({
        "lock": lock_out,
        "packages": lock.packages.len(),
        "solver": lock.solver.engine,
        "claims": {"resolves": true, "builds": false, "binary_verified": false}
    }))
}

fn target_list(arguments: &Value) -> Result<Value, String> {
    serde_json::to_value(load_targets(arguments)?).map_err(|error| error.to_string())
}

fn target_doctor(arguments: &Value) -> Result<Value, String> {
    let name = required_string(arguments, "target")?;
    let targets = load_targets(arguments)?;
    let target = targets
        .iter()
        .find(|candidate| candidate.name == name)
        .ok_or_else(|| format!("target {name} is not configured"))?;
    let report = doctor_target(target).map_err(|error| error.to_string())?;
    Ok(json!({"target": report.target, "ok": report.ok(), "checks": report.checks}))
}

fn campaign_run(arguments: &Value) -> Result<Value, String> {
    let name = required_string(arguments, "target")?;
    let target = load_targets(arguments)?
        .into_iter()
        .find(|candidate| candidate.name == name)
        .ok_or_else(|| format!("target {name} is not configured"))?;
    let state = run_campaign(&CampaignRequest {
        bundle: required_path(arguments, "bundle")?,
        target,
        state_path: required_path(arguments, "state")?,
    })
    .map_err(|error| error.to_string())?;
    Ok(json!({
        "ok": state.status != CampaignStatus::Failed,
        "state": state,
    }))
}

fn campaign_status(arguments: &Value) -> Result<Value, String> {
    let state = required_path(arguments, "state")?;
    load_json_file(&state).map_err(|error| error.to_string())
}

fn campaign_finding_claim(arguments: &Value) -> Result<Value, String> {
    let state = claim_finding(
        &required_path(arguments, "state")?,
        &required_string(arguments, "id")?,
        &required_string(arguments, "owner")?,
    )
    .map_err(|error| error.to_string())?;
    serde_json::to_value(state).map_err(|error| error.to_string())
}

fn campaign_finding_resolve(arguments: &Value) -> Result<Value, String> {
    let state = resolve_finding(
        &required_path(arguments, "state")?,
        &required_string(arguments, "id")?,
        &required_string(arguments, "owner")?,
        FindingResolution {
            action: required_string(arguments, "action")?,
            evidence: required_string(arguments, "evidence")?,
            changes: string_array(arguments, "changes")?,
        },
    )
    .map_err(|error| error.to_string())?;
    serde_json::to_value(state).map_err(|error| error.to_string())
}

fn load_targets(arguments: &Value) -> Result<Vec<BuildTarget>, String> {
    let layers = path_array(arguments, "configs")?
        .iter()
        .map(|path| TargetConfigLayer::from_path(path).map_err(|error| error.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    resolve_target_layers(&layers).map_err(|error| error.to_string())
}

fn package_layers(arguments: &Value) -> Result<Vec<PackageConfigLayer>, String> {
    string_array(arguments, "package_configs")?
        .iter()
        .map(|path| {
            PackageConfigLayer::from_path(Path::new(path)).map_err(|error| error.to_string())
        })
        .collect()
}

fn load_stack_policy(path: &Path) -> Result<StackPolicy, String> {
    let text = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    if path.extension().and_then(|value| value.to_str()) == Some("json") {
        serde_json::from_str(&text).map_err(|error| error.to_string())
    } else {
        toml::from_str(&text).map_err(|error| error.to_string())
    }
}

fn toolchain(arguments: &Value) -> Result<Toolchain, String> {
    Ok(Toolchain {
        name: optional_string(arguments, "toolchain_name").unwrap_or_else(|| "foss".into()),
        version: required_string(arguments, "toolchain_version")?,
    })
}

fn foreign_format(arguments: &Value) -> Result<Option<ForeignFormat>, String> {
    match optional_string(arguments, "format")
        .as_deref()
        .unwrap_or("auto")
    {
        "auto" => Ok(None),
        "conda" | "conda-forge" => Ok(Some(ForeignFormat::CondaForge)),
        "spack" => Ok(Some(ForeignFormat::Spack)),
        value => Err(format!("unsupported foreign format {value}")),
    }
}

fn required_string(arguments: &Value, name: &str) -> Result<String, String> {
    arguments
        .get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("missing string argument {name}"))
}

fn optional_string(arguments: &Value, name: &str) -> Option<String> {
    arguments
        .get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn required_path(arguments: &Value, name: &str) -> Result<PathBuf, String> {
    required_string(arguments, name).map(PathBuf::from)
}

fn optional_path(arguments: &Value, name: &str) -> Option<PathBuf> {
    optional_string(arguments, name).map(PathBuf::from)
}

fn path_array(arguments: &Value, name: &str) -> Result<Vec<PathBuf>, String> {
    let values = string_array(arguments, name)?;
    if values.is_empty() {
        return Err(format!("argument {name} requires at least one path"));
    }
    Ok(values.into_iter().map(PathBuf::from).collect())
}

fn string_array(arguments: &Value, name: &str) -> Result<Vec<String>, String> {
    match arguments.get(name) {
        None => Ok(Vec::new()),
        Some(Value::Array(values)) => values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_string)
                    .ok_or_else(|| format!("argument {name} must contain strings"))
            })
            .collect(),
        Some(_) => Err(format!("argument {name} must be an array")),
    }
}

fn string_map(arguments: &Value, name: &str) -> Result<HashMap<String, String>, String> {
    match arguments.get(name) {
        None => Ok(HashMap::new()),
        Some(Value::Object(values)) => values
            .iter()
            .map(|(key, value)| {
                value
                    .as_str()
                    .map(|value| (key.clone(), value.to_string()))
                    .ok_or_else(|| format!("argument {name}.{key} must be a string"))
            })
            .collect(),
        Some(_) => Err(format!("argument {name} must be an object")),
    }
}

fn tool_success(value: Value) -> Value {
    json!({
        "content": [{"type": "text", "text": serde_json::to_string_pretty(&value).unwrap_or_default()}],
        "structuredContent": value,
        "isError": false
    })
}

fn tool_error(error: String) -> Value {
    json!({
        "content": [{"type": "text", "text": error}],
        "isError": true
    })
}

fn json_rpc_error(id: Value, code: i64, message: String) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}
