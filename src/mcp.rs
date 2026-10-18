//! MCP stdio surface for the version-one package, recipe, stack, target, and campaign workflows.

use crate::campaign::{
    claim_finding, resolve_finding, run_campaign, CampaignRequest, CampaignStatus,
    FindingResolution,
};
use crate::dep_versions::dep_versions;
use crate::domain::Toolchain;
use crate::eb_parse::{
    check_recipe_deps, packaging_gate, parse_easyconfig_trees, resolve_easyconfig_file,
};
use crate::eb_style::{
    format_style, format_style_file, is_formatting_code, lint_recipe_files_scoped, lint_style,
    python_syntax_error, review_item_findings, review_item_status, RecipeScope, ReviewItem,
    ReviewItemStatus, Severity,
};
use crate::foreign::ForeignFormat;
use crate::package::{StackPolicy, STACK_POLICY_SCHEMA_VERSION};
use crate::package_catalog::{resolve_package_catalog_layers, PackageCatalogLayer};
use crate::package_closure::{plan_package_closure_with_sources, write_package_closure};
use crate::package_config::PackageConfigLayer;
use crate::package_sources::{PackageSourceRoots, SourceRootKind};
use crate::package_workflow::{
    inspect_new_package, plan_new_package_with, plan_package_bump_with, write_package_bundle,
    BumpPackageRequest, NewPackageRequest, PackageBundle, PackageEmitOptions,
};
use crate::target::{doctor_target, resolve_target_layers, BuildTarget, TargetConfigLayer};
use crate::workspace_fs::{
    apply_suggestion_workspace, configured_workspace_root, edit_workspace_scoped, grep_workspace,
    list_workspace, read_workspace, write_workspace,
};
use crate::{
    load_json_file, lock_to_cyclonedx, solve_with_baseline_source, write_json_pretty,
    BaselineSource, SolveExtraOut, StackLock,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

/// Serve MCP over a line-delimited JSON-RPC stream until the reader ends.
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

/// Answer one JSON-RPC message. `None` for a notification, which takes no reply.
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
            "Parse a conda-forge, Spack, PyPI, or CRAN recipe into a canonical build manifest and planned CycloneDX SBOM.",
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
                ("package_sources", "array"),
                ("easybuild_sources", "array"),
                ("conda_sources", "array"),
                ("spack_sources", "array"),
                ("cargo_sources", "array"),
                ("emit_easyblock", "boolean"),
            ],
        ),
        tool_with_optional(
            "eb_package_bump",
            "Retarget an EasyBuild recipe and emit its canonical SBOM, Resolvo lock, and recipe bundle.",
            &[
                ("source", "string"),
                ("toolchain_name", "string"),
                ("toolchain_version", "string"),
                ("easyconfigs", "array"),
                ("out_dir", "string"),
            ],
            &[
                ("version", "string"),
                ("source_checksum", "string"),
                ("dependencies", "object"),
                ("hierarchy_fixture", "string"),
                ("stack_policy", "string"),
                ("package_configs", "array"),
                ("emit_easyblock", "boolean"),
            ],
        ),
        tool_with_optional(
            "eb_recipe_read",
            "Read one UTF-8 file inside the workspace root (EB_STACK_WORKSPACE, the directory given to eb-stack mcp --workspace, or the server's current directory). Rejects .., absolute paths outside that root, and symlinks whose target leaves the root.",
            &[("path", "string")],
            &[],
        ),
        tool_with_optional(
            "eb_recipe_list",
            "List one directory inside the workspace root without following symlinks. A symlink whose target leaves the root is reported with escapes set and is not opened.",
            &[("path", "string")],
            &[],
        ),
        tool_with_optional(
            "eb_recipe_edit",
            "Replace an exact literal string in a workspace file. old must be non-empty and different from new. When old matches more than once the file is left unchanged and the error lists the line and the list entry (component, extension or dependency name) of every match. within names one ('name', ...) entry so only matches inside it change; when two entries share the name, occurrence (1-based) picks one. Without within, occurrence picks the n-th match. Use replace_all only when the review asks for the change at every occurrence; the result's note says when one call changed several different entries.",
            &[("path", "string"), ("old", "string"), ("new", "string")],
            &[("replace_all", "boolean"), ("within", "string"), ("occurrence", "integer")],
        ),
        tool_with_optional(
            "eb_recipe_apply_suggestion",
            "Apply a review comment's code suggestion to the lines the comment is anchored on, the way GitHub's 'Commit suggestion' does: lines line_start..line_end (1-based; line_end defaults to line_start) are replaced by suggestion, and an empty or missing suggestion deletes them. Pass expect with the anchored lines as the review showed them: when earlier edits moved them, the unique place where they now are is used, and when they are not found nothing is written. line_start and line_end are the review's numbering: without expect, once the file has changed, the lines it had there when first seen are looked up instead. A suggestion that would leave the file invalid Python is not written. Use it for every suggestion block, including empty ones, before any other edit to those lines; then check the result against the rules (R1a) with eb_recipe_lint.",
            &[("path", "string"), ("line_start", "integer")],
            &[("line_end", "integer"), ("suggestion", "string"), ("expect", "string")],
        ),
        tool_with_optional(
            "eb_recipe_diff",
            "Show what changed in each recipe since this server first saw it (rebaseline does not reset this), hunk by hunk with the top-level parameters each hunk touches and whether it only changes comments. Call it last, before the final answer, and describe exactly these hunks: a change you describe must have a hunk, and a hunk you cannot justify from the review must be reverted. With review_items (as for eb_recipe_lint) each review comment is reported as answered or open: a comment with no matching hunk is either still to do or needs a reason in the final answer (no_change_needed).",
            &[("recipes", "array")],
            &[("review_items", "array_review_items"), ("no_change_needed", "array")],
        ),
        tool_with_optional(
            "eb_recipe_write",
            "Create or overwrite a UTF-8 file inside the workspace root. Parent directories must already exist. Rejects path and symlink escapes, and does not follow a symlink that leaves the workspace.",
            &[("path", "string"), ("content", "string")],
            &[],
        ),
        tool_with_optional(
            "eb_recipe_grep",
            "Search UTF-8 files inside the workspace root for a literal string (or a regular expression when regex is true). path is a file or directory inside the workspace (default the root). Returns path, line, and text per match. Use it to find every occurrence of a pattern a reviewer pointed at before editing.",
            &[("pattern", "string")],
            &[("path", "string"), ("regex", "boolean"), ("max_matches", "integer")],
        ),
        tool_with_optional(
            "eb_recipe_check",
            "Check EasyBuild packaging metadata and robot dependency resolution. A recipe that is not valid Python fails. easyconfigs must contain at least one path: a robot directory such as easybuild/easyconfigs. A .eb file there is accepted and stands for the easyconfigs directory it sits in.",
            &[("recipe", "string"), ("easyconfigs", "array_min1")],
            &[("require_configopts", "array")],
        ),
        tool_with_optional(
            "eb_recipe_lint",
            "Report what upstream checks on an easyconfig, without rewriting it: E999 (not valid Python), the easyconfigs CI lint (F541, W605, W291), the eb --check-contrib pycodestyle codes (E501, E2xx), the easyconfigs test-suite checks, and review conventions (EB_FMT_PERCENT for '...' % x, EB_FMT_CONCAT for '...' + SHLIB_EXT, EB_SHLIB_HARDCODED, ...). clean is false while any error or any in-scope EB_FMT_* finding remains; a review fix is not done while clean is false. f'lib/%(name)s.{SHLIB_EXT}' is correct and not flagged. Scope: a finding with info: true is outside the review scope and does not count. review_lines (1-based lines the review comment is anchored to) are always in scope, and EB_SHLIB_HARDCODED on them counts too; with several recipes pass review_lines_by_recipe ({recipe: [lines]}) instead. EB_SHLIB_HARDCODED also counts on every line under review_scope \"file\" and on any line you edited. With base (the file before your edit) lines unchanged from base are out of scope unless review_scope is \"file\"; pass review_scope \"file\" when a PR-level comment asks for a style everywhere (\"use f-strings\"). Without base, only untouched '%%(...)s' % X entries are out of scope. Diff-mode errors (EB_SANITY_ENTRY_REMOVED, EB_EASYBLOCK_CHANGED, EB_FMT_TEMPLATE_REMOVED) compare against base, or else against the file as this server first saw it; rebaseline true resets that only when no diff-mode error is outstanding off the review lines. An error the file already had before your edits, on a line the review does not cover, is reported as info (pre-existing) and does not block clean. With review lines, EB_DIFF_OUTSIDE_REVIEW warns about a change far from every review line that is not the reviewed change repeated, and EB_DIFF_SOURCES_CHANGED (blocks clean) about sources, crates, patches or checksums changed when no comment is about them. EB_DEP_EMPTY_VERSION is an error: look the version up with eb_dep_versions. When eb is on PATH, eb --check-contrib findings are merged. review_items lists every review comment ({id, recipe, line_start, line_end, suggestion, comment}; lines as the review showed them; suggestion null for a prose comment and \"\" for an empty suggestion): each one is reported as answered (a change touches its lines, its suggestion is in the file, or its lines are deleted) or open, and EB_REVIEW_ITEM_OPEN blocks clean while any is open. Put an id in no_change_needed when that comment needs no change (a question already answered, or the file already does it) and say why in the final answer.",
            &[("recipes", "array")],
            &[("base", "string"), ("review_lines", "array_int"), ("review_lines_by_recipe", "object_int_arrays"), ("review_scope", "string"), ("rebaseline", "boolean"), ("review_items", "array_review_items"), ("no_change_needed", "array")],
        ),
        tool_with_optional(
            "eb_dep_versions",
            "List the versions of a dependency that have an easyconfig in a local easyconfigs tree, for a toolchain (for example GCCcore-13.3.0, or system) and for other toolchains. easyconfigs are robot directories or .eb files to search (default: the workspace root). Use it instead of guessing a dependency version; if the tree has no easyconfig of it, the result says so, and the version should not be invented.",
            &[("name", "string")],
            &[("toolchain", "string"), ("easyconfigs", "array")],
        ),
        tool_with_optional(
            "eb_recipe_format",
            "Mechanically wrap EasyBuild E501 lines and report any residual lines. Without out, the recipe is rewritten in place. With dry_run true nothing is written and the formatted text is returned. out writes a copy instead: it must be a .eb path inside the workspace whose directory exists; in a review fix, leave out unset so no extra files appear.",
            &[("recipe", "string")],
            &[("out", "string"), ("dry_run", "boolean")],
        ),
        tool_with_optional(
            "eb_module_lint",
            "Check Python modules that ship with recipes, such as easyblocks (easybuild/easyblocks/<letter>/<name>.py), the way the easyblocks CI does: E999 (not valid Python), E501 at the max-line-length of the nearest [flake8] section (setup.cfg, tox.ini or .flake8; easyblocks use 120), W291/W293 trailing whitespace, W292/W391 end of file, EBM_RUN_CMD (run_cmd/run_cmd_qa, removed in EasyBuild 5) and the EBM_OLD_SUPER warning. When flake8 is on PATH its findings (F401 unused import, F821 undefined name, C4 comprehensions, ...) are merged. Do not use eb_recipe_lint on .py files: its rules are for easyconfigs. base is the module before your edits (default: the module as this server first saw it); an error already there on an unchanged line is info and does not block clean. clean true is necessary but not enough: an easyblock change is done only when eb_easyblock_dry_run shows the commands you intended.",
            &[("modules", "array_min1")],
            &[("base", "string"), ("flake8", "boolean")],
        ),
        tool_with_optional(
            "eb_stack_solve",
            "Solve a jointly consistent EasyBuild stack lock and optional reports.",
            &[
                ("easyconfigs", "array"),
                ("policy", "string"),
                ("lock_out", "string"),
            ],
            &[
                ("baseline_easyconfigs", "string"),
                ("baseline_toolchain_version", "string"),
                ("baseline_easystacks", "array"),
                ("baseline_commits_from", "string"),
                ("sbom_out", "string"),
                ("build_list_out", "string"),
                ("build_list_excludes_baseline", "boolean"),
                ("stack_diff_out", "string"),
            ],
        ),
        tool_with_optional(
            "eb_stack_sbom",
            "Emit CycloneDX from an existing stack lock.",
            &[("lock", "string")],
            &[("out", "string")],
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
            let schema = match *kind {
                "array" => json!({"type": "array", "items": {"type": "string"}}),
                "array_min1" => {
                    json!({"type": "array", "items": {"type": "string"}, "minItems": 1})
                }
                "array_int" => json!({"type": "array", "items": {"type": "integer", "minimum": 1}}),
                "object" => json!({"type": "object", "additionalProperties": {"type": "string"}}),
                "object_int_arrays" => json!({"type": "object", "additionalProperties": {"type": "array", "items": {"type": "integer", "minimum": 1}}}),
                "array_review_items" => json!({"type": "array", "items": {
                    "type": "object",
                    "properties": {
                        "id": {"type": "string"},
                        "recipe": {"type": "string"},
                        "line_start": {"type": "integer", "minimum": 1},
                        "line_end": {"type": "integer", "minimum": 1},
                        "suggestion": {"type": ["string", "null"]},
                        "comment": {"type": "string"}
                    },
                    "required": ["line_start"]
                }}),
                other => json!({"type": other}),
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
        "eb_recipe_read" => recipe_read(arguments),
        "eb_recipe_list" => recipe_list(arguments),
        "eb_recipe_edit" => recipe_edit(arguments),
        "eb_recipe_apply_suggestion" => recipe_apply_suggestion(arguments),
        "eb_recipe_diff" => recipe_diff(arguments),
        "eb_recipe_write" => recipe_write(arguments),
        "eb_recipe_check" => recipe_check(arguments),
        "eb_recipe_grep" => recipe_grep(arguments),
        "eb_dep_versions" => dep_versions_tool(arguments),
        "eb_recipe_lint" => recipe_lint(arguments),
        "eb_recipe_format" => recipe_format(arguments),
        "eb_module_lint" => module_lint(arguments),
        "eb_stack_solve" => stack_solve(arguments),
        "eb_stack_sbom" => stack_sbom(arguments),
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
        package_index: Default::default(),
        easyconfig_roots: path_array(arguments, "easyconfigs")?,
        stack_policy,
    };
    let output = required_path(arguments, "out_dir")?;
    let catalog_paths = string_array(arguments, "package_catalogs")?
        .into_iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    let source_roots = load_package_source_roots(arguments)?;
    let use_closure = !catalog_paths.is_empty() || !source_roots.source_roots.is_empty();
    let emit_easyblock = optional_bool(arguments, "emit_easyblock");
    if use_closure && emit_easyblock {
        return Err("emit_easyblock does not combine with package-closure planning".into());
    }
    if !use_closure {
        let options = PackageEmitOptions {
            easyblock_skeleton_root: emit_easyblock.then(|| output.clone()),
        };
        let bundle =
            plan_new_package_with(&request, &options).map_err(|error| error.to_string())?;
        let written = write_package_bundle(&bundle, &output).map_err(|error| error.to_string())?;
        return Ok(json!({
            "package": bundle.plan.package.name,
            "version": bundle.plan.package.version,
            "manifest": written.manifest,
            "sbom": written.sbom,
            "locks": written.locks,
            "easyconfigs": written.easyconfigs,
            "patches": written.patches,
            "easyblocks": written.easyblocks,
            "claims": {"resolves": true, "builds": false, "binary_verified": false}
        }));
    }

    let mut layers = Vec::with_capacity(catalog_paths.len());
    for path in catalog_paths {
        layers.push(PackageCatalogLayer::from_path(&path).map_err(|error| error.to_string())?);
    }
    let catalog = resolve_package_catalog_layers(&layers).map_err(|error| error.to_string())?;
    let closure = plan_package_closure_with_sources(&request, &catalog, &source_roots)
        .map_err(|error| error.to_string())?;
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
                "easyblocks": companion.easyblocks,
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
        "easyblocks": written.root.easyblocks,
        "companions": companions,
        "closure_plan": written.closure_plan,
        "closure_sbom": written.closure_sbom,
        "build_order": written.build_order,
        "claims": {"resolves": true, "builds": false, "binary_verified": false}
    }))
}

