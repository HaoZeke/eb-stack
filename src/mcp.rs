//! MCP (Model Context Protocol) stdio server for eb-stack.
//!
//! Speaks newline-delimited JSON-RPC 2.0 on stdin/stdout, the transport every
//! MCP client (claude, codex, omp, grok) uses for local servers. Exposes the
//! packaging workflow as typed tools so an agent driver gets the same
//! machine-checked contract as the CLI, with the reporting ladder and the
//! next-action instructions embedded in every result instead of relying on
//! prose instructions the model may override.
//!
//! Tools:
//! - `eb_check_recipe`: robot completeness + packaging gates for one recipe.
//! - `eb_bump`: retarget a recipe to another toolchain generation.
//! - `eb_solve`: SAT co-select a stack lock from easyconfig trees + policy.
//! - `eb_ingest`: conda-forge/Spack → EB scaffold + residual-queue JSON
//!   (not a landable PR; establishes no claim rungs by itself).
//!
//! The protocol subset implemented: `initialize`, `ping`, `tools/list`,
//! `tools/call`; notifications are consumed without replies. That is the
//! complete surface a tools-only stdio server needs.

use crate::eb_emit::{
    emit_next_generation_auto_from_path_with_opts, emit_next_generation_from_path,
    AutoResolveOpts, EmitParams,
};
use crate::eb_parse::{
    check_recipe_deps, packaging_gate, parse_easyconfig_trees, resolve_easyconfig_file,
    RecipeDepCheck,
};
use crate::{
    solve_from_easyconfigs_with_baseline_version_and_extras, SolveExtraOut, Toolchain,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

const PROTOCOL_VERSION: &str = "2024-11-05";

/// The reporting contract embedded in every tool result: which claim this
/// tool run establishes and which it cannot.
fn ladder(resolves: bool) -> Value {
    json!({
        "resolves": resolves,
        "builds": "not-established-by-this-tool (requires a green `eb --robot` run on a build machine)",
        "binary_verified": "not-established-by-this-tool (requires `env -i <bin> --version` + ldd on the installed module)",
        "reporting_rule": "State only the rung you executed. A passing check-recipe means `resolves`, nothing more."
    })
}

/// Run the stdio server loop. Generic over reader/writer so tests can drive
/// it with in-memory buffers.
pub fn run_server<R: BufRead, W: Write>(reader: R, mut writer: W) -> std::io::Result<()> {
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let resp = json!({
                    "jsonrpc": "2.0",
                    "id": Value::Null,
                    "error": {"code": -32700, "message": format!("parse error: {e}")}
                });
                writeln!(writer, "{resp}")?;
                writer.flush()?;
                continue;
            }
        };
        if let Some(resp) = handle_message(&msg) {
            writeln!(writer, "{resp}")?;
            writer.flush()?;
        }
    }
    Ok(())
}

/// Dispatch one JSON-RPC message. Returns `None` for notifications (no id).
pub fn handle_message(msg: &Value) -> Option<Value> {
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let id = msg.get("id").cloned();
    // Notifications (no id) never get a reply, whatever the method.
    let id = match id {
        Some(v) if !v.is_null() => v,
        _ => return None,
    };
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(PROTOCOL_VERSION),
            "capabilities": {"tools": {}},
            "serverInfo": {
                "name": "eb-stack",
                "version": env!("CARGO_PKG_VERSION"),
            }
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": tool_definitions()})),
        "tools/call" => Ok(handle_tool_call(&params)),
        other => Err(json!({
            "code": -32601,
            "message": format!("method not found: {other}")
        })),
    };
    let mut resp = json!({"jsonrpc": "2.0", "id": id});
    match result {
        Ok(r) => resp["result"] = r,
        Err(e) => resp["error"] = e,
    }
    Some(resp)
}

