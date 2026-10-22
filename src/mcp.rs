//! MCP stdio surface for the version-one package, recipe, stack, target, and campaign workflows.

use crate::campaign::{
    claim_finding, resolve_finding, run_campaign, CampaignRequest, CampaignStatus,
    FindingResolution,
};
use crate::dep_versions::dep_versions;
use crate::domain::Toolchain;
use crate::eb_easyblock::{encode_class_name, index_easyblocks, EASYBLOCK_CLASS_PREFIX};
use crate::eb_parse::{
    check_recipe_deps, packaging_gate, parse_easyconfig_trees, resolve_easyconfig_file,
    resolve_easyconfig_file_reporting,
};
use crate::eb_run::{
    build_test_description, builds_allowed, commands_in, config_log_failure, dry_run_environment,
    dry_run_guard, dry_run_next, error_lines, failed_dry_run_text, failure_kind, find_config_log,
    last_step_in_file, lmod_error, parse_dry_run, read_tail, recipe_toolchain_options, run_eb,
    run_eb_with, split_steps, split_warnings, step_text, tail, threaded_findings,
    threaded_findings_text, threaded_note, threaded_pairs, EbInvocation, EbRun, ALLOW_BUILD_ENV,
};
use crate::eb_style::{
    format_style, format_style_file, hunks, is_formatting_code, lint_recipe_files_scoped,
    lint_style, python_syntax_error, review_item_findings, review_item_status,
    valid_easyconfig_parameters, RecipeScope, ReviewItem, ReviewItemStatus, Severity,
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
    apply_suggestion_workspace, configured_workspace_root, edit_workspace_checked, grep_workspace,
    list_workspace, read_workspace, resolve_inside, write_workspace,
};
use crate::{
    load_json_file, lock_to_cyclonedx, solve_with_baseline_source, write_json_pretty,
    BaselineSource, SolveExtraOut, StackLock,
};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

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
    let mut tools = vec![
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
                ("fetch_checksums", "boolean"),
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
            "Replace an exact literal string in a workspace file. old must be non-empty and different from new. When old matches more than once the file is left unchanged and the error lists the line and the list entry (component, extension or dependency name) of every match. within names one ('name', ...) entry so only matches inside it change; when two entries share the name, occurrence (1-based) picks one. Without within, occurrence picks the n-th match. Use replace_all only when the review asks for the change at every occurrence; the result's note says when one call changed several different entries. model_guard runs a post-edit gate for deterministic-then-model bump flows: deterministic fields are immutable and newly added top-level names must be valid EasyBuild parameters for the recipe easyblock.",
            &[("path", "string"), ("old", "string"), ("new", "string")],
            &[
                ("replace_all", "boolean"),
                ("within", "string"),
                ("occurrence", "integer"),
                ("model_guard", "boolean"),
            ],
        ),
        tool_with_optional(
            "eb_recipe_apply_suggestion",
            "Apply a review comment's code suggestion to the lines the comment is anchored on, the way GitHub's 'Commit suggestion' does: lines line_start..line_end (1-based; line_end defaults to line_start) are replaced by suggestion, and an empty or missing suggestion deletes them. Pass expect with the anchored lines as the review showed them: when earlier edits moved them, the unique place where they now are is used, and when they are not found nothing is written. line_start and line_end are the review's numbering: without expect, once the file has changed, the lines it had there when first seen are looked up instead. A suggestion that would leave the file invalid Python is not written. Use it for every suggestion block, including empty ones, before any other edit to those lines; then check the result against the rules (R1a) with eb_recipe_lint. model_guard runs a post-edit gate for deterministic-then-model bump flows: deterministic fields are immutable and newly added top-level names must be valid EasyBuild parameters for the recipe easyblock.",
            &[("path", "string"), ("line_start", "integer")],
            &[
                ("line_end", "integer"),
                ("suggestion", "string"),
                ("expect", "string"),
                ("model_guard", "boolean"),
            ],
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
            "Check a recipe the way `recipe check` does: strict parse, sources, maintainer, easyblock, packaging, and robot dependencies. A recipe that is not valid Python fails. easyconfigs must contain at least one path: a robot directory such as easybuild/easyconfigs. A .eb file there is accepted and stands for the easyconfigs directory it sits in.",
            &[("recipe", "string"), ("easyconfigs", "array_min1")],
            &[
                ("require_configopts", "array"),
                ("easyblocks", "array"),
                ("metadata_only", "boolean"),
                ("verify_sources", "boolean"),
                ("seeded_from", "string"),
                ("seeded_source_url", "string"),
                ("seeded_git", "string"),
                ("source_tree", "string"),
                ("strict", "boolean"),
            ],
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
            "Check Python modules that ship with recipes, such as easyblocks (easybuild/easyblocks/<letter>/<name>.py), the way the easyblocks CI does: E999 (not valid Python), E501 at the max-line-length of the nearest [flake8] section (setup.cfg, tox.ini or .flake8; easyblocks use 120), W291/W293 trailing whitespace, W292/W391 end of file, EBM_RUN_CMD (run_cmd/run_cmd_qa, removed in EasyBuild 5), the EBM_OLD_SUPER warning, and two warnings on easyconfig values: EBM_CFG_OVERRIDE (self.cfg['key'] set to a fixed value without checking it is unset, which overwrites the easyconfig's value) and EBM_TEST_STEP_OWN_CMD (a test_step with its own command that ignores runtest). Warnings do not block clean; read them before calling a change done. When flake8 is on PATH its findings (F401 unused import, F821 undefined name, C4 comprehensions, ...) are merged. Do not use eb_recipe_lint on .py files: its rules are for easyconfigs. base is the module before your edits (default: the module as this server first saw it); an error already there on an unchanged line is info and does not block clean. clean true is necessary but not enough: an easyblock change is done only when eb_easyblock_dry_run shows the commands you intended.",
            &[("modules", "array_min1")],
            &[("base", "string"), ("flake8", "boolean")],
        ),
        tool_with_optional(
            "eb_easyblock_dry_run",
            "Run eb -x (extended dry run: nothing is downloaded or compiled) on a recipe with your easyblock modules passed through --include-easyblocks, and return the commands the easyblock generates, grouped by step (configure, build, test, install, ...), with the error lines. Use it after every easyblock edit to see the configure options, make targets and test command your code produces for each recipe and toolchain you were asked about; a recipe with toolchainopts such as openmp is a separate call. environment lists the build environment the dry run reports (what the toolchain and dependencies define, in the form the easyblock reads it); env_filter keeps only names containing that text. toolchain_options is the recipe's toolchainopts as written, and threaded_pairs lists the variables that have a threaded variant (an MT part in the name) with a different value; notes says when the recipe turns a threaded option on, since the plain and threaded variables are both defined either way. recipe and easyblocks are workspace paths; robot adds easyconfig directories searched before the defaults; try_amend takes name=value easyconfig overrides. The full output is saved under .eb-stack/dry-run/ for eb_build_log. When the run fails, failure_kind is setup (EasyBuild never started because a module failed to load; errors has the module system's message), environment (the recipe's toolchain or dependencies cannot be set up here; no easyblock code ran) or easyblock (a Python error in a module), and warnings holds the WARNING lines EasyBuild printed about errors it ignored. next says what to do after a dry run on this server: build with eb_build_test when builds are on, since a dry run never links or runs the tests.",
            &[("recipe", "string"), ("easyblocks", "array_min1")],
            &[("robot", "array"), ("try_amend", "array"), ("timeout_s", "integer"), ("env_filter", "string")],
        ),
        tool_with_optional(
            "eb_build_log",
            "Read an EasyBuild log or eb output file in the workspace (for example the log an eb_easyblock_dry_run or eb_build_test call returned) by step. Returns the step list with line counts, the last tail lines (default 120) of the whole log or of one step (step: configure, build, test, install, sanitycheck, ...), optionally only lines containing grep, plus the commands and error lines in that part and the per-command output files EasyBuild named. Use it to read why a test or build step failed before changing code again.",
            &[("path", "string")],
            &[("step", "string"), ("grep", "string"), ("tail", "integer"), ("max_bytes", "integer")],
        ),
        tool_with_optional(
            "eb_build_test",
            &build_test_description(builds_allowed()),
            &[("recipe", "string"), ("easyblocks", "array_min1")],
            &[("robot", "array"), ("try_amend", "array"), ("timeout_s", "integer")],
        ),
        tool_with_optional(
            "eb_stack_solve",
            "Solve a jointly consistent EasyBuild stack lock with the same inputs as `stack solve`.",
            &[("easyconfigs", "array")],
            &[
                ("policy", "string"),
                ("toolchain", "string"),
                ("root", "array"),
                ("roots_from_build_list", "array"),
                ("baseline_easyconfigs", "string"),
                ("baseline_toolchain_version", "string"),
                ("baseline_easystack", "array"),
                ("baseline_commits_from", "string"),
                ("filter_deps", "array"),
                ("baseline_also_toolchain", "array"),
                ("lock_out", "string"),
                ("sbom_out", "string"),
                ("build_list_out", "string"),
                ("build_list_excludes_baseline", "boolean"),
                ("jenkins_build_list_out", "string"),
                ("jenkins_flags_from", "array"),
                ("jenkins_gpu_systems", "array"),
                ("jenkins_roots_only", "boolean"),
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
        tool_with_optional(
            "eb_campaign_finding_resolve",
            "Resolve an owned campaign finding with durable action and evidence.",
            &[
                ("state", "string"),
                ("id", "string"),
                ("owner", "string"),
                ("action", "string"),
                ("evidence", "string"),
            ],
            &[("changes", "array")],
        ),
    ];
    if let Some(lint) = tools
        .iter_mut()
        .find(|tool| tool["name"] == "eb_recipe_lint")
    {
        // The handler accepts `recipes` or a single `recipe`. A flat required
        // list cannot say that, and marking only `recipes` required rejects a
        // call the handler runs.
        lint["inputSchema"]["properties"]["recipe"] = json!({"type": "string"});
        lint["inputSchema"]["required"] = json!([]);
        lint["inputSchema"]["anyOf"] = json!([
            {"required": ["recipes"]},
            {"required": ["recipe"]}
        ]);
    }
    tools
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
        "eb_easyblock_dry_run" => easyblock_dry_run(arguments),
        "eb_build_log" => build_log(arguments),
        "eb_build_test" => build_test(arguments),
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
    // The schema and the CLI both require the name. `toolchain` defaults it
    // for inspect and plan, where the CLI default is foss; bump has no default.
    let target = Toolchain {
        name: required_string(arguments, "toolchain_name")?,
        version: required_string(arguments, "toolchain_version")?,
    };
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
    let request = BumpPackageRequest {
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
    };
    let (bundle, fetched) = if optional_bool(arguments, "fetch_checksums") {
        crate::source_fetch::plan_package_bump_fetching(
            &request,
            &options,
            &crate::registry::UreqClient,
        )
    } else {
        plan_package_bump_with(&request, &options).map(|bundle| (bundle, Vec::new()))
    }
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
        "fetched": fetched,
        "open": bundle.plan.residuals,
        "claims": {"resolves": true, "builds": false, "binary_verified": false}
    }))
}