fn load_package_source_roots(arguments: &Value) -> Result<PackageSourceRoots, String> {
    let mut roots = PackageSourceRoots {
        schema_version: 1,
        source_roots: Vec::new(),
    };
    // Same contract as the CLI: the solve robot is also a discovery root, so a
    // dependency present there only at another generation becomes a companion
    // bump instead of an unsatisfiable solve.
    for path in string_array(arguments, "easyconfigs")? {
        roots.push(SourceRootKind::EasyBuild, PathBuf::from(path));
    }
    for path in string_array(arguments, "package_sources")? {
        let layer =
            PackageSourceRoots::from_path(Path::new(&path)).map_err(|error| error.to_string())?;
        roots.extend_from(&layer);
    }
    for path in string_array(arguments, "easybuild_sources")? {
        roots.push(SourceRootKind::EasyBuild, PathBuf::from(path));
    }
    for path in string_array(arguments, "conda_sources")? {
        roots.push(SourceRootKind::CondaForge, PathBuf::from(path));
    }
    for path in string_array(arguments, "spack_sources")? {
        roots.push(SourceRootKind::Spack, PathBuf::from(path));
    }
    for path in string_array(arguments, "cargo_sources")? {
        roots.push(SourceRootKind::Cargo, PathBuf::from(path));
    }
    Ok(roots)
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
    let options = PackageEmitOptions {
        easyblock_skeleton_root: optional_bool(arguments, "emit_easyblock")
            .then(|| required_path(arguments, "out_dir"))
            .transpose()?,
    };
    let bundle = plan_package_bump_with(
        &BumpPackageRequest {
            source: required_path(arguments, "source")?,
            toolchain: target,
            version: optional_string(arguments, "version"),
            source_checksum: optional_string(arguments, "source_checksum"),
            easyconfig_roots: path_array(arguments, "easyconfigs")?,
            hierarchy_fixture: optional_path(arguments, "hierarchy_fixture"),
            overrides: string_map(arguments, "dependencies")?,
            stack_policy,
            strict_patches: false,
            package_layers: package_layers(arguments)?,
        },
        &options,
    )
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
        "easyblocks": written.easyblocks,
        "claims": {"resolves": true, "builds": false, "binary_verified": false}
    }))
}