fn tool_definitions() -> Value {
    json!([
        {
            "name": "eb_check_recipe",
            "description": "Packaging check for one easyconfig recipe: resolves every runtime/build dep against robot tree(s), runs the packaging gates (positional checksum lint, required configopts). Establishes ONLY the `resolves` rung of the reporting ladder. Missing-dep reasons name the nearest generations where a dep exists; treat them as the work queue.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "recipe": {"type": "string", "description": "Path to the .eb recipe to validate"},
                    "easyconfigs": {
                        "type": "array", "items": {"type": "string"},
                        "description": "Robot easyconfig tree(s); later paths override earlier (draft overlay on upstream)"
                    },
                    "require_configopts": {
                        "type": "array", "items": {"type": "string"},
                        "description": "Substrings that must appear in configopts"
                    },
                    "scaffold_missing": {
                        "type": "string",
                        "description": "Optional directory: write draft companion .eb files for every missing dep (letter/name layout) to fill and re-check"
                    }
                },
                "required": ["recipe", "easyconfigs"]
            }
        },
        {
            "name": "eb_bump",
            "description": "Retarget an easyconfig to another toolchain generation. With `easyconfigs` set, dependency versions auto-resolve hierarchy-aware from the robot tree; hand `deps` overrides win. Never guesses versions: unresolvable deps fail with nearest-generation hints unless keep_old_deps is set. Writes the conventionally named .eb and returns its path plus warnings that must be surfaced.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "source": {"type": "string", "description": "Source .eb to rewrite"},
                    "toolchain_name": {"type": "string"},
                    "toolchain_version": {"type": "string"},
                    "version": {"type": "string", "description": "Optional new application version"},
                    "source_checksum": {"type": "string", "description": "sha256 for the new source tarball on a version bump"},
                    "deps": {
                        "type": "object", "additionalProperties": {"type": "string"},
                        "description": "Dependency version overrides, name -> version"
                    },
                    "easyconfigs": {"type": "string", "description": "Robot tree for hierarchy-aware auto-resolve of dep versions"},
                    "hierarchy_fixture": {"type": "string", "description": "Optional toolchain-hierarchy fixture JSON (escape hatch; default derives from the robot tree)"},
                    "keep_old_deps": {"type": "boolean", "description": "Keep source dep versions when auto-resolve finds no candidate instead of failing"},
                    "out_dir": {"type": "string", "description": "Directory to write the conventional basename under"},
                    "out": {"type": "string", "description": "Explicit output file path (overrides out_dir)"}
                },
                "required": ["source", "toolchain_name", "toolchain_version"]
            }
        },
        {
            "name": "eb_solve",
            "description": "SAT co-select a full stack lock from easyconfig tree(s) and a policy JSON (toolchain, roots, pins). Optionally writes a planned CycloneDX SBOM, a dependency-ordered build list, and a markdown stack diff vs a baseline generation.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "easyconfigs": {
                        "type": "array", "items": {"type": "string"},
                        "description": "Easyconfig tree(s), later paths override earlier"
                    },
                    "policy": {"type": "string", "description": "Policy JSON path"},
                    "baseline_easyconfigs": {"type": "string"},
                    "baseline_toolchain_version": {"type": "string"},
                    "lock_out": {"type": "string", "description": "Lock output path (default stack.lock.json)"},
                    "sbom_out": {"type": "string"},
                    "build_list_out": {"type": "string"},
                    "stack_diff_out": {"type": "string"}
                },
                "required": ["easyconfigs", "policy"]
            }
        },
        {
            "name": "eb_ingest",
            "description": "Ingest a foreign conda-forge or Spack recipe into a parseable EasyBuild scaffold. Optional robot trees fill generation-native dep versions (hierarchy + resolvo). Writes the .eb and a residual-queue JSON for judgment work. Does NOT claim a landable PR, builds, or product configopts — residuals stay in the queue.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "source": {"type": "string", "description": "Path to meta.yaml / recipe.yaml / package.py"},
                    "format": {"type": "string", "description": "auto | conda-forge | spack (default auto)"},
                    "toolchain_name": {"type": "string", "description": "default foss"},
                    "toolchain_version": {"type": "string", "description": "default 2024a"},
                    "easyconfigs": {
                        "type": "array", "items": {"type": "string"},
                        "description": "Optional robot tree(s) for dep version resolve"
                    },
                    "keep_old_deps": {"type": "boolean"},
                    "out": {"type": "string"},
                    "out_dir": {"type": "string"},
                    "residual_queue": {"type": "string", "description": "Optional residual-queue JSON path (default next to .eb)"}
                },
                "required": ["source"]
            }
        },
        {
            "name": "eb_plan",
            "description": "Parse foreign recipe → intermediate package manifest + planned SBOM + build config → optional resolvo joint co-select over robot easyconfigs → mechanical new .eb or bump_from existing. Structured path preferred over bare ingest when plan JSON is needed.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "source": {"type": "string"},
                    "format": {"type": "string", "description": "auto | conda-forge | spack"},
                    "toolchain_name": {"type": "string"},
                    "toolchain_version": {"type": "string"},
                    "easyconfigs": {"type": "array", "items": {"type": "string"}},
                    "keep_old_deps": {"type": "boolean"},
                    "manifest_out": {"type": "string", "description": "Intermediate plan JSON path"},
                    "sbom_out": {"type": "string", "description": "Planned CycloneDX-like SBOM path"},
                    "out": {"type": "string"},
                    "out_dir": {"type": "string"},
                    "bump_from": {"type": "string", "description": "Existing .eb to bump using solved pins"}
                },
                "required": ["source"]
            }
        }
    ])
}