/// Same easyblock search as `recipe check`: the bundle `easyblocks/` beside
/// `easyconfigs/` first, then the named roots. A named root makes that set
/// complete, so a missing `EB_` class is the same error the CLI bails with.
fn resolve_easyblock(
    recipe: &Path,
    name: &str,
    easyblock: Option<&str>,
    explicit_roots: &[PathBuf],
) -> Result<(Value, Option<String>), String> {
    let derived = easyblock.is_none();
    let class = easyblock
        .map(str::to_string)
        .unwrap_or_else(|| encode_class_name(name));
    let bundle_root = recipe
        .ancestors()
        .find(|ancestor| {
            ancestor
                .file_name()
                .is_some_and(|name| name == "easyconfigs")
        })
        .and_then(|easyconfigs| easyconfigs.parent())
        .map(|bundle| bundle.join("easyblocks"))
        .filter(|easyblocks| easyblocks.is_dir());
    let mut roots = Vec::new();
    if let Some(bundle) = bundle_root.as_deref() {
        roots.push(bundle);
    }
    roots.extend(explicit_roots.iter().map(PathBuf::as_path));
    let index = index_easyblocks(&roots)?;
    let found = index.get(&class);
    let missing = if found.is_none()
        && !explicit_roots.is_empty()
        && class.starts_with(EASYBLOCK_CLASS_PREFIX)
    {
        Some(format!(
            "No software-specific easyblock '{class}' found for {name}: none of the easyblock roots defines it"
        ))
    } else {
        None
    };
    let resolution = json!({
        "class": class,
        "derived_from_name": derived,
        "defined_in": found.map(|(path, _)| path.display().to_string()),
        "bases": found.map(|(_, class)| class.bases.clone()),
        "extra_options": found.map(|(_, class)| class.extra_options.clone()),
        "searched": roots.iter().map(|root| root.display().to_string()).collect::<Vec<_>>(),
    });
    Ok((resolution, missing))
}