fn recipe_check(arguments: &Value) -> Result<Value, String> {
    let recipe_path = required_path(arguments, "recipe")?;
    let text = std::fs::read_to_string(&recipe_path)
        .map_err(|error| format!("read {}: {error}", recipe_path.display()))?;
    if let Some((line, column, message)) = python_syntax_error(&text) {
        return Err(format!(
            "{}:{line}:{column}: {message}. EasyBuild cannot parse this easyconfig; fix the syntax (eb_recipe_lint reports it as E999) before checking it",
            recipe_path.display()
        ));
    }
    let recipe = resolve_easyconfig_file(&recipe_path).map_err(|error| error.to_string())?;
    let roots = path_array(arguments, "easyconfigs")?
        .into_iter()
        .map(|root| robot_root(&root))
        .collect::<Result<Vec<_>, _>>()?;
    let root_refs = roots.iter().map(PathBuf::as_path).collect::<Vec<_>>();
    let tree = parse_easyconfig_trees(&root_refs).map_err(|error| error.to_string())?;
    let check = check_recipe_deps(&recipe, &tree.candidates);
    let required_options = string_array(arguments, "require_configopts")?;
    let option_refs = required_options
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let packaging = packaging_gate(&recipe, &option_refs);
    // An empty `missing` list earns the resolves claim only when the matches
    // behind it were filtered by a real toolchain hierarchy. Without one they
    // ignore toolchain and can pair the recipe with another generation, which
    // is the shape the CLI refuses to call a resolve. The claim ladder is the
    // whole point of this surface, so it does not get to be laxer.
    let resolves = check.ok() && check.toolchain_verified() && packaging.is_ok();
    let packaging_errors = packaging.err().unwrap_or_default();
    Ok(json!({
        "recipe": recipe.easyconfig_path,
        "resolves": resolves,
        "dependency_check": check,
        "packaging_errors": packaging_errors,
        "claims": {"resolves": resolves, "builds": false, "binary_verified": false}
    }))
}