/// tools/call always returns a `result` (never a JSON-RPC error): tool
/// failures are in-band `isError: true` content, per the MCP spec.
fn handle_tool_call(params: &Value) -> Value {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or(json!({}));
    let outcome = match name {
        "eb_check_recipe" => tool_check_recipe(&args),
        "eb_bump" => tool_bump(&args),
        "eb_solve" => tool_solve(&args),
        "eb_ingest" => tool_ingest(&args),
        "eb_plan" => tool_plan(&args),
        other => Err(format!("unknown tool: {other}")),
    };
    match outcome {
        Ok(v) => json!({
            "content": [{"type": "text", "text": v.to_string()}],
            "isError": false
        }),
        Err(e) => json!({
            "content": [{"type": "text", "text": e}],
            "isError": true
        }),
    }
}

fn req_str(args: &Value, key: &str) -> Result<String, String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("missing required argument: {key}"))
}

fn opt_str(args: &Value, key: &str) -> Option<String> {
    args.get(key).and_then(Value::as_str).map(str::to_string)
}

fn str_vec(args: &Value, key: &str) -> Vec<String> {
    args.get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Actionable instructions derived from a check result, so the tool output
/// itself tells the driver what to do next.
fn check_next_actions(check: &RecipeDepCheck, gate_errors: &[String]) -> Vec<String> {
    let mut actions = Vec::new();
    for m in &check.missing {
        if m.name == "checksums" && m.role == "packaging" {
            actions.push(format!(
                "Fix the recipe's checksums block ({}): checksums are positional — all source entries first, then patch entries in patches order. Never delete or bypass the check.",
                m.reason
            ));
        } else {
            actions.push(format!(
                "Resolve missing {} dep {} {}: {}. Bump or scaffold the companion recipe at this generation, then re-run eb_check_recipe.",
                m.role, m.name, m.version, m.reason
            ));
        }
    }
    for e in gate_errors {
        actions.push(format!("Fix packaging gate: {e}"));
    }
    if actions.is_empty() {
        actions.push(
            "`resolves` rung established. To claim `builds`, run `eb --robot` on a build machine through its scheduler; to claim `binary-verified`, run the installed binary and ldd it."
                .to_string(),
        );
    }
    actions
}

fn tool_check_recipe(args: &Value) -> Result<Value, String> {
    let recipe = req_str(args, "recipe")?;
    let trees = str_vec(args, "easyconfigs");
    if trees.is_empty() {
        return Err("missing required argument: easyconfigs (non-empty array)".into());
    }
    let reqs = str_vec(args, "require_configopts");
    let resolved =
        resolve_easyconfig_file(Path::new(&recipe)).map_err(|e| format!("resolve {recipe}: {e}"))?;
    let req_refs: Vec<&str> = reqs.iter().map(String::as_str).collect();
    let gate = packaging_gate(&resolved, &req_refs);
    let roots: Vec<&Path> = trees.iter().map(|s| Path::new(s.as_str())).collect();
    let tree = parse_easyconfig_trees(&roots).map_err(|e| format!("parse robot trees: {e}"))?;
    let check = check_recipe_deps(&resolved, &tree.candidates);
    let gate_errors = gate.err().unwrap_or_default();
    let mut scaffold_written: Vec<String> = Vec::new();
    if let Some(dir) = opt_str(args, "scaffold_missing") {
        if !check.ok() {
            let written = crate::scaffold_missing_companions(
                &check.missing,
                Path::new(&dir),
                &resolved.toolchain,
            )
            .map_err(|e| format!("scaffold under {dir}: {e}"))?;
            scaffold_written = written
                .iter()
                .map(|s| {
                    if s.skipped_existing {
                        format!("{} (exists, skipped)", s.path)
                    } else {
                        s.path.clone()
                    }
                })
                .collect();
        }
    }
    let ok = check.ok() && gate_errors.is_empty();
    Ok(json!({
        "ok": ok,
        "check": check,
        "packaging_gate_errors": gate_errors,
        "robot_parse_skipped": tree.skip_count(),
        "scaffold_written": scaffold_written,
        "ladder": ladder(ok),
        "next_actions": check_next_actions(&check, &gate_errors),
    }))
}

fn tool_bump(args: &Value) -> Result<Value, String> {
    let source = req_str(args, "source")?;
    let toolchain = Toolchain {
        name: req_str(args, "toolchain_name")?,
        version: req_str(args, "toolchain_version")?,
    };
    let version = opt_str(args, "version");
    let source_checksum = opt_str(args, "source_checksum");
    let hand: HashMap<String, String> = args
        .get("deps")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default();
    let keep_old = args
        .get("keep_old_deps")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let src = PathBuf::from(&source);
    let result = if let Some(ec_dir) = opt_str(args, "easyconfigs") {
        emit_next_generation_auto_from_path_with_opts(
            &src,
            &toolchain,
            Path::new(&ec_dir),
            None,
            opt_str(args, "hierarchy_fixture").as_deref().map(Path::new),
            &hand,
            version,
            source_checksum,
            &AutoResolveOpts { keep_old },
        )
        .map_err(|e| format!("bump {source} with auto-resolve from {ec_dir}: {e}"))?
    } else {
        emit_next_generation_from_path(
            &src,
            &EmitParams {
                toolchain,
                version,
                dep_versions: hand,
                source_checksum,
            },
        )
        .map_err(|e| format!("bump {source}: {e}"))?
    };
    let dest = if let Some(out) = opt_str(args, "out") {
        PathBuf::from(out)
    } else if let Some(dir) = opt_str(args, "out_dir") {
        std::fs::create_dir_all(&dir).map_err(|e| format!("create out-dir {dir}: {e}"))?;
        Path::new(&dir).join(&result.filename)
    } else {
        PathBuf::from(&result.filename)
    };
    if let Some(parent) = dest.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("create parent {}: {e}", parent.display()))?;
        }
    }
    std::fs::write(&dest, &result.text)
        .map_err(|e| format!("write {}: {e}", dest.display()))?;
    Ok(json!({
        "wrote": dest.display().to_string(),
        "filename": result.filename,
        "warnings": result.warnings,
        "next_actions": [
            "Run eb_check_recipe on the written file against the robot tree before any build claim.",
            "Surface every warning above to the user verbatim."
        ]
    }))
}