fn recipe_check(arguments: &Value) -> Result<Value, String> {
    let path = required_path(arguments, "recipe")?;
    let source_text = std::fs::read_to_string(&path)
        .map_err(|error| format!("read {}: {error}", path.display()))?;
    if let Some((line, column, message)) = python_syntax_error(&source_text) {
        return Err(format!(
            "{}:{line}:{column}: {message}. EasyBuild cannot parse this easyconfig; fix the syntax (eb_recipe_lint reports it as E999) before checking it",
            path.display()
        ));
    }
    let easyconfigs = path_array(arguments, "easyconfigs")?
        .into_iter()
        .map(|root| robot_root(&root))
        .collect::<Result<Vec<_>, _>>()?;
    if optional_string(arguments, "seeded_source_url").is_some()
        && optional_string(arguments, "seeded_from").is_none()
    {
        return Err("seeded_source_url requires seeded_from".into());
    }
    if optional_string(arguments, "seeded_git").is_some()
        && optional_string(arguments, "seeded_from").is_none()
    {
        return Err("seeded_git requires seeded_from".into());
    }
    let verify_sources = optional_bool(arguments, "verify_sources");
    if !verify_sources
        && (optional_string(arguments, "seeded_from").is_some()
            || optional_path(arguments, "source_tree").is_some())
    {
        return Err("seeded_from and source_tree require verify_sources".into());
    }
    let strict = optional_bool(arguments, "strict");
    let (recipe, skipped) = if strict {
        resolve_easyconfig_file_reporting(&path).map_err(|error| error.to_string())?
    } else {
        (
            resolve_easyconfig_file(&path).map_err(|error| error.to_string())?,
            Vec::new(),
        )
    };
    let mut sources = Vec::new();
    let mut source_findings = Vec::new();
    let mut declared_version = Value::Null;
    if verify_sources {
        let seed = optional_string(arguments, "seeded_from").map(|origin| crate::SeededChecksum {
            origin,
            source_url: optional_string(arguments, "seeded_source_url"),
            git: optional_string(arguments, "seeded_git"),
            sha256: recipe.checksums.first().cloned(),
        });
        let mut findings = crate::verify_sources(&recipe.source_urls, seed.as_ref());
        if let Some(tree) = optional_path(arguments, "source_tree") {
            let declared = crate::declared_version(&tree);
            if let Some(found) = declared.as_ref() {
                declared_version = json!({"value": found.value, "source": found.source});
            }
            findings.extend(crate::verify_declared_version(
                &recipe.version,
                declared.as_ref(),
            ));
        }
        for url in &recipe.source_urls {
            sources.push(json!({
                "class": crate::classify_url(url).to_string(),
                "url": url,
            }));
        }
        source_findings = findings.iter().map(|finding| finding.to_string()).collect();
    }
    let required_options = string_array(arguments, "require_configopts")?;
    let option_refs = required_options
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let packaging = packaging_gate(&recipe, &option_refs);
    let maintainer = crate::check_maintainer_acceptability(&recipe, &source_text);
    let easyblock_paths = string_array(arguments, "easyblocks")?
        .into_iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    let (easyblock_resolution, easyblock_error) = resolve_easyblock(
        &path,
        &recipe.name,
        recipe.easyblock.as_deref(),
        &easyblock_paths,
    )?;
    let metadata_only = optional_bool(arguments, "metadata_only");
    let mut dep_ok = true;
    let mut toolchain_verified = true;
    let mut duplicate_error = None;
    let dependency_check = if metadata_only {
        Value::Null
    } else {
        let root_refs = easyconfigs.iter().map(PathBuf::as_path).collect::<Vec<_>>();
        let tree = parse_easyconfig_trees(&root_refs).map_err(|error| error.to_string())?;
        if let Some(dup) = crate::check_duplicate_upstream(&recipe, &tree.candidates)
            .into_iter()
            .find(|finding| finding.is_error())
        {
            duplicate_error = Some(format!("{}: {}", dup.code, dup.message));
        }
        let check = check_recipe_deps(&recipe, &tree.candidates);
        dep_ok = check.ok();
        toolchain_verified = check.toolchain_verified();
        serde_json::to_value(&check).map_err(|error| error.to_string())?
    };
    let packaging_errors = packaging.err().unwrap_or_default();
    let source_failed = source_findings
        .iter()
        .any(|line: &String| line.starts_with("[err]"));
    // A clean dependency list is a resolve only with a real toolchain
    // hierarchy, and only after the maintainer, easyblock, source, and strict
    // gates the CLI bails on. metadata_only stops before the robot tree, as
    // `recipe check --metadata-only` does.
    let resolves = packaging_errors.is_empty()
        && maintainer.ok_for_upstream()
        && easyblock_error.is_none()
        && !source_failed
        && !(strict && !skipped.is_empty())
        && duplicate_error.is_none()
        && (metadata_only || (dep_ok && toolchain_verified));
    let skipped_text: Vec<String> = skipped.iter().map(ToString::to_string).collect();
    Ok(json!({
        "recipe": recipe.easyconfig_path,
        "resolves": resolves,
        "dependency_check": dependency_check,
        "packaging_errors": packaging_errors,
        "claims": {"resolves": resolves, "builds": false, "binary_verified": false},
        "strict": if strict { json!({"skipped": skipped_text}) } else { Value::Null },
        "sources": sources,
        "source_findings": source_findings,
        "declared_version": declared_version,
        "maintainer_acceptability": maintainer,
        "easyblock_resolution": easyblock_resolution,
        "easyblock_error": easyblock_error,
        "duplicate_error": duplicate_error,
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

/// Top-level assignment names (`name = ...` / `name += ...`) in file order.
fn top_level_assignment_names(text: &str) -> BTreeSet<String> {
    text.lines()
        .filter(|line| !line.starts_with([' ', '\t', '#']))
        .filter_map(|line| {
            let name: String = line
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if name.is_empty() {
                return None;
            }
            let rest = line[name.len()..].trim_start();
            (rest.starts_with('=') || rest.starts_with("+=")).then_some(name)
        })
        .collect()
}

/// `easyblock = 'Class'` / `easyblock = "Class"` at top level.
fn easyblock_name(text: &str) -> Option<String> {
    text.lines()
        .filter(|line| !line.starts_with([' ', '\t', '#']))
        .find_map(|line| {
            let (name, value) = line.split_once('=')?;
            if name.trim() != "easyblock" {
                return None;
            }
            let trimmed = value.trim();
            for quote in ['\'', '"'] {
                if let Some(body) = trimmed.strip_prefix(quote) {
                    let end = body.find(quote)?;
                    let class = body[..end].trim();
                    return (!class.is_empty()).then_some(class.to_string());
                }
            }
            None
        })
}

/// Post-model guard for deterministic bump recipes.
///
/// When enabled, an edit is rolled back if it modifies a deterministic field
/// from the first-seen baseline or introduces a top-level name that is not a
/// valid EasyBuild parameter for the recipe's easyblock.
fn enforce_model_guard(root: &Path, path: &Path, baseline: &str) -> Result<(), String> {
    let after = read_workspace(root, path)?.content;
    let deterministic = top_level_assignment_names(baseline);
    let touched = hunks(baseline, &after)
        .into_iter()
        .flat_map(|hunk| hunk.params.into_iter())
        .collect::<BTreeSet<_>>();
    let mut changed_deterministic = touched
        .iter()
        .filter(|name| deterministic.contains(*name))
        .cloned()
        .collect::<Vec<_>>();
    changed_deterministic.sort();
    changed_deterministic.dedup();

    let current = top_level_assignment_names(&after);
    let introduced = current
        .difference(&deterministic)
        .cloned()
        .collect::<Vec<_>>();
    let easyblock = easyblock_name(&after).or_else(|| easyblock_name(baseline));
    let valid = valid_easyconfig_parameters(easyblock.as_deref());
    let mut invalid_added = introduced
        .into_iter()
        .filter(|name| !valid.contains(name))
        .collect::<Vec<_>>();
    invalid_added.sort();
    invalid_added.dedup();

    if changed_deterministic.is_empty() && invalid_added.is_empty() {
        return Ok(());
    }

    write_workspace(root, path, baseline)?;
    let mut reasons = Vec::new();
    if !changed_deterministic.is_empty() {
        reasons.push(format!(
            "deterministic fields changed: {}",
            changed_deterministic.join(", ")
        ));
    }
    if !invalid_added.is_empty() {
        reasons.push(format!(
            "invalid top-level names added: {}",
            invalid_added.join(", ")
        ));
    }
    Err(format!(
        "not applied: post-model guard reverted this edit ({})",
        reasons.join("; ")
    ))
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
    let guarded = optional_bool(arguments, "model_guard");
    let baseline = if guarded { pristine(&path) } else { None };
    if guarded && baseline.is_none() {
        return Err(format!(
            "model_guard needs an existing baseline for {}",
            path.display()
        ));
    }
    let within = optional_string(arguments, "within").filter(|name| !name.is_empty());
    let occurrence = arguments
        .get("occurrence")
        .and_then(Value::as_u64)
        .map(|value| value as usize);
    // A module edit that leaves invalid Python is undone (proof-easyblock r2:
    // a pasted `+` line shipped after five lint runs).
    let edited = edit_workspace_checked(
        &root,
        &path,
        &required_string(arguments, "old")?,
        &required_string(arguments, "new")?,
        optional_bool(arguments, "replace_all"),
        within.as_deref(),
        occurrence,
    )?;
    if let Some(kept) = baseline.as_deref() {
        enforce_model_guard(&root, &path, kept)?;
    }
    serde_json::to_value(edited).map_err(|error| error.to_string())
}

/// `eb_recipe_apply_suggestion`: put a review suggestion on the lines its
/// comment is anchored to (an empty suggestion deletes them).
fn recipe_apply_suggestion(arguments: &Value) -> Result<Value, String> {
    let root = configured_workspace_root()?;
    let path = required_path(arguments, "path")?;
    remember_first_seen(&path);
    let guarded = optional_bool(arguments, "model_guard");
    let baseline = if guarded { pristine(&path) } else { None };
    if guarded && baseline.is_none() {
        return Err(format!(
            "model_guard needs an existing baseline for {}",
            path.display()
        ));
    }
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
    if let Some(kept) = baseline.as_deref() {
        enforce_model_guard(&root, &path, kept)?;
    }
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
    let warnings: usize = results
        .iter()
        .map(|file| {
            file.findings
                .iter()
                .filter(|item| item.severity == Severity::Warning && !item.info)
                .count()
        })
        .sum();
    // Name each warning in the summary: a count alone was read as clean
    // (proof-easyblock r2).
    let listed = results
        .iter()
        .flat_map(|file| file.findings.iter())
        .filter(|item| item.severity == Severity::Warning && !item.info)
        .take(3)
        .map(|item| {
            let message: String = item.message.chars().take(240).collect();
            format!("{} line {}: {message}", item.code, item.line)
        })
        .collect::<Vec<_>>()
        .join("; ");
    let summary = if clean && warnings > 0 {
        format!("clean: no error outside the pre-existing ones, but {warnings} new warning(s) to fix or justify before calling the change done: {listed}. Then check the generated commands with eb_easyblock_dry_run.")
    } else if clean {
        "clean: no error outside the pre-existing ones. Check the generated commands with eb_easyblock_dry_run before calling the change done.".to_string()
    } else {
        format!("{errors} error(s) to fix before the module is clean")
    };
    Ok(json!({
        "clean": clean,
        "errors": errors,
        "warnings": warnings,
        "results": results,
        "flake8": reports,
        "summary": summary,
    }))
}

/// The recipe, easyblocks, robot paths and overrides of an eb tool call.
fn eb_invocation(arguments: &Value) -> Result<EbInvocation, String> {
    Ok(EbInvocation {
        recipe: required_path(arguments, "recipe")?,
        easyblocks: path_array(arguments, "easyblocks")?,
        robot: string_array(arguments, "robot")?
            .into_iter()
            .map(PathBuf::from)
            .collect(),
        try_amend: string_array(arguments, "try_amend")?,
    })
}

/// A fresh numbered directory under `<root>/.eb-stack/<kind>/`.
fn eb_run_dir(root: &Path, kind: &str) -> Result<PathBuf, String> {
    let base = root.join(".eb-stack").join(kind);
    std::fs::create_dir_all(&base)
        .map_err(|error| format!("create {}: {error}", base.display()))?;
    let taken = std::fs::read_dir(&base)
        .map_err(|error| format!("list {}: {error}", base.display()))?
        .count();
    let dir = base.join(format!("{:03}", taken + 1));
    std::fs::create_dir_all(&dir).map_err(|error| format!("create {}: {error}", dir.display()))?;
    Ok(dir)
}

fn workspace_relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(|relative| relative.display().to_string())
        .unwrap_or_else(|_| path.display().to_string())
}

fn timeout_arg(arguments: &Value, default: u64, min: u64, max: u64) -> Duration {
    Duration::from_secs(
        arguments
            .get("timeout_s")
            .and_then(Value::as_u64)
            .unwrap_or(default)
            .clamp(min, max),
    )
}

/// A missing `eb` binary is an environment failure: no easyblock code ran.
///
/// `run_eb` returns that as a tool error. Write it where the dry-run parser
/// reads errors so `failure_kind` can classify it with every other environment
/// failure instead of leaving the caller with an unclassified tool error.
fn run_eb_or_absent_with(
    args: &[String],
    limit: Duration,
    output: &Path,
    extra_env: &[(String, String)],
) -> Result<EbRun, String> {
    match run_eb_with(args, limit, output, extra_env) {
        Ok(run) => Ok(run),
        Err(error) if error.contains("eb is not on PATH") || error.contains("EB_STACK_EB=") => {
            std::fs::write(output, format!("ERROR: {error}\n"))
                .map_err(|err| format!("write {}: {err}", output.display()))?;
            Ok(EbRun {
                argv: vec!["eb".into()],
                exit_code: Some(127),
                timed_out: false,
                seconds: 0,
                output_file: output.to_path_buf(),
            })
        }
        Err(error) => Err(error),
    }
}

/// `eb_easyblock_dry_run`: the commands an easyblock generates, from `eb -x`.
fn easyblock_dry_run(arguments: &Value) -> Result<Value, String> {
    let root = configured_workspace_root()?;
    let invocation = eb_invocation(arguments)?.resolved(&root)?;
    for path in &invocation.easyblocks {
        remember_first_seen(path);
    }
    let dir = eb_run_dir(&root, "dry-run")?;
    let output = dir.join("eb.out");
    let guard = dry_run_guard(&dir)?;
    let run = run_eb_or_absent_with(
        &invocation.dry_run_args(&dir),
        timeout_arg(arguments, 600, 10, 3600),
        &output,
        &guard,
    )?;
    let (text, _) = read_tail(&output, 4 * 1024 * 1024)?;
    let lines = text.lines().collect::<Vec<_>>();
    let steps = parse_dry_run(&text);
    // The plain and threaded library variables are defined whatever the
    // recipe's toolchain options say; report the options and the pairs so a
    // threaded build is not judged by an unchanged environment.
    let toolchain_options =
        recipe_toolchain_options(&std::fs::read_to_string(&invocation.recipe).unwrap_or_default());
    let pairs = threaded_pairs(&dry_run_environment(&text, None));
    let mut notes = Vec::new();
    if let Some(note) = threaded_note(&toolchain_options, &pairs) {
        notes.push(note);
    }
    let (errors, warnings) = split_warnings(error_lines(&text, 20));
    let kind = if run.exit_code == Some(0) {
        None
    } else {
        failure_kind(&errors)
    };
    let (failed_summary, failed_next) = failed_dry_run_text(kind, &errors);
    let next = if run.exit_code == Some(0) {
        dry_run_next(builds_allowed(), !steps.is_empty())
    } else {
        failed_next
    };
    // A command that still carries a plain value under a threaded option is
    // a finding, not a note: it decides summary and next (proof-easyblock r2:
    // a conditional note under a success summary was never acted on).
    let findings = threaded_findings(&toolchain_options, &pairs, &steps);
    let threaded_text = if run.exit_code == Some(0) {
        threaded_findings_text(&toolchain_options, &findings)
    } else {
        None
    };
    let next = match &threaded_text {
        Some((_, edit)) => edit.clone(),
        None => next.to_string(),
    };
    let summary = match (run.exit_code, steps.is_empty()) {
        (Some(0), false) if threaded_text.is_some() => threaded_text
            .as_ref()
            .map(|(text, _)| text.clone())
            .unwrap_or_default(),
        (Some(0), false) => {
            "dry run finished: check each step's commands against what the task asks for"
                .to_string()
        }
        (Some(0), true) => {
            "dry run finished but reported no commands: read the output with eb_build_log"
                .to_string()
        }
        _ => failed_summary,
    };
    Ok(json!({
        "exit_code": run.exit_code,
        "timed_out": run.timed_out,
        "seconds": run.seconds,
        "argv": run.argv,
        "output": workspace_relative(&root, &output),
        "steps": steps,
        "environment": dry_run_environment(&text, optional_string(arguments, "env_filter").as_deref()),
        "toolchain_options": toolchain_options,
        "threaded_pairs": pairs,
        "threaded_findings": findings,
        "notes": notes,
        "errors": errors,
        "warnings": warnings,
        "failure_kind": kind,
        "tail": tail(&lines, 40),
        "summary": summary,
        "next": next,
        "claims": {"builds": false, "tests_ran": false},
    }))
}

/// `eb_build_log`: one step of an EasyBuild log, bounded.
fn build_log(arguments: &Value) -> Result<Value, String> {
    let root = configured_workspace_root()?;
    let requested = required_path(arguments, "path")?;
    let (_, path) = resolve_inside(&root, &requested)?;
    let max_bytes = arguments
        .get("max_bytes")
        .and_then(Value::as_u64)
        .unwrap_or(8 * 1024 * 1024)
        .clamp(1024, 64 * 1024 * 1024);
    let (text, start_truncated) = read_tail(&path, max_bytes)?;
    let steps = split_steps(&text);
    let step = optional_string(arguments, "step").filter(|step| !step.is_empty());
    // A step whose output alone is longer than the tail leaves no step marker
    // in it (proof-easyblock r2: "no build step ...; steps: " on a build that
    // failed in the build step). The whole file says which step the tail is in.
    let whole_file_step = if start_truncated && steps.is_empty() {
        last_step_in_file(&path)
    } else {
        None
    };
    let mut note = None;
    let selected = match step.as_deref() {
        Some(name) => match step_text(&text, &steps, name) {
            Some(lines) => lines,
            None if whole_file_step
                .as_ref()
                .is_some_and(|(found, _)| found == name) =>
            {
                note = Some(format!(
                    "the {name} step starts at line {} of the whole log, before the last {max_bytes} bytes read here, so all of them are shown as that step",
                    whole_file_step.as_ref().map_or(0, |(_, line)| *line)
                ));
                text.lines().collect()
            }
            None => {
                let listed = steps
                    .iter()
                    .map(|span| span.step.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(match &whole_file_step {
                    Some((found, line)) => format!(
                        "no {name} step in the last {max_bytes} bytes of {}: they are all inside the {found} step (it starts at line {line} of the whole log). Call again with step \"{found}\", or without step and with grep",
                        requested.display()
                    ),
                    None => format!("no {name} step in {}; steps: {listed}", requested.display()),
                });
            }
        },
        None => text.lines().collect(),
    };
    let shown = match optional_string(arguments, "grep").filter(|pattern| !pattern.is_empty()) {
        Some(pattern) => selected
            .iter()
            .copied()
            .filter(|line| line.contains(pattern.as_str()))
            .collect::<Vec<_>>(),
        None => selected.clone(),
    };
    let count = arguments
        .get("tail")
        .and_then(Value::as_u64)
        .map_or(120, |value| value as usize)
        .clamp(1, 600);
    let mut body = tail(&shown, count);
    if body.len() > 20_000 {
        let cut = body.len() - 20_000;
        let cut = (cut..body.len())
            .find(|index| body.is_char_boundary(*index))
            .unwrap_or(cut);
        body = body[cut..].to_string();
    }
    let part = selected.join("\n");
    Ok(json!({
        "path": workspace_relative(&root, &path),
        "steps": steps,
        "step": step,
        "lines_selected": selected.len(),
        "lines_matched": shown.len(),
        "text": body,
        "commands": commands_in(&part),
        "errors": error_lines(&part, 20),
        "nested_outputs": crate::campaign::easybuild_output_paths(&text),
        "start_truncated": start_truncated,
        "whole_log_last_step": whole_file_step.map(|(found, _)| found),
        "note": note,
    }))
}

/// `eb_build_test`: one real build with workspace easyblocks, refused unless
/// [`ALLOW_BUILD_ENV`] is `1`.
fn build_test(arguments: &Value) -> Result<Value, String> {
    if !builds_allowed() {
        return Err(format!(
            "builds are off on this server ({ALLOW_BUILD_ENV}=1 is not set). Use eb_easyblock_dry_run to check the commands your easyblock generates; the build runs where builds are allowed."
        ));
    }
    let root = configured_workspace_root()?;
    let invocation = eb_invocation(arguments)?.resolved(&root)?;
    for path in &invocation.easyblocks {
        remember_first_seen(path);
    }
    let dir = eb_run_dir(&root, "builds")?;
    let output = dir.join("eb.out");
    let extra = vec![
        "--rebuild".to_string(),
        "--logtostdout".to_string(),
        "--trace".to_string(),
        "--disable-cleanup-tmpdir".to_string(),
        format!("--tmpdir={}", dir.join("tmp").display()),
        format!("--buildpath={}", dir.join("build").display()),
    ];
    let run = run_eb(
        &invocation.args(&extra),
        timeout_arg(arguments, 7200, 60, 6 * 3600),
        &output,
    )?;
    let (text, start_truncated) = read_tail(&output, 16 * 1024 * 1024)?;
    let steps = split_steps(&text);
    let completed = run.exit_code == Some(0)
        && (text.contains("COMPLETED: Installation ended successfully")
            || text.contains("Build succeeded for 1 out of 1"));
    // No step marker in the tail: the whole file says which step it is in.
    let whole_file_step = if start_truncated && steps.is_empty() {
        last_step_in_file(&output).map(|(found, _)| found)
    } else {
        None
    };
    let last_step = steps
        .last()
        .map(|span| span.step.clone())
        .or_else(|| whole_file_step.clone());
    let test_lines = step_text(&text, &steps, "test")
        .or_else(|| (whole_file_step.as_deref() == Some("test")).then(|| text.lines().collect()));
    let test = test_lines.map(|lines| {
        let part = lines.join("\n");
        json!({
            "commands": commands_in(&part),
            "errors": error_lines(&part, 20),
            "tail": tail(&lines, 80),
        })
    });
    let test_ran = test
        .as_ref()
        .and_then(|test| test["commands"].as_array())
        .is_some_and(|commands| !commands.is_empty());
    // A failed configure says why in config.log, not in the EasyBuild log
    // (proof-easyblock r2: trials stopped after a configure failure without
    // reading it).
    let config_log = if !completed && last_step.as_deref() == Some("configure") {
        find_config_log(&dir.join("build")).and_then(|found| {
            let failure = config_log_failure(&std::fs::read_to_string(&found).ok()?)?;
            Some((workspace_relative(&root, &found), failure))
        })
    } else {
        None
    };
    let summary = match (completed, test_ran) {
        (true, true) => {
            "installed, and the test step ran the commands listed under test".to_string()
        }
        (true, false) => {
            "installed, but the test step ran no command: nothing was tested".to_string()
        }
        (false, _) if run.timed_out => "killed at the time limit".to_string(),
        (false, _) => match (&config_log, last_step.as_deref()) {
            (Some((found, failure)), _) => format!(
                "failed in the configure step; config.log ({found}) shows the last failed check, 'checking {}' at its line {}: config_log.excerpt has that check's commands and compiler or linker output. Fix what that output shows, then build again",
                failure.check, failure.line
            ),
            (None, Some(step)) if step != "start" => format!(
                "failed in the {step} step: read errors (the first lines name the cause), then eb_build_log with step \"{step}\""
            ),
            _ => match lmod_error(&text) {
                // proof-easyblock r3: a module load failed before EasyBuild
                // started, and the summary did not say so.
                Some(message) => format!(
                    "failed before EasyBuild started: {message}. No easyblock code ran, so this is not a fault in your easyblock; say that the build could not start on this server and quote the error"
                ),
                None => "failed: read errors, then the failing step with eb_build_log".to_string(),
            },
        },
    };
    Ok(json!({
        "exit_code": run.exit_code,
        "timed_out": run.timed_out,
        "seconds": run.seconds,
        "completed": completed,
        "last_step": last_step,
        "steps": steps,
        "log": workspace_relative(&root, &output),
        "build_dir": workspace_relative(&root, &dir.join("build")),
        "test": test,
        "test_ran": test_ran,
        "errors": error_lines(&text, 20),
        "config_log": config_log.map(|(found, failure)| json!({
            "path": found,
            "check": failure.check,
            "line": failure.line,
            "excerpt": failure.excerpt,
        })),
        "nested_outputs": crate::campaign::easybuild_output_paths(&text),
        "start_truncated": start_truncated,
        "summary": summary,
        "claims": {"builds": true, "tests_ran": test_ran},
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
            let findings = file
                .findings
                .iter()
                .map(|finding| {
                    let mut item = json!({
                        "path": file.path.display().to_string(),
                        "line": finding.line,
                        "column": finding.column,
                        "code": finding.code,
                        "message": finding.message,
                        "severity": finding.severity,
                        "mechanical": finding.mechanical,
                    });
                    if !finding.origin.is_empty() {
                        item["origin"] = json!(finding.origin);
                    }
                    if finding.info {
                        item["info"] = json!(true);
                    }
                    if finding.in_review {
                        item["in_review"] = json!(true);
                    }
                    item
                })
                .collect::<Vec<_>>();
            let mut entry = json!({
                "recipe": file.path,
                "baseline": baselines.get(index).copied().unwrap_or("none"),
                "clean": file.clean(),
                "findings": findings,
            });
            if let Ok(text) = std::fs::read_to_string(&file.path) {
                let maintainer = crate::check_maintainer_acceptability_text(&text);
                entry["maintainer_acceptability"] = json!(maintainer
                    .findings
                    .into_iter()
                    .map(|finding| {
                        json!({
                            "path": file.path.display().to_string(),
                            "code": finding.code,
                            "severity": finding.severity,
                            "message": finding.message,
                            "evidence": finding.evidence,
                        })
                    })
                    .collect::<Vec<_>>());
            }
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
    let easyconfigs = path_array(arguments, "easyconfigs")?;
    let mut roots = string_array(arguments, "root")?;
    let list_paths = string_array(arguments, "roots_from_build_list")?;
    for path in &list_paths {
        let text = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
        for line in crate::jenkins::parse_build_list(&text) {
            if let Some(name) = crate::jenkins::package_name(&line.file) {
                if !roots.iter().any(|root| root == name) {
                    roots.push(name.to_string());
                }
            }
        }
    }
    let toolchain = optional_string(arguments, "toolchain");
    let policy_arg = optional_path(arguments, "policy");
    if policy_arg.is_some() && (toolchain.is_some() || !roots.is_empty() || !list_paths.is_empty())
    {
        return Err("pass policy or toolchain and root, not both".into());
    }
    if toolchain.is_none() && policy_arg.is_none() {
        return Err("pass policy, or toolchain as name/version with root".into());
    }
    if toolchain.is_some() && roots.is_empty() {
        return Err("toolchain needs at least one root or roots_from_build_list".into());
    }
    let generated;
    let policy = if let Some(path) = policy_arg {
        path
    } else {
        let spec = toolchain.as_deref().expect("toolchain checked above");
        let (name, version) = spec
            .split_once('/')
            .ok_or_else(|| "toolchain wants name/version, for example foss/2026.1".to_string())?;
        generated = std::env::temp_dir().join(format!(
            "eb-stack-policy-{name}-{version}-{}.json",
            std::process::id()
        ));
        let body = json!({
            "toolchain": {"name": name, "version": version},
            "roots": roots,
            "objective": "prefer_newer",
        });
        std::fs::write(
            &generated,
            serde_json::to_vec_pretty(&body).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        generated
    };
    let stacks = string_array(arguments, "baseline_easystack")?
        .into_iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    let baseline_tree = optional_path(arguments, "baseline_easyconfigs");
    let baseline_version = optional_string(arguments, "baseline_toolchain_version");
    if !stacks.is_empty() && (baseline_tree.is_some() || baseline_version.is_some()) {
        return Err(
            "baseline_easystack conflicts with baseline_easyconfigs and baseline_toolchain_version"
                .into(),
        );
    }
    let commits_from = optional_path(arguments, "baseline_commits_from");
    if commits_from.is_some() && stacks.is_empty() {
        return Err("baseline_commits_from requires baseline_easystack".into());
    }
    let also_specs = string_array(arguments, "baseline_also_toolchain")?;
    if !also_specs.is_empty() && stacks.is_empty() {
        return Err("baseline_also_toolchain requires baseline_easystack".into());
    }
    let also = also_specs
        .iter()
        .map(|spec| {
            let (name, version) = spec
                .split_once('/')
                .ok_or_else(|| format!("baseline_also_toolchain wants name/version, got {spec}"))?;
            Ok(Toolchain {
                name: name.into(),
                version: version.into(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let stack_refs = stacks.iter().map(PathBuf::as_path).collect::<Vec<_>>();
    let baseline = if stacks.is_empty() {
        match baseline_tree
            .as_deref()
            .or_else(|| easyconfigs.first().map(PathBuf::as_path))
        {
            Some(root) => BaselineSource::Tree {
                root,
                toolchain_version: baseline_version.as_deref(),
            },
            None => BaselineSource::None,
        }
    } else {
        BaselineSource::Easystacks {
            files: &stack_refs,
            commits_repo: commits_from.as_deref(),
            also_toolchains: &also,
        }
    };
    let filter_deps = string_array(arguments, "filter_deps")?;
    let jenkins_out = optional_path(arguments, "jenkins_build_list_out");
    let flags_from = string_array(arguments, "jenkins_flags_from")?;
    let gpu_systems = string_array(arguments, "jenkins_gpu_systems")?;
    let roots_only = optional_bool(arguments, "jenkins_roots_only");
    if jenkins_out.is_none() && (!flags_from.is_empty() || !gpu_systems.is_empty() || roots_only) {
        return Err(
            "jenkins_flags_from, jenkins_gpu_systems, and jenkins_roots_only require jenkins_build_list_out"
                .into(),
        );
    }
    let mut previous = Vec::new();
    for path in &flags_from {
        let text = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
        previous.extend(crate::jenkins::parse_build_list(&text));
    }
    let only = if roots_only {
        let value: Value = load_json_file(&policy).map_err(|error| error.to_string())?;
        let list = value
            .get("roots")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("jenkins_roots_only needs roots in {}", policy.display()))?;
        Some(
            list.iter()
                .filter_map(|root| root.as_str().map(str::to_string))
                .collect(),
        )
    } else {
        None
    };
    let lock_out =
        optional_path(arguments, "lock_out").unwrap_or_else(|| PathBuf::from("stack.lock.json"));
    let root_refs = easyconfigs.iter().map(PathBuf::as_path).collect::<Vec<_>>();
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
            filter_deps: &filter_deps,
            jenkins_build_list_out: jenkins_out.as_deref(),
            jenkins: crate::jenkins::JenkinsOptions {
                previous,
                gpu_systems,
                only,
            },
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