/// A robot root for `eb_recipe_check`. A `.eb` file stands for the
/// `easyconfigs` directory it sits in (or its own directory when there is
/// none above it), so agents that pass the recipe itself still get a solve.
fn robot_root(path: &Path) -> Result<PathBuf, String> {
    if path.is_dir() {
        return Ok(path.to_path_buf());
    }
    if !path.exists() {
        return Err(format!(
            "easyconfigs path {} does not exist; pass a robot directory such as easybuild/easyconfigs",
            path.display()
        ));
    }
    if path.extension().and_then(|extension| extension.to_str()) != Some("eb") {
        return Err(format!(
            "easyconfigs path {} is a file, not a directory; pass a robot directory such as easybuild/easyconfigs",
            path.display()
        ));
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    let parent = parent.unwrap_or_else(|| Path::new("."));
    let root = parent
        .ancestors()
        .find(|ancestor| ancestor.file_name().and_then(|name| name.to_str()) == Some("easyconfigs"))
        .unwrap_or(parent);
    Ok(root.to_path_buf())
}

fn dep_versions_tool(arguments: &Value) -> Result<Value, String> {
    let name = required_string(arguments, "name")?;
    let mut roots = path_array(arguments, "easyconfigs").unwrap_or_default();
    if roots.is_empty() {
        roots.push(configured_workspace_root()?);
    }
    let toolchain = optional_string(arguments, "toolchain");
    let found = dep_versions(&roots, &name, toolchain.as_deref())?;
    serde_json::to_value(found).map_err(|error| error.to_string())
}

fn recipe_grep(arguments: &Value) -> Result<Value, String> {
    let root = configured_workspace_root()?;
    let path = optional_path(arguments, "path").unwrap_or_else(|| PathBuf::from("."));
    let max_matches = arguments
        .get("max_matches")
        .and_then(Value::as_u64)
        .map(|value| value as usize);
    let found = grep_workspace(
        &root,
        &path,
        &required_string(arguments, "pattern")?,
        optional_bool(arguments, "regex"),
        max_matches,
    )?;
    serde_json::to_value(found).map_err(|error| error.to_string())
}

/// The text of each recipe as this server first saw it, keyed by canonical
/// path. `None` when the first touch created the file. This is the
/// pre-edit baseline for diff-mode lint rules when no base is passed.
static FIRST_SEEN: std::sync::LazyLock<std::sync::Mutex<HashMap<PathBuf, Option<String>>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));

/// `path` as the tools see it: as given when it exists, else under the
/// workspace root.
fn resolve_recipe_path(path: &Path) -> PathBuf {
    if path.is_relative() && !path.exists() {
        if let Ok(root) = configured_workspace_root() {
            return root.join(path);
        }
    }
    path.to_path_buf()
}

fn snapshot_key(path: &Path) -> Option<PathBuf> {
    let resolved = resolve_recipe_path(path);
    if let Ok(found) = resolved.canonicalize() {
        return Some(found);
    }
    // Not created yet: key on the canonical directory plus the file name.
    let parent = resolved.parent()?.canonicalize().ok()?;
    Some(parent.join(resolved.file_name()?))
}

/// Record `path`'s current text the first time this server sees it.
fn remember_first_seen(path: &Path) {
    let Some(key) = snapshot_key(path) else {
        return;
    };
    let text = std::fs::read_to_string(resolve_recipe_path(path)).ok();
    if let Ok(mut kept) = PRISTINE.lock() {
        kept.entry(key.clone()).or_insert(text.clone());
    }
    if let Ok(mut seen) = FIRST_SEEN.lock() {
        seen.entry(key).or_insert(text);
    }
}

/// The text of each recipe as this server first saw it. Unlike
/// [`FIRST_SEEN`], `rebaseline` never resets it, so a diff-mode error
/// (a lost sanity entry, a changed easyblock, a removed template) cannot be
/// silenced by rebaselining (proof v6: a lost versioned sanity entry was
/// cleared with `rebaseline: true`).
static PRISTINE: std::sync::LazyLock<std::sync::Mutex<HashMap<PathBuf, Option<String>>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));

fn pristine(path: &Path) -> Option<String> {
    let key = snapshot_key(path)?;
    PRISTINE.lock().ok()?.get(&key).cloned().flatten()
}

/// Diff-mode error codes: they compare the file with its earlier version.
const DIFF_MODE_ERRORS: &[&str] = &["EB_SANITY_ENTRY_REMOVED", "EB_EASYBLOCK_CHANGED"];

fn first_seen(path: &Path) -> Option<String> {
    let key = snapshot_key(path)?;
    FIRST_SEEN.lock().ok()?.get(&key).cloned().flatten()
}

fn rebaseline(path: &Path) {
    if let Some(key) = snapshot_key(path) {
        let text = std::fs::read_to_string(resolve_recipe_path(path)).ok();
        if let Ok(mut seen) = FIRST_SEEN.lock() {
            seen.insert(key, text);
        }
    }
}

fn recipe_read(arguments: &Value) -> Result<Value, String> {
    let root = configured_workspace_root()?;
    let path = required_path(arguments, "path")?;
    remember_first_seen(&path);
    let text = read_workspace(&root, &path)?;
    serde_json::to_value(text).map_err(|error| error.to_string())
}

fn recipe_list(arguments: &Value) -> Result<Value, String> {
    let root = configured_workspace_root()?;
    let listing = list_workspace(&root, &required_path(arguments, "path")?)?;
    serde_json::to_value(listing).map_err(|error| error.to_string())
}

fn recipe_edit(arguments: &Value) -> Result<Value, String> {
    let root = configured_workspace_root()?;
    let path = required_path(arguments, "path")?;
    remember_first_seen(&path);
    let within = optional_string(arguments, "within").filter(|name| !name.is_empty());
    let occurrence = arguments
        .get("occurrence")
        .and_then(Value::as_u64)
        .map(|value| value as usize);
    let edited = edit_workspace_scoped(
        &root,
        &path,
        &required_string(arguments, "old")?,
        &required_string(arguments, "new")?,
        optional_bool(arguments, "replace_all"),
        within.as_deref(),
        occurrence,
    )?;
    serde_json::to_value(edited).map_err(|error| error.to_string())
}