fn tool_solve(args: &Value) -> Result<Value, String> {
    let trees = str_vec(args, "easyconfigs");
    if trees.is_empty() {
        return Err("missing required argument: easyconfigs (non-empty array)".into());
    }
    let policy = req_str(args, "policy")?;
    let lock_out = opt_str(args, "lock_out").unwrap_or_else(|| "stack.lock.json".into());
    let baseline = opt_str(args, "baseline_easyconfigs").unwrap_or_else(|| trees[0].clone());
    let sbom_out = opt_str(args, "sbom_out").map(PathBuf::from);
    let build_list_out = opt_str(args, "build_list_out").map(PathBuf::from);
    let stack_diff_out = opt_str(args, "stack_diff_out").map(PathBuf::from);
    let roots: Vec<&Path> = trees.iter().map(|s| Path::new(s.as_str())).collect();
    let lock = solve_from_easyconfigs_with_baseline_version_and_extras(
        &roots,
        Path::new(&policy),
        Some(Path::new(&baseline)),
        opt_str(args, "baseline_toolchain_version").as_deref(),
        Path::new(&lock_out),
        sbom_out.as_deref(),
        SolveExtraOut {
            build_list_out: build_list_out.as_deref(),
            stack_diff_out: stack_diff_out.as_deref(),
        },
    )
    .map_err(|e| format!("solve: {e}"))?;
    let packages: Vec<Value> = lock
        .packages
        .iter()
        .map(|p| {
            json!({
                "name": p.name,
                "version": p.version,
                "easyconfig_path": p.easyconfig_path,
            })
        })
        .collect();
    Ok(json!({
        "lock_out": lock_out,
        "engine": lock.solver.engine,
        "package_count": lock.packages.len(),
        "packages": packages,
        "sbom_out": sbom_out.map(|p| p.display().to_string()),
        "build_list_out": build_list_out.map(|p| p.display().to_string()),
        "stack_diff_out": stack_diff_out.map(|p| p.display().to_string()),
    }))
}

fn tool_plan(args: &Value) -> Result<Value, String> {
    use crate::foreign::{ForeignFormat, IngestOpts};
    use crate::manifest::plan_and_emit;
    let source = req_str(args, "source")?;
    let fmt = match opt_str(args, "format")
        .unwrap_or_else(|| "auto".into())
        .to_ascii_lowercase()
        .as_str()
    {
        "auto" => None,
        "conda" | "conda-forge" | "cf" => Some(ForeignFormat::CondaForge),
        "spack" => Some(ForeignFormat::Spack),
        other => {
            return Err(format!(
                "unknown format {other:?}; expected auto, conda-forge, or spack"
            ))
        }
    };
    let toolchain = Toolchain {
        name: opt_str(args, "toolchain_name").unwrap_or_else(|| "foss".into()),
        version: opt_str(args, "toolchain_version").unwrap_or_else(|| "2024a".into()),
    };
    let opts = IngestOpts {
        easyconfigs: str_vec(args, "easyconfigs")
            .into_iter()
            .map(PathBuf::from)
            .collect(),
        keep_old_deps: args
            .get("keep_old_deps")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        hierarchy_fixture: None,
    };
    let (plan, eb_path) = plan_and_emit(
        Path::new(&source),
        fmt,
        &toolchain,
        &opts,
        opt_str(args, "manifest_out").as_deref().map(Path::new),
        opt_str(args, "sbom_out").as_deref().map(Path::new),
        opt_str(args, "out").as_deref().map(Path::new),
        opt_str(args, "out_dir").as_deref().map(Path::new),
        opt_str(args, "bump_from").as_deref().map(Path::new),
    )
    .map_err(|e| format!("plan {source}: {e}"))?;
    Ok(json!({
        "package": plan.package.name,
        "version": plan.package.version,
        "origin": plan.package.origin.as_str(),
        "coverage_ratio": plan.package.coverage.ratio(),
        "extracted": plan.package.coverage.extracted_count(),
        "residual": plan.package.coverage.residual_count(),
        "solved_pins": plan.solved.as_ref().map(|s| s.dep_versions.len()).unwrap_or(0),
        "engine_note": plan.solved.as_ref().map(|s| s.engine_note.clone()),
        "manifest_out": opt_str(args, "manifest_out"),
        "sbom_out": opt_str(args, "sbom_out"),
        "easyconfig": eb_path.map(|p| p.display().to_string()),
        "ladder": ladder(plan.solved.is_some()),
        "next_actions": [
            "Inspect intermediate plan JSON (package + build_config + planned SBOM).",
            "Solved dep pins come from hierarchy + resolvo when --easyconfigs is set.",
            "Run eb --inject-checksums / check-contrib / check-recipe; claim builds only after eb --robot."
        ]
    }))
}