/// `eb_recipe_apply_suggestion`: put a review suggestion on the lines its
/// comment is anchored to (an empty suggestion deletes them).
fn recipe_apply_suggestion(arguments: &Value) -> Result<Value, String> {
    let root = configured_workspace_root()?;
    let path = required_path(arguments, "path")?;
    remember_first_seen(&path);
    let line = |name: &str| {
        arguments
            .get(name)
            .and_then(Value::as_u64)
            .map(|value| value as usize)
            .ok_or_else(|| format!("missing integer argument {name}"))
    };
    let start = line("line_start")?;
    let end = arguments
        .get("line_end")
        .and_then(Value::as_u64)
        .map_or(start, |value| value as usize);
    let suggestion = optional_string(arguments, "suggestion").unwrap_or_default();
    let expect = optional_string(arguments, "expect");
    let kept = pristine(&path);
    let edited = apply_suggestion_workspace(
        &root,
        &path,
        start,
        end,
        &suggestion,
        expect.as_deref(),
        kept.as_deref(),
    )?;
    serde_json::to_value(edited).map_err(|error| error.to_string())
}

/// `review_items` that belong to `path`: items whose `recipe` names it, and
/// items without a recipe when the call is about one recipe only.
fn review_items_for(arguments: &Value, path: &Path, single: bool) -> Vec<ReviewItem> {
    let Some(items) = arguments.get("review_items").and_then(Value::as_array) else {
        return Vec::new();
    };
    let wanted = snapshot_key(path);
    items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let line_start = item.get("line_start").and_then(Value::as_u64)? as usize;
            let belongs = match item
                .get("recipe")
                .and_then(Value::as_str)
                .filter(|recipe| !recipe.is_empty())
            {
                Some(recipe) => {
                    let key = Path::new(recipe);
                    key == path
                        || (wanted.is_some() && snapshot_key(key) == wanted)
                        || (key.components().count() == 1 && key.file_name() == path.file_name())
                }
                None => single,
            };
            if !belongs {
                return None;
            }
            let id = match item.get("id") {
                Some(Value::String(id)) if !id.is_empty() => id.clone(),
                Some(Value::Number(id)) => id.to_string(),
                _ => format!("item{}", index + 1),
            };
            Some(ReviewItem {
                id,
                line_start,
                line_end: item
                    .get("line_end")
                    .and_then(Value::as_u64)
                    .map_or(line_start, |line| line as usize),
                suggestion: item
                    .get("suggestion")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                comment: item
                    .get("comment")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            })
        })
        .collect()
}

/// Ids in `no_change_needed` (strings or numbers).
fn acknowledged_items(arguments: &Value) -> Vec<String> {
    arguments
        .get("no_change_needed")
        .and_then(Value::as_array)
        .map(|ids| {
            ids.iter()
                .filter_map(|id| match id {
                    Value::String(id) => Some(id.clone()),
                    Value::Number(id) => Some(id.to_string()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Review item status of `path` against the file as first seen (never
/// rebaselined), or nothing when no item belongs to it.
fn item_statuses(arguments: &Value, path: &Path, single: bool, now: &str) -> Vec<ReviewItemStatus> {
    let items = review_items_for(arguments, path, single);
    if items.is_empty() {
        return Vec::new();
    }
    let Some(kept) = pristine(path) else {
        return Vec::new();
    };
    review_item_status(&kept, now, &items, &acknowledged_items(arguments))
}

/// The summary sentence about review items, empty when none were given.
fn review_items_note(statuses: &[ReviewItemStatus]) -> String {
    if statuses.is_empty() {
        return String::new();
    }
    let open = statuses
        .iter()
        .filter(|status| status.open)
        .map(|status| format!("{} ({})", status.id, status.status))
        .collect::<Vec<_>>();
    if open.is_empty() {
        return format!(
            " All {} review item(s) are answered by a change or marked no_change_needed.",
            statuses.len()
        );
    }
    format!(
        " {} of {} review item(s) are not answered: {}. The review fix is not done until each one has its change, or its id is in no_change_needed with the reason in the final answer.",
        open.len(),
        statuses.len(),
        open.join(", ")
    )
}

/// `eb_recipe_diff`: what changed in each recipe since this server first saw
/// it, hunk by hunk, so a final answer can describe exactly that.
fn recipe_diff(arguments: &Value) -> Result<Value, String> {
    let paths = recipe_paths(arguments)?;
    let mut files = Vec::new();
    let mut all_params = std::collections::BTreeSet::new();
    let mut total = 0usize;
    let mut statuses_all = Vec::new();
    for path in &paths {
        remember_first_seen(path);
        let now = std::fs::read_to_string(resolve_recipe_path(path))
            .map_err(|error| format!("read {}: {error}", path.display()))?;
        let Some(kept) = pristine(path) else {
            files.push(json!({"recipe": path, "created": true, "hunks": []}));
            continue;
        };
        let statuses = item_statuses(arguments, path, paths.len() == 1, &now);
        let hunks = crate::eb_style::hunks(&kept, &now);
        total += hunks.len();
        // A Python module has no top-level parameters; name the def or class
        // each hunk sits in instead, so the answer can say "configure_step".
        let python = path.extension().and_then(|value| value.to_str()) == Some("py");
        let rendered = hunks
            .iter()
            .map(|hunk| {
                all_params.extend(hunk.params.iter().cloned());
                let mut diff = String::new();
                for line in &hunk.removed {
                    diff.push_str(&format!("-{line}\n"));
                }
                for line in &hunk.added {
                    diff.push_str(&format!("+{line}\n"));
                }
                let scope = python
                    .then(|| {
                        crate::eb_module::enclosing_scope(&now, hunk.new_start.max(1)).or_else(
                            || crate::eb_module::enclosing_scope(&kept, hunk.old_start.max(1)),
                        )
                    })
                    .flatten();
                json!({
                    "old_line": hunk.old_start,
                    "new_line": hunk.new_start,
                    "parameters": hunk.params,
                    "scope": scope,
                    "comment_only": hunk.comment_only(),
                    "diff": diff,
                })
            })
            .collect::<Vec<_>>();
        let mut entry = json!({"recipe": path, "hunks": rendered});
        if !statuses.is_empty() {
            entry["review_items"] = json!(statuses);
        }
        files.push(entry);
        statuses_all.extend(statuses);
    }
    let params = all_params.into_iter().collect::<Vec<_>>();
    let summary = if total == 0 {
        "no changes since the files were first seen. If the review asked for a change, it is not made yet; if not, say that nothing needed changing.".to_string()
    } else {
        format!(
            "{total} hunk(s), parameters touched: {}. Your final answer must describe these hunks and nothing else: name every hunk, do not claim a change that has no hunk (for example a removal that was already absent, or a value you only meant to change), and do not call a comment-only hunk a change the review asked for. If a hunk is not something the review asked for or needs, revert it first.",
            if params.is_empty() { "none".to_string() } else { params.join(", ") }
        )
    };
    let summary = summary + &review_items_note(&statuses_all);
    let open = statuses_all.iter().filter(|status| status.open).count();
    Ok(json!({"files": files, "summary": summary, "parameters": params, "open_review_items": open}))
}

fn recipe_write(arguments: &Value) -> Result<Value, String> {
    let root = configured_workspace_root()?;
    let path = required_path(arguments, "path")?;
    remember_first_seen(&path);
    let written = write_workspace(&root, &path, &required_string(arguments, "content")?)?;
    serde_json::to_value(written).map_err(|error| error.to_string())
}

/// `eb_module_lint`: the easyblocks CI checks on Python modules.
fn module_lint(arguments: &Value) -> Result<Value, String> {
    let paths = path_array(arguments, "modules")?;
    if paths.is_empty() {
        return Err("argument modules requires at least one path".into());
    }
    let base = match optional_path(arguments, "base").filter(|base| !base.as_os_str().is_empty()) {
        Some(base) => Some(
            std::fs::read_to_string(&base)
                .map_err(|error| format!("read base {}: {error}", base.display()))?,
        ),
        None => None,
    };
    let flake8 = arguments
        .get("flake8")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let mut results = Vec::new();
    let mut reports = Vec::new();
    for path in &paths {
        remember_first_seen(path);
        let resolved = resolve_recipe_path(path);
        let text = std::fs::read_to_string(&resolved)
            .map_err(|error| format!("read {}: {error}", path.display()))?;
        // An unedited module keeps its errors (proof v7: a plain lint must
        // not call a file clean because it has not changed yet).
        let kept = base
            .clone()
            .or_else(|| pristine(path).filter(|kept| *kept != text));
        let (file, report) =
            crate::eb_module::lint_module(&resolved, &text, kept.as_deref(), flake8);
        results.push(file);
        reports.push(report);
    }
    let clean = results.iter().all(|file| file.clean);
    let errors: usize = results
        .iter()
        .map(|file| {
            file.findings
                .iter()
                .filter(|item| item.severity == Severity::Error && !item.info)
                .count()
        })
        .sum();
    let summary = if clean {
        "clean: no error outside the pre-existing ones. Check the generated commands with eb_easyblock_dry_run before calling the change done.".to_string()
    } else {
        format!("{errors} error(s) to fix before the module is clean")
    };
    Ok(json!({
        "clean": clean,
        "errors": errors,
        "results": results,
        "flake8": reports,
        "summary": summary,
    }))
}

fn recipe_lint(arguments: &Value) -> Result<Value, String> {
    let paths = recipe_paths(arguments)?;
    let refs = paths.iter().map(PathBuf::as_path).collect::<Vec<_>>();
    let review_lines = arguments
        .get("review_lines")
        .and_then(Value::as_array)
        .map(|lines| {
            lines
                .iter()
                .filter_map(Value::as_u64)
                .map(|line| line as usize)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    // `"base": ""` or `null` means no base (proof v5: "read base : No such file").
    let explicit_base =
        match optional_path(arguments, "base").filter(|base| !base.as_os_str().is_empty()) {
            Some(base) => Some(
                std::fs::read_to_string(&base)
                    .map_err(|error| format!("read base {}: {error}", base.display()))?,
            ),
            None => None,
        };
    // review_scope "file": a PR-level comment asked for the style everywhere,
    // so lines unchanged from base still count (proof v5, PR 27186).
    let file_scope = optional_string(arguments, "review_scope").as_deref() == Some("file");
    // review_lines_by_recipe: {recipe path: [lines]} for a call that lints
    // several recipes, each with its own anchors (proof v6: a two-recipe lint
    // call carried no anchors at all, so a hardcoded .so on an anchored line
    // never counted).
    let by_recipe = arguments
        .get("review_lines_by_recipe")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let lines_for = |path: &Path| -> Vec<usize> {
        let wanted = snapshot_key(path);
        by_recipe
            .iter()
            .find(|(key, _)| {
                let key = Path::new(key.as_str());
                key == path || (wanted.is_some() && snapshot_key(key) == wanted)
            })
            .and_then(|(_, lines)| lines.as_array())
            .map(|lines| {
                lines
                    .iter()
                    .filter_map(Value::as_u64)
                    .map(|line| line as usize)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| review_lines.clone())
    };
    let mut baselines = Vec::with_capacity(paths.len());
    let mut notes: Vec<String> = Vec::new();
    let scopes = paths
        .iter()
        .map(|path| {
            remember_first_seen(path);
            if optional_bool(arguments, "rebaseline") {
                // Refuse while a diff-mode error against the pristine copy is
                // outstanding off the review lines: rebaselining would only
                // hide it.
                let outstanding = pristine(path)
                    .and_then(|kept| {
                        let scope = RecipeScope {
                            base: Some(kept),
                            scope_to_base: false,
                            review_lines: lines_for(path.as_path()),
                            file_scope: false,
                        };
                        lint_recipe_files_scoped(&[path.as_path()], &[scope]).ok()
                    })
                    .map(|report| {
                        report
                            .files
                            .iter()
                            .flat_map(|file| file.findings.iter())
                            .filter(|finding| {
                                DIFF_MODE_ERRORS.contains(&finding.code.as_str())
                                    && !finding.in_review
                            })
                            .map(|finding| format!("{} (line {})", finding.code, finding.line))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                if outstanding.is_empty() {
                    rebaseline(path);
                } else {
                    notes.push(format!(
                        "rebaseline refused for {}: {} still compare(s) against the file as first seen. Restore what was lost (re-spelling an entry must keep the same path and every %(...)s template); if the review explicitly asked for the removal, pass its anchored lines as review_lines.",
                        path.display(),
                        outstanding.join(", ")
                    ));
                }
            }
            let (base, scope_to_base, baseline) = match &explicit_base {
                Some(text) => (Some(text.clone()), !file_scope, "base"),
                None => match first_seen(path) {
                    Some(text) => (Some(text), false, "first seen in this session"),
                    None => (None, false, "none"),
                },
            };
            baselines.push(baseline);
            RecipeScope {
                base,
                scope_to_base,
                review_lines: lines_for(path.as_path()),
                file_scope,
            }
        })
        .collect::<Vec<_>>();
    let mut report = lint_recipe_files_scoped(&refs, &scopes).map_err(|error| error.to_string())?;
    // Diff scope against the file as first seen (never rebaselined): hunks
    // far from every review line that are not the reviewed change repeated.
    if !file_scope {
        for (index, file) in report.files.iter_mut().enumerate() {
            let Some(path) = paths.get(index) else {
                continue;
            };
            let lines = lines_for(path.as_path());
            let (Some(kept), Ok(now)) = (pristine(path), std::fs::read_to_string(&file.path))
            else {
                continue;
            };
            file.findings
                .extend(crate::eb_style::diff_scope_findings(&kept, &now, &lines));
        }
    }
    // Pre-existing errors: the same code on the same line text in the
    // baseline. Out of scope unless a review line covers it (proof v6: a
    // pre-existing checksum-count error drove the agent to delete checksums
    // or invent sources in a review that never mentioned them).
    // review_scope "file" widens the formatting scope only (a PR-level "use
    // f-strings" comment): EB_FMT_* findings still count on every line, but
    // other errors the file already had stay advisory (proof v7: agents passed
    // review_scope "file" on two of three lint calls, and a pre-existing
    // EB_PY_NO_USER_SITE or EB_LOCAL_VAR_NAMING then blocked clean, so they
    // edited sanity commands and renamed variables no comment was about).
    let mut preexisting = 0usize;
    {
        for (index, file) in report.files.iter_mut().enumerate() {
            let Some(base) = scopes.get(index).and_then(|scope| scope.base.as_deref()) else {
                continue;
            };
            let base_lines: Vec<&str> = base.lines().collect();
            let base_findings = lint_style(base);
            // A review whose anchored lines carry no EB_FMT finding (before or
            // now) is not a formatting review: formatting the file already had
            // elsewhere is out of its scope too (proof v6: nine pre-existing
            // '%' findings made "clean" false in a one-option review and the
            // agent rewrote the whole file).
            let anchored = paths
                .get(index)
                .map(|path| lines_for(path.as_path()))
                .unwrap_or_default();
            let formatting_review = file_scope
                || anchored.is_empty()
                || base_findings
                    .iter()
                    .chain(file.findings.iter())
                    .any(|finding| {
                        is_formatting_code(&finding.code) && anchored.contains(&finding.line)
                    });
            let before: std::collections::HashSet<(String, String)> = base_findings
                .into_iter()
                .filter(|finding| {
                    finding.severity == Severity::Error
                        || (!formatting_review && is_formatting_code(&finding.code))
                })
                .map(|finding| {
                    let text = finding
                        .line
                        .checked_sub(1)
                        .and_then(|i| base_lines.get(i))
                        .map(|line| line.trim().to_string())
                        .unwrap_or_default();
                    (finding.code, text)
                })
                .collect();
            let now = std::fs::read_to_string(&file.path).unwrap_or_default();
            let now_lines: Vec<&str> = now.lines().collect();
            // A plain lint of an unedited file has no review to be outside
            // of: E501 and E999 still block clean. Demote a pre-existing
            // error once review lines were given, or once the file differs
            // from the baseline (the error survived an edit).
            let demote_errors = !anchored.is_empty() || now != base;
            for finding in file.findings.iter_mut() {
                let counted_fmt = !formatting_review && is_formatting_code(&finding.code);
                if (finding.severity != Severity::Error && !counted_fmt)
                    || finding.in_review
                    || finding.info
                    || DIFF_MODE_ERRORS.contains(&finding.code.as_str())
                    || (file_scope && is_formatting_code(&finding.code))
                {
                    continue;
                }
                let text = finding
                    .line
                    .checked_sub(1)
                    .and_then(|i| now_lines.get(i))
                    .map(|line| line.trim().to_string())
                    .unwrap_or_default();
                if before.contains(&(finding.code.clone(), text)) {
                    if finding.severity == Severity::Error && !demote_errors {
                        continue;
                    }
                    finding.info = true;
                    if counted_fmt {
                        finding.message.push_str(" (info: the file had this before your edits and the review is not about formatting; leave it)");
                        continue;
                    }
                    preexisting += 1;
                    finding.message.push_str(" (info: pre-existing, the file had this before your edits and the review does not cover the line; advisory only, leave it unless a review comment is about it. review_scope \"file\" does not make it count. Never delete or invent checksums, sources, crates, patches or sanity entries to clear it)");
                }
            }
        }
    }
    // Review items: each comment answered by a change, or still open
    // (proof v7: a prose comment with no hunk and empty suggestions left
    // unapplied still ended "clean").
    let mut statuses_by_file: Vec<Vec<ReviewItemStatus>> = Vec::new();
    for (index, file) in report.files.iter_mut().enumerate() {
        let statuses = match (paths.get(index), std::fs::read_to_string(&file.path)) {
            (Some(path), Ok(now)) => item_statuses(arguments, path, paths.len() == 1, &now),
            _ => Vec::new(),
        };
        file.findings.extend(review_item_findings(&statuses));
        statuses_by_file.push(statuses);
    }
    let count = |severity: Severity| {
        report
            .files
            .iter()
            .flat_map(|file| file.findings.iter())
            .filter(|finding| finding.severity == severity)
            .count()
    };
    let errors = report
        .files
        .iter()
        .flat_map(|file| file.findings.iter())
        .filter(|finding| finding.severity == Severity::Error && !finding.info)
        .count();
    let warnings = count(Severity::Warning);
    let all = || report.files.iter().flat_map(|file| file.findings.iter());
    let formatting = all()
        .filter(|finding| is_formatting_code(&finding.code) && !finding.info)
        .count();
    let info = all()
        .filter(|finding| finding.info && is_formatting_code(&finding.code))
        .count();
    let other_warnings = all()
        .filter(|finding| {
            finding.severity == Severity::Warning && !is_formatting_code(&finding.code)
        })
        .count();
    let mut other_codes = all()
        .filter(|finding| {
            finding.severity == Severity::Warning
                && !is_formatting_code(&finding.code)
                && !crate::eb_style::blocks_clean(finding)
        })
        .map(|finding| finding.code.clone())
        .collect::<Vec<_>>();
    other_codes.sort();
    other_codes.dedup();
    let results = report
        .files
        .iter()
        .enumerate()
        .map(|(index, file)| {
            let mut entry = json!({
                "recipe": file.path,
                "baseline": baselines.get(index).copied().unwrap_or("none"),
                "clean": file.clean(),
                "findings": file.findings,
            });
            if let Some(statuses) = statuses_by_file.get(index).filter(|s| !s.is_empty()) {
                entry["review_items"] = json!(statuses);
            }
            entry
        })
        .collect::<Vec<_>>();
    let clean = report.clean();
    let mut summary = lint_summary(clean, errors, formatting, other_warnings);
    if clean && !other_codes.is_empty() {
        summary.push_str(&format!(
            " Not blocking: {}. These still count when the review is about them: an inline comment anchored on that line, a PR-level comment about the same thing, or a review whose subject is that convention (for EB_SHLIB_HARDCODED: shared-library names, SHLIB_EXT or '.so'). Then fix every such line and lint with review_lines or review_scope \"file\".",
            other_codes.join(", ")
        ));
    }
    if info > 0 {
        summary.push_str(&format!(
            " {info} EB_FMT finding(s) are info (outside the review scope: unchanged from the base, or untouched %%-escaped entries) and do not count. Convert them too when the review asks for this change in general (a PR-level comment such as \"use f-strings\") or its comment is anchored on that line: lint again with review_scope \"file\" or with those review_lines."
        ));
    }
    if preexisting > 0 {
        summary.push_str(&format!(
            " {preexisting} pre-existing error(s) (info) were already in the file before your edits and are outside the review scope: they do not count, and error_free still reports them."
        ));
    }
    for note in &notes {
        summary.push(' ');
        summary.push_str(note);
    }
    let all_statuses = statuses_by_file.concat();
    summary.push_str(&review_items_note(&all_statuses));
    let open_items = all_statuses.iter().filter(|status| status.open).count();
    Ok(json!({
        "open_review_items": open_items,
        "info": info,
        "preexisting_errors": preexisting,
        "clean": clean,
        "error_free": report.error_free(),
        "errors": errors,
        "warnings": warnings,
        "formatting": formatting,
        "summary": summary,
        "results": results,
        "check_contrib": report.check_contrib,
    }))
}

/// The one-line verdict `eb_recipe_lint` returns. `clean` is false while any
/// error or any `EB_FMT_*` finding remains, so leftover `'%s' % X` formatting
/// reads as not done.
pub(crate) fn lint_summary(
    clean: bool,
    errors: usize,
    formatting: usize,
    other_warnings: usize,
) -> String {
    if clean && other_warnings == 0 {
        return "clean: no findings. If nothing else matches what the reviewer asked for, changing nothing is a valid result.".to_string();
    }
    if clean {
        return format!(
            "clean: no errors and no EB_FMT findings; {other_warnings} other warning(s) that do not block clean on their own."
        );
    }
    let mut parts = Vec::new();
    if errors > 0 {
        parts.push(format!("{errors} error(s)"));
    }
    if formatting > 0 {
        parts.push(format!(
            "{formatting} EB_FMT formatting finding(s) (for example '%s' % SHLIB_EXT left over)"
        ));
    }
    if other_warnings > 0 {
        parts.push(format!("{other_warnings} other warning(s)"));
    }
    format!(
        "not clean: {}. The review fix is not done: fix every error, and in a style or formatting review convert every EB_FMT finding the same way (f-strings for constants and locals, %(...)s templates kept), then lint again.",
        parts.join(", ")
    )
}

fn recipe_format(arguments: &Value) -> Result<Value, String> {
    let recipe = required_path(arguments, "recipe")?;
    remember_first_seen(&recipe);
    let before = std::fs::read_to_string(&recipe).map_err(|error| error.to_string())?;
    let before_findings = lint_style(&before);
    if optional_bool(arguments, "dry_run") {
        let result = format_style(&before);
        return Ok(json!({
            "recipe": recipe,
            "written": Value::Null,
            "text": result.text,
            "before": before_findings,
            "rewritten": result.lines_rewritten,
            "remaining": result.remaining
        }));
    }
    let Some(out) = optional_path(arguments, "out") else {
        let result = format_style_file(&recipe, None).map_err(|error| error.to_string())?;
        return Ok(json!({
            "recipe": recipe,
            "written": recipe,
            "before": before_findings,
            "rewritten": result.lines_rewritten,
            "remaining": result.remaining
        }));
    };
    if out.extension().and_then(|extension| extension.to_str()) != Some("eb") {
        return Err(format!(
            "out {} is not a .eb path; omit out to format the recipe in place, or set dry_run to get the text without writing a file",
            out.display()
        ));
    }
    let result = format_style(&before);
    let root = configured_workspace_root()?;
    let written = write_workspace(&root, &out, &result.text)?;
    Ok(json!({
        "recipe": recipe,
        "written": written.path,
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
    let baseline_tree = optional_path(arguments, "baseline_easyconfigs");
    let baseline_version = optional_string(arguments, "baseline_toolchain_version");
    let stacks: Vec<PathBuf> = string_array(arguments, "baseline_easystacks")?
        .into_iter()
        .map(PathBuf::from)
        .collect();
    let stack_refs = stacks.iter().map(PathBuf::as_path).collect::<Vec<_>>();
    let commits_from = optional_path(arguments, "baseline_commits_from");
    let baseline = if !stack_refs.is_empty() {
        if baseline_tree.is_some() {
            return Err("pass baseline_easyconfigs or baseline_easystacks, not both".into());
        }
        BaselineSource::Easystacks {
            files: &stack_refs,
            commits_repo: commits_from.as_deref(),
            also_toolchains: &[],
        }
    } else if let Some(root) = baseline_tree.as_deref() {
        BaselineSource::Tree {
            root,
            toolchain_version: baseline_version.as_deref(),
        }
    } else {
        BaselineSource::None
    };
    let lock = solve_with_baseline_source(
        &root_refs,
        &policy,
        baseline,
        &lock_out,
        optional_path(arguments, "sbom_out").as_deref(),
        SolveExtraOut {
            build_list_out: optional_path(arguments, "build_list_out").as_deref(),
            stack_diff_out: optional_path(arguments, "stack_diff_out").as_deref(),
            build_list_excludes_baseline: optional_bool(arguments, "build_list_excludes_baseline"),
            ..Default::default()
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

fn stack_sbom(arguments: &Value) -> Result<Value, String> {
    let lock_path = required_path(arguments, "lock")?;
    let out = optional_path(arguments, "out").unwrap_or_else(|| PathBuf::from("stack.cdx.json"));
    let lock: StackLock = load_json_file(&lock_path).map_err(|error| error.to_string())?;
    let sbom = lock_to_cyclonedx(&lock);
    write_json_pretty(&out, &sbom).map_err(|error| error.to_string())?;
    let components = sbom
        .get("components")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    Ok(json!({
        "lock": lock_path,
        "out": out,
        "components": components,
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
        "pypi" => Ok(Some(ForeignFormat::Pypi)),
        "cran" => Ok(Some(ForeignFormat::Cran)),
        "cargo" | "crates" | "crates.io" => Ok(Some(ForeignFormat::Cargo)),
        "luarocks" | "lua" => Ok(Some(ForeignFormat::Luarocks)),
        "raku" | "perl6" => Ok(Some(ForeignFormat::Raku)),
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

fn optional_bool(arguments: &Value, name: &str) -> bool {
    arguments
        .get(name)
        .and_then(Value::as_bool)
        .unwrap_or(false)
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

/// Accept either `recipes: [path, ...]` or a single `recipe` path.
fn recipe_paths(arguments: &Value) -> Result<Vec<PathBuf>, String> {
    if arguments.get("recipes").is_some() {
        return path_array(arguments, "recipes");
    }
    if let Some(path) = optional_path(arguments, "recipe") {
        return Ok(vec![path]);
    }
    Err("argument recipes requires at least one path".into())
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
        Some(Value::String(value)) => Ok(vec![value.clone()]),
        Some(_) => Err(format!("argument {name} must be an array of strings")),
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