fn tool_ingest(args: &Value) -> Result<Value, String> {
    use crate::foreign::{
        ingest_foreign_to_easyconfig_with_opts, residual_queue_from_ingest,
        write_ingest_result_with_queue, ForeignFormat, IngestOpts,
    };
    let source = req_str(args, "source")?;
    let fmt = match opt_str(args, "format")
        .unwrap_or_else(|| "auto".into())
        .to_ascii_lowercase()
        .as_str()
    {
        "auto" => None,
        "conda" | "conda-forge" | "cf" => Some(ForeignFormat::CondaForge),
        "spack" => Some(ForeignFormat::Spack),
        other => {
            return Err(format!(
                "unknown format {other:?}; expected auto, conda-forge, or spack"
            ))
        }
    };
    let toolchain = Toolchain {
        name: opt_str(args, "toolchain_name").unwrap_or_else(|| "foss".into()),
        version: opt_str(args, "toolchain_version").unwrap_or_else(|| "2024a".into()),
    };
    let opts = IngestOpts {
        easyconfigs: str_vec(args, "easyconfigs")
            .into_iter()
            .map(PathBuf::from)
            .collect(),
        keep_old_deps: args
            .get("keep_old_deps")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        hierarchy_fixture: None,
    };
    let result = ingest_foreign_to_easyconfig_with_opts(
        Path::new(&source),
        fmt,
        &toolchain,
        &opts,
    )
    .map_err(|e| format!("ingest {source}: {e}"))?;
    let residual = opt_str(args, "residual_queue").map(PathBuf::from);
    let dest = write_ingest_result_with_queue(
        &result,
        &toolchain,
        opt_str(args, "out").as_deref().map(Path::new),
        opt_str(args, "out_dir").as_deref().map(Path::new),
        residual.as_deref(),
    )
    .map_err(|e| format!("write ingest: {e}"))?;
    let queue = residual_queue_from_ingest(&result, &toolchain);
    let mut queue_path = dest.clone();
    let stem = queue_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("scaffold");
    queue_path.set_file_name(format!("{stem}.residuals.json"));
    if let Some(p) = residual {
        queue_path = p;
    }
    Ok(json!({
        "wrote": dest.display().to_string(),
        "residual_queue": queue_path.display().to_string(),
        "filename": result.filename,
        "warnings": result.warnings,
        "residual_items": queue.items.len(),
        "ladder": ladder(false),
        "next_actions": [
            "Treat residual_queue JSON as the judgment work list — do not invent product configopts in eb-stack.",
            "Run eb --inject-checksums and eb --check-contrib on the scaffold.",
            "Run eb_check_recipe against the robot tree (+ companion overlay) for the resolves rung.",
            "Claim builds only after green eb --robot on an EasyBuild host."
        ]
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn rpc(method: &str, id: i64, params: Value) -> Value {
        json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
    }

    fn call_tool(name: &str, args: Value) -> Value {
        let resp = handle_message(&rpc(
            "tools/call",
            7,
            json!({"name": name, "arguments": args}),
        ))
        .expect("tools/call must produce a response");
        resp["result"].clone()
    }

    fn tool_payload(result: &Value) -> Value {
        let text = result["content"][0]["text"].as_str().expect("text content");
        serde_json::from_str(text).expect("tool payload is JSON")
    }

    /// A tiny robot tree + recipe fixture on disk. Mirrors the lint bench:
    /// `broken` controls whether the patch checksum sits in a source slot.
    fn write_bench(dir: &Path, broken: bool) -> (PathBuf, PathBuf) {
        let robot = dir.join("robot");
        let sub = robot.join("s").join("Sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(
            sub.join("Sub-2.0-foss-2026.1.eb"),
            r#"easyblock = 'CMakeMake'
name = 'Sub'
version = '2.0'
homepage = 'https://example.com'
description = 'sub'
toolchain = {'name': 'foss', 'version': '2026.1'}
sources = ['sub-2.0.tar.gz']
checksums = [{'sub-2.0.tar.gz': 'bb22'}]
moduleclass = 'tools'
"#,
        )
        .unwrap();
        let order = if broken {
            "    {'app-1.0.tar.gz': 'aa11'},\n    {'App-1.0_fix.patch': 'dd44'},\n    {'sub-2.0.tar.gz': 'bb22'},\n"
        } else {
            "    {'app-1.0.tar.gz': 'aa11'},\n    {'sub-2.0.tar.gz': 'bb22'},\n    {'App-1.0_fix.patch': 'dd44'},\n"
        };
        let recipe = dir.join("App-1.0-foss-2026.1.eb");
        std::fs::write(
            &recipe,
            format!(
                r#"easyblock = 'CMakeMake'
name = 'App'
version = '1.0'
homepage = 'https://example.com'
description = 'app'
toolchain = {{'name': 'foss', 'version': '2026.1'}}
sources = ['app-1.0.tar.gz', 'sub-2.0.tar.gz']
patches = ['App-1.0_fix.patch']
checksums = [
{order}]
dependencies = [('Sub', '2.0')]
moduleclass = 'tools'
"#
            ),
        )
        .unwrap();
        (recipe, robot)
    }

    #[test]
    fn initialize_and_list_tools_over_the_wire() {
        let input = format!(
            "{}\n{}\n{}\n",
            rpc("initialize", 1, json!({"protocolVersion": "2025-03-26"})),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            rpc("tools/list", 2, Value::Null),
        );
        let mut out = Vec::new();
        run_server(Cursor::new(input), &mut out).unwrap();
        let lines: Vec<Value> = String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        // Notification produced no reply: exactly two responses.
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["result"]["serverInfo"]["name"], "eb-stack");
        assert_eq!(lines[0]["result"]["protocolVersion"], "2025-03-26");
        let names: Vec<&str> = lines[1]["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            ["eb_check_recipe", "eb_bump", "eb_solve", "eb_ingest"]
        );
    }

    #[test]
    fn ingest_tool_writes_scaffold_and_residual_queue() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("meta.yaml");
        std::fs::write(
            &src,
            r#"
package:
  name: zlib
  version: 1.3.1
source:
  url: https://example.com/zlib-1.3.1.tar.gz
  sha256: 9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23
requirements:
  build:
    - make
about:
  home: https://zlib.net/
  summary: zlib
"#,
        )
        .unwrap();
        let out = dir.path().join("zlib.eb");
        let result = call_tool(
            "eb_ingest",
            json!({
                "source": src.display().to_string(),
                "out": out.display().to_string(),
                "toolchain_name": "foss",
                "toolchain_version": "2024a",
            }),
        );
        assert_eq!(result["isError"], false, "{result}");
        let payload = tool_payload(&result);
        assert_eq!(payload["ladder"]["resolves"], false);
        assert!(out.is_file(), "scaffold written");
        let rq = PathBuf::from(payload["residual_queue"].as_str().unwrap());
        assert!(rq.is_file(), "residual queue written at {rq:?}");
        let qtext = std::fs::read_to_string(&rq).unwrap();
        assert!(qtext.contains("moduleclass") || qtext.contains("sanity") || qtext.contains("items"));
    }

    #[test]
    fn unknown_method_errors_and_unknown_tool_is_in_band() {
        let resp = handle_message(&rpc("resources/list", 3, Value::Null)).unwrap();
        assert_eq!(resp["error"]["code"], -32601);
        let result = call_tool("no_such_tool", json!({}));
        assert_eq!(result["isError"], true);
    }

    #[test]
    fn check_recipe_tool_flags_checksum_order_with_instruction() {
        let dir = tempfile::tempdir().unwrap();
        let (recipe, robot) = write_bench(dir.path(), true);
        let result = call_tool(
            "eb_check_recipe",
            json!({
                "recipe": recipe.display().to_string(),
                "easyconfigs": [robot.display().to_string()],
            }),
        );
        assert_eq!(result["isError"], false);
        let payload = tool_payload(&result);
        assert_eq!(payload["ok"], false);
        assert_eq!(payload["ladder"]["resolves"], false);
        let actions = payload["next_actions"].as_array().unwrap();
        let joined = actions
            .iter()
            .map(|a| a.as_str().unwrap())
            .collect::<Vec<_>>()
            .join(" | ");
        assert!(
            joined.contains("positional") && joined.contains("Never delete or bypass"),
            "checksum finding must carry the fix instruction, got: {joined}"
        );
    }

    #[test]
    fn check_recipe_tool_passes_fixed_bench_and_states_ladder() {
        let dir = tempfile::tempdir().unwrap();
        let (recipe, robot) = write_bench(dir.path(), false);
        let result = call_tool(
            "eb_check_recipe",
            json!({
                "recipe": recipe.display().to_string(),
                "easyconfigs": [robot.display().to_string()],
            }),
        );
        let payload = tool_payload(&result);
        assert_eq!(payload["ok"], true);
        assert_eq!(payload["ladder"]["resolves"], true);
        let builds = payload["ladder"]["builds"].as_str().unwrap();
        assert!(builds.contains("not-established"));
        let first = payload["next_actions"][0].as_str().unwrap();
        assert!(first.contains("`resolves` rung established"));
    }

    #[test]
    fn bump_tool_writes_conventional_file_and_check_accepts_it() {
        let dir = tempfile::tempdir().unwrap();
        let (recipe, robot) = write_bench(dir.path(), false);
        let out_dir = dir.path().join("out");
        let result = call_tool(
            "eb_bump",
            json!({
                "source": recipe.display().to_string(),
                "toolchain_name": "foss",
                "toolchain_version": "2027.1",
                "deps": {"Sub": "3.0"},
                "out_dir": out_dir.display().to_string(),
            }),
        );
        assert_eq!(result["isError"], false, "bump failed: {result}");
        let payload = tool_payload(&result);
        assert_eq!(payload["filename"], "App-1.0-foss-2027.1.eb");
        let wrote = payload["wrote"].as_str().unwrap();
        let text = std::fs::read_to_string(wrote).unwrap();
        assert!(text.contains("'version': '2027.1'"));
        assert!(payload["next_actions"][0]
            .as_str()
            .unwrap()
            .contains("eb_check_recipe"));
        // Deps match cross-toolchain by name+version, so the bump to Sub 3.0
        // (absent from the robot tree at any generation) must fail the check
        // — proving the loop (bump -> check) composes.
        let check = tool_payload(&call_tool(
            "eb_check_recipe",
            json!({
                "recipe": wrote,
                "easyconfigs": [robot.display().to_string()],
            }),
        ));
        assert_eq!(check["ok"], false);
        assert_eq!(check["check"]["missing"][0]["name"], "Sub");
    }

    #[test]
    fn missing_required_argument_is_in_band_error() {
        let result = call_tool("eb_check_recipe", json!({"recipe": "/nope.eb"}));
        assert_eq!(result["isError"], true);
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("easyconfigs"));
    }
}
