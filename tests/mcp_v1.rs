use eb_stack::mcp::handle_message;
use serde_json::json;
use std::sync::Mutex;

static WORKSPACE_ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn mcp_catalog_matches_the_version_one_workflows() {
    let response = handle_message(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
        "params": {}
    }))
    .expect("tools/list response");
    let names = response["result"]["tools"]
        .as_array()
        .expect("tool array")
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect::<Vec<_>>();
    for expected in [
        "eb_package_inspect",
        "eb_package_plan",
        "eb_package_bump",
        "eb_recipe_read",
        "eb_recipe_list",
        "eb_recipe_edit",
        "eb_recipe_apply_suggestion",
        "eb_recipe_diff",
        "eb_recipe_write",
        "eb_recipe_grep",
        "eb_recipe_easyblock",
        "eb_recipe_templates",
        "eb_recipe_hierarchy",
        "eb_dep_versions",
        "eb_recipe_check",
        "eb_recipe_lint",
        "eb_recipe_format",
        "eb_module_lint",
        "eb_easyblock_dry_run",
        "eb_build_log",
        "eb_build_test",
        "eb_stack_solve",
        "eb_stack_sbom",
        "eb_stack_order",
        "eb_stack_easystack",
        "eb_package_latest",
        "eb_stack_gap",
        "eb_stack_batch",
        "eb_stack_arch",
        "eb_stack_files",
        "eb_stack_sections",
        "eb_stack_from_commit",
        "eb_component_reach",
        "eb_target_list",
        "eb_target_doctor",
        "eb_campaign_run",
        "eb_campaign_status",
        "eb_campaign_finding_claim",
        "eb_campaign_finding_resolve",
        "eb_pr_overlap",
        "eb_rebuild_set",
        "eb_component_reach",
        "eb_easyblock_reach",
        "eb_repro_summary",
    ] {
        assert!(names.contains(&expected), "missing {expected}: {names:?}");
    }
    assert_eq!(names.len(), 43, "unexpected MCP tools: {names:?}");

    let inspect = response["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "eb_package_inspect")
        .expect("package inspect schema");
    let description = inspect["description"].as_str().expect("description");
    for format in [
        "conda-forge",
        "Spack",
        "PyPI",
        "CRAN",
        "Cargo",
        "LuaRocks",
        "Raku",
    ] {
        assert!(
            description.contains(format),
            "eb_package_inspect description omits {format}: {description}"
        );
    }

    let package_bump = response["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "eb_package_bump")
        .expect("package bump schema");
    for optional in [
        "version",
        "source_checksum",
        "dependencies",
        "hierarchy_fixture",
        "stack_policy",
        "package_configs",
        "emit_easyblock",
        "fetch_checksums",
        "strict_patches",
        "latest_secondary",
        "drop_patch",
        "toolchain",
    ] {
        assert!(
            package_bump["inputSchema"]["properties"]
                .get(optional)
                .is_some(),
            "bump schema missing optional {optional}"
        );
    }
    assert_eq!(
        package_bump["inputSchema"]["properties"]["dependencies"]["type"],
        "object"
    );

    let recipe_lint = response["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "eb_recipe_lint")
        .expect("recipe lint schema");
    assert_eq!(
        recipe_lint["inputSchema"]["properties"]["recipes"]["type"],
        "array"
    );
    assert!(recipe_lint["inputSchema"]["properties"]["recipes"]["minItems"].is_null());

    let recipe_check = response["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "eb_recipe_check")
        .expect("recipe check schema");
    assert_eq!(
        recipe_check["inputSchema"]["properties"]["easyconfigs"]["type"],
        "array"
    );
    assert_eq!(
        recipe_check["inputSchema"]["properties"]["easyconfigs"]["minItems"],
        1
    );
    for (tool_name, required) in [
        ("eb_recipe_read", vec!["path"]),
        ("eb_recipe_list", vec!["path"]),
        ("eb_recipe_edit", vec!["path", "old", "new"]),
        ("eb_recipe_write", vec!["path", "content"]),
        ("eb_recipe_grep", vec!["pattern"]),
        ("eb_dep_versions", vec!["name"]),
    ] {
        let tool = response["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == tool_name)
            .unwrap_or_else(|| panic!("missing {tool_name}"));
        let actual = tool["inputSchema"]["required"]
            .as_array()
            .expect("required")
            .iter()
            .filter_map(|value| value.as_str())
            .collect::<Vec<_>>();
        assert_eq!(actual, required, "{tool_name}");
    }
    let recipe_edit = response["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "eb_recipe_edit")
        .expect("recipe edit schema");
    assert_eq!(
        recipe_edit["inputSchema"]["properties"]["replace_all"]["type"],
        "boolean"
    );
    for optional in [
        "easyblocks",
        "metadata_only",
        "verify_sources",
        "seeded_from",
        "seeded_source_url",
        "seeded_git",
        "source_tree",
        "strict",
    ] {
        assert!(
            recipe_check["inputSchema"]["properties"]
                .get(optional)
                .is_some(),
            "check schema missing {optional}"
        );
    }
    let recipe_format = response["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "eb_recipe_format")
        .expect("recipe format schema");
    assert_eq!(
        recipe_format["inputSchema"]["properties"]["dry_run"]["type"],
        "boolean"
    );

    let stack_sbom = response["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "eb_stack_sbom")
        .expect("stack sbom schema");
    assert_eq!(
        stack_sbom["inputSchema"]["properties"]["lock"]["type"],
        "string"
    );
    for removed in [
        "eb_ingest",
        "eb_plan",
        "eb_bump",
        "eb_solve",
        "eb_check_recipe",
    ] {
        assert!(!names.contains(&removed), "legacy tool remains: {names:?}");
    }

    let package_plan = response["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "eb_package_plan")
        .expect("package plan schema");
    assert_eq!(
        package_plan["inputSchema"]["properties"]["source_checksums"]["type"],
        "array"
    );
    assert_eq!(
        package_plan["inputSchema"]["properties"]["package_configs"]["type"],
        "array"
    );
    assert_eq!(
        package_plan["inputSchema"]["properties"]["emit_easyblock"]["type"],
        "boolean"
    );
    assert_eq!(
        package_plan["inputSchema"]["properties"]["package_catalogs"]["type"],
        "array"
    );
    assert_eq!(
        package_plan["inputSchema"]["properties"]["package_sources"]["type"],
        "array"
    );
    assert_eq!(
        package_plan["inputSchema"]["properties"]["easybuild_sources"]["type"],
        "array"
    );
    assert!(
        package_plan["inputSchema"]["properties"]
            .get("profile_configs")
            .is_none(),
        "the package config owns metadata, build policy, and profiles"
    );
    assert!(
        !package_plan["inputSchema"]["required"]
            .as_array()
            .unwrap()
            .iter()
            .any(|name| name == "source_checksums"),
        "source checksums are optional when the foreign recipe supplies them"
    );
}

#[test]
fn mcp_target_doctor_uses_layered_public_config() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = temp.path().join("targets.toml");
    std::fs::write(
        &config,
        r#"
schema_version = 1
[[targets]]
name = "local-doctor"
[targets.transport]
kind = "local"
[targets.executor]
kind = "direct"
[targets.runtime]
kind = "host"
[targets.easybuild]
command = "true"
robot_paths = ["/tmp"]
work_root = "/tmp"
tmp_root = "/tmp"
"#,
    )
    .expect("config");
    let response = handle_message(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "eb_target_doctor",
            "arguments": {
                "configs": [config],
                "target": "local-doctor"
            }
        }
    }))
    .expect("doctor response");
    assert_eq!(response["result"]["isError"], false, "{response}");
    let body = response["result"]["structuredContent"].clone();
    assert_eq!(body["target"], "local-doctor");
    assert_eq!(body["ok"], true);
}

#[test]
fn mcp_package_plan_writes_the_same_catalog_backed_closure_as_the_cli() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("alpha.yaml");
    let companion = temp.path().join("bravo.yaml");
    let catalog = temp.path().join("catalog.toml");
    let stack = temp.path().join("stack.toml");
    let robot = temp.path().join("robot");
    let output = temp.path().join("output");
    std::fs::create_dir(&robot).expect("robot");
    std::fs::write(
        &root,
        r#"
package:
  name: alpha
  version: "1.0"
source:
  url: https://example.invalid/alpha-1.0.tar.gz
  sha256: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
requirements:
  host:
    - bravo >=1.0
"#,
    )
    .expect("root recipe");
    std::fs::write(
        &companion,
        r#"
package:
  name: bravo
  version: "1.5"
source:
  url: https://example.invalid/bravo-1.5.tar.gz
  sha256: bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
"#,
    )
    .expect("companion recipe");
    std::fs::write(
        &catalog,
        r#"
schema_version = 1

[[packages]]
name = "bravo"
version = "1.5"
source = "bravo.yaml"
format = "conda-forge"
toolchain = { name = "foss", version = "2026.1" }
"#,
    )
    .expect("catalog");
    std::fs::write(
        &stack,
        r#"
schema_version = 1
name = "site"
[toolchain]
name = "foss"
version = "2026.1"
"#,
    )
    .expect("stack");

    let response = handle_message(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "eb_package_plan",
            "arguments": {
                "source": root,
                "format": "conda-forge",
                "toolchain": "foss/2026.1",
                "easyconfigs": [robot],
                "stack_policy": stack,
                "package_catalogs": [catalog],
                "out_dir": output
            }
        }
    }))
    .expect("package plan response");
    assert_eq!(response["result"]["isError"], false, "{response}");
    let body = &response["result"]["structuredContent"];
    assert!(body["closure_plan"].as_str().is_some(), "{body}");
    assert!(body["closure_sbom"].as_str().is_some(), "{body}");
    assert!(body["build_order"].as_str().is_some(), "{body}");
    assert_eq!(body["companions"].as_array().map(Vec::len), Some(1));
    assert!(output.join("closure.plan.json").is_file());
    assert!(output.join("closure.sbom.cdx.json").is_file());
    assert!(output.join("build-order.json").is_file());
}

#[test]
fn mcp_package_plan_reuses_robot_roots_for_cross_generation_bumps() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("alpha.yaml");
    let stack = temp.path().join("stack.toml");
    let robot = temp.path().join("robot");
    let output = temp.path().join("output");
    std::fs::create_dir(&robot).expect("robot");
    std::fs::write(
        &root,
        r#"package:
  name: alpha
  version: "1.0"
source:
  url: https://example.invalid/alpha-1.0.tar.gz
  sha256: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
requirements:
  host:
    - bravo >=1.5
"#,
    )
    .expect("root recipe");
    std::fs::write(
        &stack,
        r#"schema_version = 1
name = "site"
[toolchain]
name = "foss"
version = "2026.1"
"#,
    )
    .expect("stack");
    std::fs::write(
        robot.join("bravo-1.5-foss-2023b.eb"),
        "name = 'bravo'\n\
         version = '1.5'\n\
         homepage = 'https://example.invalid/bravo'\n\
         description = 'synthetic'\n\
         toolchain = {'name': 'foss', 'version': '2023b'}\n\
         sources = ['bravo-1.5.tar.gz']\n\
         checksums = ['bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb']\n\
         moduleclass = 'lib'\n",
    )
    .expect("source easyconfig");

    let response = handle_message(&json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "tools/call",
        "params": {
            "name": "eb_package_plan",
            "arguments": {
                "source": root,
                "format": "conda-forge",
                "toolchain": "foss/2026.1",
                "easyconfigs": [robot],
                "stack_policy": stack,
                "out_dir": output
            }
        }
    }))
    .expect("package plan response");
    assert_eq!(response["result"]["isError"], false, "{response}");
    let body = &response["result"]["structuredContent"];
    assert_eq!(body["companions"].as_array().map(Vec::len), Some(1));
    assert!(output.join("closure.plan.json").is_file());
    assert!(output
        .join("easyconfigs/b/bravo/bravo-1.5-foss-2026.1.eb")
        .is_file());
}

#[test]
fn mcp_recipe_lint_reports_style_findings() {
    let temp = tempfile::tempdir().expect("tempdir");
    let recipe = temp.path().join("long.eb");
    let long = format!("name = 'tool'\n# {}\n", "x".repeat(200));
    std::fs::write(&recipe, long).expect("recipe");
    let response = handle_message(&json!({
        "jsonrpc": "2.0",
        "id": 5,
        "method": "tools/call",
        "params": {
            "name": "eb_recipe_lint",
            "arguments": {
                "recipes": [recipe]
            }
        }
    }))
    .expect("lint response");
    assert_eq!(response["result"]["isError"], false, "{response}");
    let body = &response["result"]["structuredContent"];
    let results = body["results"].as_array().expect("results");
    assert_eq!(results.len(), 1);
    assert!(
        results[0]["findings"]
            .as_array()
            .map(|findings| !findings.is_empty())
            .unwrap_or(false),
        "expected E501 findings: {body}"
    );
    assert!(body.get("check_contrib").is_some(), "{body}");
    assert_eq!(body["clean"], false, "E501 is an error: {body}");
    assert_eq!(results[0]["clean"], false, "{body}");
    assert!(
        body["summary"].as_str().unwrap_or("").contains("not clean"),
        "{body}"
    );

    // The review is about another line: this pre-existing E501 is info.
    let off = call(
        6,
        "eb_recipe_lint",
        json!({"recipes": [recipe], "review_lines": [1]}),
    );
    let body = &off["result"]["structuredContent"];
    assert_eq!(body["clean"], true, "{body}");
    assert_eq!(body["error_free"], false, "{body}");
    assert_eq!(body["preexisting_errors"], 1, "{body}");
    assert_eq!(body["results"][0]["findings"][0]["info"], true, "{body}");
    // The review covers the long line: it still blocks.
    let on = call(
        7,
        "eb_recipe_lint",
        json!({"recipes": [recipe], "review_lines": [2]}),
    );
    let body = &on["result"]["structuredContent"];
    assert_eq!(
        body["clean"], false,
        "E501 on a review line still blocks: {body}"
    );
}

fn call(id: u64, name: &str, arguments: serde_json::Value) -> serde_json::Value {
    handle_message(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": {"name": name, "arguments": arguments}
    }))
    .expect("tool response")
}

#[test]
fn mcp_recipe_edit_model_guard_reverts_arm3_field_clobbering() {
    let _guard = WORKSPACE_ENV_LOCK.lock().expect("workspace env lock");
    let previous = std::env::var_os("EB_STACK_WORKSPACE");
    std::env::set_var("EB_STACK_WORKSPACE", env!("CARGO_MANIFEST_DIR"));
    struct Restore(Option<std::ffi::OsString>);
    impl Drop for Restore {
        fn drop(&mut self) {
            match self.0.take() {
                Some(value) => std::env::set_var("EB_STACK_WORKSPACE", value),
                None => std::env::remove_var("EB_STACK_WORKSPACE"),
            }
        }
    }
    let _restore = Restore(previous);
    let temp = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).expect("tempdir");
    let cases = [
        (
            include_str!("repro_fixtures/model_guard/bracken_deterministic.eb"),
            include_str!("repro_fixtures/model_guard/bracken_arm3.eb"),
            "bracken",
        ),
        (
            include_str!("repro_fixtures/model_guard/multiprocess_deterministic.eb"),
            include_str!("repro_fixtures/model_guard/multiprocess_arm3.eb"),
            "multiprocess",
        ),
    ];
    for (index, (deterministic, arm3, label)) in cases.iter().enumerate() {
        let recipe = temp.path().join(format!("{label}.eb"));
        std::fs::write(&recipe, deterministic).expect("write deterministic fixture");
        let response = call(
            10_000 + index as u64,
            "eb_recipe_edit",
            json!({
                "path": recipe,
                "old": deterministic,
                "new": arm3,
                "model_guard": true
            }),
        );
        assert_eq!(response["result"]["isError"], true, "{response}");
        let message = response["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default();
        assert!(
            message.contains("post-model guard reverted this edit"),
            "{label}: {message}"
        );
        let after = std::fs::read_to_string(&recipe).expect("read reverted recipe");
        assert_eq!(after, *deterministic, "{label}: deterministic file changed");
    }
}

#[test]
fn mcp_recipe_lint_reports_syntax_errors_and_a_clean_flag() {
    let temp = tempfile::tempdir().expect("tempdir");
    let broken = temp.path().join("broken.eb");
    std::fs::write(
        &broken,
        "name = 'x'
sources = ['f'libexec/7z.{SHLIB_EXT}']
",
    )
    .expect("broken");
    let response = call(20, "eb_recipe_lint", json!({"recipes": [broken]}));
    assert_eq!(response["result"]["isError"], false, "{response}");
    let body = &response["result"]["structuredContent"];
    assert_eq!(body["clean"], false, "{body}");
    let findings = body["results"][0]["findings"].as_array().expect("findings");
    assert!(
        findings.iter().any(|finding| finding["code"] == "E999"),
        "a file that is not Python must not lint clean: {body}"
    );

    let good = temp.path().join("good.eb");
    std::fs::write(
        &good,
        "easyblock = 'ConfigureMake'
name = 'x'
sanity_check_paths = {
    'files': [f'lib/%(name)s.{SHLIB_EXT}'],
    'dirs': [],
}
",
    )
    .expect("good");
    // A scalar where an array is expected is accepted.
    let response = call(21, "eb_recipe_lint", json!({"recipes": good}));
    assert_eq!(response["result"]["isError"], false, "{response}");
    let body = &response["result"]["structuredContent"];
    assert_eq!(body["clean"], true, "{body}");
    assert_eq!(body["errors"], 0, "{body}");
    assert!(
        body["summary"]
            .as_str()
            .unwrap_or("")
            .contains("changing nothing is a valid result"),
        "{body}"
    );

    let concat = temp.path().join("concat.eb");
    std::fs::write(
        &concat,
        "name = 'x'
files = ['lib/%(name)s.' + SHLIB_EXT]
",
    )
    .expect("concat");
    let response = call(22, "eb_recipe_lint", json!({"recipes": [concat]}));
    let body = &response["result"]["structuredContent"];
    let findings = body["results"][0]["findings"].as_array().expect("findings");
    assert!(
        findings
            .iter()
            .any(|finding| finding["code"] == "EB_FMT_CONCAT" && finding["severity"] == "warning"),
        "{body}"
    );
}

#[test]
fn mcp_recipe_lint_is_not_clean_while_percent_formatting_remains() {
    let temp = tempfile::tempdir().expect("tempdir");
    let recipe = temp.path().join("p7zip.eb");
    std::fs::write(
        &recipe,
        "name = 'p7zip'\nlocal_makefile = 'makefile.linux_amd64'\nprebuildopts = f'cp {local_makefile} makefile.linux && '\nfiles = ['libexec/7z.%s' % SHLIB_EXT]\n",
    )
    .expect("recipe");
    let response = call(27, "eb_recipe_lint", json!({"recipes": [recipe]}));
    assert_eq!(response["result"]["isError"], false, "{response}");
    let body = &response["result"]["structuredContent"];
    assert_eq!(body["errors"], 0, "{body}");
    assert_eq!(body["error_free"], true, "{body}");
    assert_eq!(
        body["clean"], false,
        "a leftover '%s' % SHLIB_EXT is not done: {body}"
    );
    assert_eq!(body["formatting"], 1, "{body}");
    let summary = body["summary"].as_str().unwrap_or("");
    assert!(summary.starts_with("not clean"), "{summary}");
    assert!(summary.contains("EB_FMT"), "{summary}");
    assert!(
        !summary.contains("leave correct untouched lines alone"),
        "{summary}"
    );
    let message = body["results"][0]["findings"][0]["message"]
        .as_str()
        .unwrap_or("");
    assert!(
        message.contains("becomes `f'libexec/7z.{SHLIB_EXT}'`"),
        "{message}"
    );

    std::fs::write(
        &recipe,
        "name = 'p7zip'\nlocal_makefile = 'makefile.linux_amd64'\nprebuildopts = f'cp {local_makefile} makefile.linux && '\nfiles = [f'libexec/7z.{SHLIB_EXT}']\n",
    )
    .expect("fixed");
    let response = call(28, "eb_recipe_lint", json!({"recipes": [recipe]}));
    let body = &response["result"]["structuredContent"];
    assert_eq!(body["clean"], true, "{body}");
}

#[test]
fn mcp_dep_versions_lists_versions_from_a_tree_and_refuses_to_guess() {
    let temp = tempfile::tempdir().expect("tempdir");
    let dir = temp.path().join("easybuild/easyconfigs/c/Cython");
    std::fs::create_dir_all(&dir).expect("dir");
    std::fs::write(
        dir.join("Cython-0.29.37-GCCcore-13.3.0.eb"),
        "name = 'Cython'\nversion = '0.29.37'\ntoolchain = {'name': 'GCCcore', 'version': '13.3.0'}\n",
    )
    .expect("ec");
    std::fs::write(
        dir.join("Cython-3.0.10-GCCcore-13.2.0.eb"),
        "name = 'Cython'\nversion = '3.0.10'\ntoolchain = {'name': 'GCCcore', 'version': '13.2.0'}\n",
    )
    .expect("ec");
    let response = call(
        40,
        "eb_dep_versions",
        json!({"name": "Cython", "toolchain": "GCCcore-13.3.0", "easyconfigs": [temp.path().join("easybuild/easyconfigs")]}),
    );
    assert_eq!(response["result"]["isError"], false, "{response}");
    let body = &response["result"]["structuredContent"];
    assert_eq!(body["matching"][0]["version"], "0.29.37", "{body}");
    assert_eq!(
        body["other_toolchains"][0]["toolchain"], "GCCcore-13.2.0",
        "{body}"
    );

    let empty = tempfile::tempdir().expect("empty");
    let response = call(
        41,
        "eb_dep_versions",
        json!({"name": "Cython", "easyconfigs": [empty.path()]}),
    );
    assert_eq!(response["result"]["isError"], true, "{response}");
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("");
    assert!(text.contains("Do not guess"), "{text}");
}

#[test]
fn mcp_recipe_lint_compares_against_the_file_as_first_seen() {
    let temp = tempfile::tempdir().expect("tempdir");
    let recipe = temp.path().join("pkg.eb");
    let original = "easyblock = 'PythonPackage'\nname = 'pkg'\nsanity_check_paths = {\n    'files': ['bin/pkg', 'lib/libpkg.%s' % SHLIB_EXT],\n    'dirs': [],\n}\n";
    std::fs::write(&recipe, original).expect("recipe");
    let first = call(42, "eb_recipe_lint", json!({"recipes": [recipe]}));
    let body = &first["result"]["structuredContent"];
    assert_eq!(
        body["results"][0]["baseline"], "first seen in this session",
        "{body}"
    );

    // Drop the easyblock and collapse a sanity entry.
    std::fs::write(
        &recipe,
        "name = 'pkg'\nsanity_check_paths = {\n    'files': [f'lib/libpkg.{SHLIB_EXT}'],\n    'dirs': [],\n}\n",
    )
    .expect("edited");
    let second = call(43, "eb_recipe_lint", json!({"recipes": [recipe]}));
    let body = &second["result"]["structuredContent"];
    assert_eq!(body["clean"], false, "{body}");
    let codes: Vec<&str> = body["results"][0]["findings"]
        .as_array()
        .expect("findings")
        .iter()
        .filter_map(|finding| finding["code"].as_str())
        .collect();
    assert!(codes.contains(&"EB_EASYBLOCK_CHANGED"), "{codes:?}");
    assert!(codes.contains(&"EB_SANITY_ENTRY_REMOVED"), "{codes:?}");

    // rebaseline is refused while a lost sanity entry or a changed easyblock
    // is outstanding off the review lines: resetting would only hide it.
    let third = call(
        44,
        "eb_recipe_lint",
        json!({"recipes": [recipe], "rebaseline": true}),
    );
    let body = &third["result"]["structuredContent"];
    assert_eq!(body["clean"], false, "{body}");
    let summary = body["summary"].as_str().unwrap_or("");
    assert!(summary.contains("rebaseline refused"), "{summary}");
    assert!(summary.contains("EB_SANITY_ENTRY_REMOVED"), "{summary}");
    assert!(summary.contains("EB_EASYBLOCK_CHANGED"), "{summary}");

    // The review asked for those lines, so rebaseline accepts the file.
    let fourth = call(
        45,
        "eb_recipe_lint",
        json!({"recipes": [recipe], "rebaseline": true, "review_lines": [1, 2]}),
    );
    let body = &fourth["result"]["structuredContent"];
    assert_eq!(body["clean"], true, "{body}");
}

#[test]
fn mcp_recipe_lint_and_diff_report_open_review_items() {
    let temp = tempfile::tempdir().expect("tempdir");
    let recipe = temp.path().join("items.eb");
    let pre = "easyblock = 'CMakeMake'\nname = 'items'\nconfigopts = '-DBUILD_SHARED_LIBS=ON'\nsanity_check_paths = {\n    'files': ['bin/items'],\n    'dirs': [],\n}\nruntest = 'check'\nmoduleclass = 'tools'\n";
    std::fs::write(&recipe, pre).expect("recipe");
    let items = json!([
        {"id": "pic", "line_start": 3, "suggestion": null,
         "comment": "should this also pass `-DWITH_PIC=ON`?"},
        {"id": "lib", "line_start": 5, "suggestion": "    'files': ['bin/items', 'lib/libitems.a'],"},
        {"id": "runtest", "line_start": 8, "suggestion": ""}
    ]);
    let first = call(
        70,
        "eb_recipe_lint",
        json!({"recipes": [recipe], "review_lines": [3, 5, 8], "review_items": items}),
    );
    let body = &first["result"]["structuredContent"];
    assert_eq!(body["clean"], false, "{body}");
    assert_eq!(body["open_review_items"], 3, "{body}");
    let statuses = body["results"][0]["review_items"]
        .as_array()
        .expect("items");
    assert_eq!(statuses[0]["status"], "no_hunk", "{body}");
    assert_eq!(statuses[0]["mentions_absent"][0], "-DWITH_PIC=ON", "{body}");
    assert_eq!(statuses[1]["status"], "suggestion_missing", "{body}");
    assert_eq!(statuses[2]["status"], "not_deleted", "{body}");

    // The two suggestions are applied; the question is still open.
    std::fs::write(
        &recipe,
        pre.replace("['bin/items']", "['bin/items', 'lib/libitems.a']")
            .replace("runtest = 'check'\n", ""),
    )
    .expect("edited");
    let second = call(
        71,
        "eb_recipe_lint",
        json!({"recipes": [recipe], "review_lines": [3, 5, 8], "review_items": items}),
    );
    let body = &second["result"]["structuredContent"];
    assert_eq!(body["open_review_items"], 1, "{body}");
    assert_eq!(body["clean"], false, "{body}");
    let codes: Vec<&str> = body["results"][0]["findings"]
        .as_array()
        .expect("findings")
        .iter()
        .filter_map(|finding| finding["code"].as_str())
        .collect();
    assert!(codes.contains(&"EB_REVIEW_ITEM_OPEN"), "{body}");
    assert!(
        body["summary"]
            .as_str()
            .unwrap_or_default()
            .contains("pic (no_hunk)"),
        "{body}"
    );

    let diff = call(
        72,
        "eb_recipe_diff",
        json!({"recipes": [recipe], "review_items": items}),
    );
    let body = &diff["result"]["structuredContent"];
    assert_eq!(body["open_review_items"], 1, "{body}");

    // The question needed no change: acknowledged, the review is answered.
    let third = call(
        73,
        "eb_recipe_lint",
        json!({"recipes": [recipe], "review_lines": [3, 5, 8], "review_items": items,
               "no_change_needed": ["pic"]}),
    );
    let body = &third["result"]["structuredContent"];
    assert_eq!(body["open_review_items"], 0, "{body}");
    assert_eq!(body["clean"], true, "{body}");
}

#[test]
fn mcp_recipe_lint_file_scope_keeps_preexisting_errors_advisory() {
    let temp = tempfile::tempdir().expect("tempdir");
    let recipe = temp.path().join("tool.eb");
    let pre = "easyblock = 'MesonNinja'\nname = 'tool'\nversion_tag = 'v1'\nbuilddependencies = [\n    ('Meson', '1.8.2'),\n]\nsanity_check_commands = ['python -c \"import tool\"']\n";
    std::fs::write(&recipe, pre).expect("recipe");
    let first = call(60, "eb_recipe_lint", json!({"recipes": [recipe]}));
    let body = &first["result"]["structuredContent"];
    let codes: Vec<&str> = body["results"][0]["findings"]
        .as_array()
        .expect("findings")
        .iter()
        .filter_map(|finding| finding["code"].as_str())
        .collect();
    assert!(codes.contains(&"EB_PY_NO_USER_SITE"), "{body}");
    assert!(codes.contains(&"EB_LOCAL_VAR_NAMING"), "{body}");

    // The review asks for one more build dependency (line 5).
    std::fs::write(
        &recipe,
        pre.replace(
            "    ('Meson', '1.8.2'),\n",
            "    ('Meson', '1.8.2'),\n    ('flex', '2.6.4'),\n",
        ),
    )
    .expect("edited");
    for scope in [json!(null), json!("file")] {
        let mut arguments = json!({"recipes": [recipe], "review_lines": [5, 6]});
        if !scope.is_null() {
            arguments["review_scope"] = scope.clone();
        }
        let response = call(61, "eb_recipe_lint", arguments);
        let body = &response["result"]["structuredContent"];
        assert_eq!(body["clean"], true, "scope {scope}: {body}");
        assert!(
            body["preexisting_errors"].as_u64().unwrap_or(0) >= 2,
            "scope {scope}: {body}"
        );
        for code in ["EB_PY_NO_USER_SITE", "EB_LOCAL_VAR_NAMING"] {
            let finding = body["results"][0]["findings"]
                .as_array()
                .expect("findings")
                .iter()
                .find(|finding| finding["code"] == code)
                .unwrap_or_else(|| panic!("{code} missing: {body}"));
            assert_eq!(finding["info"], true, "scope {scope}: {body}");
        }
        assert_eq!(body["error_free"], false, "scope {scope}: {body}");
    }

    // Under review_scope "file" a pre-existing EB_FMT finding still counts:
    // that is what the file scope is for.
    let fmt = temp.path().join("fmt.eb");
    let fmt_pre = "name = 'tool'\nfiles = ['lib/libtool.%s' % SHLIB_EXT]\nx = 1\n";
    std::fs::write(&fmt, fmt_pre).expect("fmt");
    let _ = call(62, "eb_recipe_lint", json!({"recipes": [fmt]}));
    std::fs::write(&fmt, fmt_pre.replace("x = 1", "x = 2")).expect("fmt edit");
    let response = call(
        63,
        "eb_recipe_lint",
        json!({"recipes": [fmt], "review_lines": [3], "review_scope": "file"}),
    );
    let body = &response["result"]["structuredContent"];
    assert_eq!(body["clean"], false, "{body}");
}

#[test]
fn mcp_recipe_lint_review_lines_and_base_decide_formatting_scope() {
    let temp = tempfile::tempdir().expect("tempdir");
    let base = temp.path().join("base.eb");
    let recipe = temp.path().join("tool.eb");
    let pre = "name = 'tool'\nlocal_m = 'm'\nprebuildopts = 'cp %s x &&' % local_m\nfiles = ['libexec/t.%s' % SHLIB_EXT]\n";
    std::fs::write(&base, pre).expect("base");
    std::fs::write(
        &recipe,
        "name = 'tool'\nlocal_m = 'm'\nprebuildopts = f'cp {local_m} x &&'\nfiles = ['libexec/t.%s' % SHLIB_EXT]\n",
    )
    .expect("recipe");
    // Explicit base: the untouched line 4 is info, so the file is clean.
    let scoped = call(
        45,
        "eb_recipe_lint",
        json!({"recipes": [recipe], "base": base}),
    );
    let body = &scoped["result"]["structuredContent"];
    assert_eq!(body["clean"], true, "{body}");
    assert_eq!(body["info"], 1, "{body}");
    assert_eq!(body["results"][0]["findings"][0]["info"], true, "{body}");
    // The review is about line 4 too: not clean until it is converted.
    let reviewed = call(
        46,
        "eb_recipe_lint",
        json!({"recipes": [recipe], "base": base, "review_lines": [3, 4]}),
    );
    let body = &reviewed["result"]["structuredContent"];
    assert_eq!(body["clean"], false, "{body}");
    // No base given: every unescaped '%' line counts, as for #27186.
    let other = temp.path().join("fresh.eb");
    std::fs::write(&other, std::fs::read_to_string(&recipe).unwrap()).expect("fresh");
    let unscoped = call(47, "eb_recipe_lint", json!({"recipes": [other]}));
    let body = &unscoped["result"]["structuredContent"];
    assert_eq!(body["clean"], false, "{body}");
}

#[test]
fn mcp_recipe_lint_base_reports_removed_templates() {
    let temp = tempfile::tempdir().expect("tempdir");
    let base = temp.path().join("base.eb");
    let edited = temp.path().join("edited.eb");
    std::fs::write(
        &base,
        "name = 'x'
files = ['include/%(name)s/%(name)s.h']
",
    )
    .expect("base");
    std::fs::write(
        &edited,
        "name = 'x'
files = [f'include/{name}/{name}.h']
",
    )
    .expect("edited");
    let response = call(
        23,
        "eb_recipe_lint",
        json!({"recipes": [edited], "base": base}),
    );
    let body = &response["result"]["structuredContent"];
    let findings = body["results"][0]["findings"].as_array().expect("findings");
    assert!(
        findings
            .iter()
            .any(|finding| finding["code"] == "EB_FMT_TEMPLATE_REMOVED" && finding["line"] == 2),
        "{body}"
    );
}

#[test]
fn mcp_recipe_check_refuses_unparseable_recipes_and_accepts_an_eb_robot_path() {
    let temp = tempfile::tempdir().expect("tempdir");
    let robot = temp.path().join("easybuild/easyconfigs/d/Demo");
    std::fs::create_dir_all(&robot).expect("robot");
    let recipe = robot.join("Demo-1.0.eb");
    std::fs::write(
        &recipe,
        "easyblock = 'ConfigureMake'
name = 'Demo'
version = '1.0'
homepage = 'https://example.invalid'
description = 'demo'
toolchain = SYSTEM
moduleclass = 'tools'
",
    )
    .expect("recipe");
    let response = call(
        24,
        "eb_recipe_check",
        json!({"recipe": recipe, "easyconfigs": [recipe]}),
    );
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("")
        .to_string();
    assert!(!text.contains("Not a directory"), "{response}");
    assert_eq!(response["result"]["isError"], false, "{response}");

    let broken = robot.join("Broken-1.0.eb");
    std::fs::write(
        &broken,
        "name = 'Broken'
version = '1.0'
sources = ['gd-f'{version}'/']
",
    )
    .expect("broken");
    let response = call(
        25,
        "eb_recipe_check",
        json!({"recipe": broken, "easyconfigs": [temp.path().join("easybuild/easyconfigs")]}),
    );
    assert_eq!(response["result"]["isError"], true, "{response}");
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("");
    assert!(text.contains("SyntaxError"), "{text}");

    let missing = call(
        26,
        "eb_recipe_check",
        json!({"recipe": recipe, "easyconfigs": [temp.path().join("notes.txt")]}),
    );
    assert_eq!(missing["result"]["isError"], true, "{missing}");
}

#[test]
fn mcp_recipe_check_rejects_an_empty_easyconfigs_list() {
    let temp = tempfile::tempdir().expect("tempdir");
    let recipe = temp.path().join("demo.eb");
    std::fs::write(
        &recipe,
        "name = 'Demo'\nversion = '1.0'\ntoolchain = {'name': 'foss', 'version': '2025b'}\n",
    )
    .expect("recipe");
    let response = handle_message(&json!({
        "jsonrpc": "2.0",
        "id": 6,
        "method": "tools/call",
        "params": {
            "name": "eb_recipe_check",
            "arguments": {
                "recipe": recipe,
                "easyconfigs": []
            }
        }
    }))
    .expect("check response");
    assert_eq!(response["result"]["isError"], true, "{response}");
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("");
    assert!(
        text.contains("argument easyconfigs requires at least one path"),
        "{text}"
    );
}

#[cfg(unix)]
#[test]
fn mcp_recipe_file_tools_stay_inside_the_workspace() {
    let _guard = WORKSPACE_ENV_LOCK.lock().expect("workspace env lock");
    let previous = std::env::var_os("EB_STACK_WORKSPACE");
    let temp = tempfile::tempdir().expect("tempdir");
    let outside = tempfile::tempdir().expect("outside");
    std::env::set_var("EB_STACK_WORKSPACE", temp.path());
    struct Restore(Option<std::ffi::OsString>);
    impl Drop for Restore {
        fn drop(&mut self) {
            match self.0.take() {
                Some(value) => std::env::set_var("EB_STACK_WORKSPACE", value),
                None => std::env::remove_var("EB_STACK_WORKSPACE"),
            }
        }
    }
    let _restore = Restore(previous);

    let secret = outside.path().join("secret.txt");
    std::fs::write(&secret, "original-secret").expect("secret");
    std::os::unix::fs::symlink(&secret, temp.path().join("leak")).expect("symlink");

    let wrote = handle_message(&json!({
        "jsonrpc": "2.0",
        "id": 7,
        "method": "tools/call",
        "params": {
            "name": "eb_recipe_write",
            "arguments": {"path": "demo.eb", "content": "name = 'demo'\nfiles = ['%s' % SHLIB_EXT, '%s' % SHLIB_EXT]\n"}
        }
    }))
    .expect("write");
    assert_eq!(wrote["result"]["isError"], false, "{wrote}");
    assert_eq!(wrote["result"]["structuredContent"]["created"], true);

    let ambiguous = handle_message(&json!({
        "jsonrpc": "2.0",
        "id": 8,
        "method": "tools/call",
        "params": {
            "name": "eb_recipe_edit",
            "arguments": {
                "path": "demo.eb",
                "old": "'%s' % SHLIB_EXT",
                "new": "f'{SHLIB_EXT}'",
                "replace_all": false
            }
        }
    }))
    .expect("edit");
    assert_eq!(ambiguous["result"]["isError"], true, "{ambiguous}");
    let edit_error = ambiguous["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("");
    assert!(edit_error.contains("replace_all"), "{edit_error}");

    let edited = handle_message(&json!({
        "jsonrpc": "2.0",
        "id": 9,
        "method": "tools/call",
        "params": {
            "name": "eb_recipe_edit",
            "arguments": {
                "path": "demo.eb",
                "old": "'%s' % SHLIB_EXT",
                "new": "f'{SHLIB_EXT}'",
                "replace_all": true
            }
        }
    }))
    .expect("replace all");
    assert_eq!(edited["result"]["isError"], false, "{edited}");
    assert_eq!(edited["result"]["structuredContent"]["replaced"], 2);

    let read = handle_message(&json!({
        "jsonrpc": "2.0",
        "id": 10,
        "method": "tools/call",
        "params": {
            "name": "eb_recipe_read",
            "arguments": {"path": "demo.eb"}
        }
    }))
    .expect("read");
    let content = read["result"]["structuredContent"]["content"]
        .as_str()
        .unwrap_or("");
    assert_eq!(content.matches("f'{SHLIB_EXT}'").count(), 2);

    let grep = handle_message(&json!({
        "jsonrpc": "2.0",
        "id": 30,
        "method": "tools/call",
        "params": {
            "name": "eb_recipe_grep",
            "arguments": {"pattern": "SHLIB_EXT"}
        }
    }))
    .expect("grep");
    assert_eq!(grep["result"]["isError"], false, "{grep}");
    let matches = grep["result"]["structuredContent"]["matches"]
        .as_array()
        .expect("matches");
    assert_eq!(matches.len(), 1, "{grep}");
    assert_eq!(matches[0]["path"], "demo.eb");
    assert_eq!(matches[0]["line"], 2);
    let grep_text = grep.to_string();
    assert!(!grep_text.contains("original-secret"), "{grep_text}");

    // eb_recipe_format writes nothing but the requested .eb path.
    let recipe = temp.path().join("demo.eb");
    let before_entries = {
        let mut names: Vec<_> = std::fs::read_dir(temp.path())
            .expect("read dir")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        names.sort();
        names
    };
    for (id, out) in [
        (31, "formatted_tcl.txt"),
        (32, "out"),
        (33, "missing/dir/copy.eb"),
    ] {
        let refused = call(
            id,
            "eb_recipe_format",
            json!({"recipe": recipe, "out": out}),
        );
        assert_eq!(refused["result"]["isError"], true, "{out}: {refused}");
    }
    let after_refusals = {
        let mut names: Vec<_> = std::fs::read_dir(temp.path())
            .expect("read dir")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        names.sort();
        names
    };
    assert_eq!(
        before_entries, after_refusals,
        "no stray files or directories"
    );
    let dry = call(
        34,
        "eb_recipe_format",
        json!({"recipe": recipe, "dry_run": true}),
    );
    assert_eq!(dry["result"]["isError"], false, "{dry}");
    assert!(
        dry["result"]["structuredContent"]["written"].is_null(),
        "{dry}"
    );
    assert!(
        dry["result"]["structuredContent"]["text"]
            .as_str()
            .unwrap_or("")
            .contains("name = 'demo'"),
        "{dry}"
    );
    let copy = call(
        35,
        "eb_recipe_format",
        json!({"recipe": recipe, "out": "copy.eb"}),
    );
    assert_eq!(copy["result"]["isError"], false, "{copy}");
    assert_eq!(copy["result"]["structuredContent"]["written"], "copy.eb");
    assert!(temp.path().join("copy.eb").is_file());
    std::fs::remove_file(temp.path().join("copy.eb")).expect("remove copy");
    let escape = call(
        36,
        "eb_recipe_format",
        json!({"recipe": recipe, "out": "../escape.eb"}),
    );
    assert_eq!(escape["result"]["isError"], true, "{escape}");

    let escaped = handle_message(&json!({
        "jsonrpc": "2.0",
        "id": 11,
        "method": "tools/call",
        "params": {
            "name": "eb_recipe_read",
            "arguments": {"path": "leak"}
        }
    }))
    .expect("leak");
    assert_eq!(escaped["result"]["isError"], true, "{escaped}");
    let leak_text = escaped["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("");
    assert!(leak_text.contains("symlink escapes"), "{leak_text}");
    assert!(!leak_text.contains("original-secret"), "{leak_text}");
    assert_eq!(
        std::fs::read_to_string(&secret).expect("secret"),
        "original-secret"
    );

    let listing = handle_message(&json!({
        "jsonrpc": "2.0",
        "id": 12,
        "method": "tools/call",
        "params": {
            "name": "eb_recipe_list",
            "arguments": {"path": "."}
        }
    }))
    .expect("list");
    assert_eq!(listing["result"]["isError"], false, "{listing}");
    let entries = listing["result"]["structuredContent"]["entries"]
        .as_array()
        .expect("entries");
    let leak = entries
        .iter()
        .find(|entry| entry["name"] == "leak")
        .expect("leak entry");
    assert_eq!(leak["escapes"], true);

    let parent = handle_message(&json!({
        "jsonrpc": "2.0",
        "id": 13,
        "method": "tools/call",
        "params": {
            "name": "eb_recipe_read",
            "arguments": {"path": "../outside.eb"}
        }
    }))
    .expect("parent");
    assert_eq!(parent["result"]["isError"], true, "{parent}");
    let parent_text = parent["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("");
    assert!(parent_text.contains("escapes"), "{parent_text}");
}

#[test]
fn mcp_stack_sbom_writes_cyclonedx_from_lock() {
    let temp = tempfile::tempdir().expect("tempdir");
    let lock_path = temp.path().join("stack.lock.json");
    let out = temp.path().join("stack.cdx.json");
    std::fs::write(
        &lock_path,
        r#"{
  "schema_version": 1,
  "toolchain": {"name": "foss", "version": "2026.1"},
  "packages": [
    {
      "name": "zlib",
      "version": "1.2",
      "toolchain": {"name": "foss", "version": "2026.1"},
      "versionsuffix": null,
      "easyconfig_path": "zlib-1.2-foss-2026.1.eb"
    }
  ],
  "solver": {
    "engine": "resolvo",
    "engine_version": "0",
    "timestamp": "2026-01-01T00:00:00Z"
  }
}"#,
    )
    .expect("lock");
    let response = handle_message(&json!({
        "jsonrpc": "2.0",
        "id": 6,
        "method": "tools/call",
        "params": {
            "name": "eb_stack_sbom",
            "arguments": {
                "lock": lock_path,
                "out": out
            }
        }
    }))
    .expect("sbom response");
    assert_eq!(response["result"]["isError"], false, "{response}");
    let body = &response["result"]["structuredContent"];
    assert!(out.is_file(), "{body}");
    assert!(body["components"].as_u64().unwrap_or(0) >= 1, "{body}");
    let sbom: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&out).expect("read sbom")).expect("json");
    assert_eq!(sbom["bomFormat"], "CycloneDX");
}

#[test]
fn mcp_stack_sbom_reads_dependency_edges_from_the_lock_recipes() {
    let temp = tempfile::tempdir().expect("tempdir");
    let zlib = temp.path().join("zlib.eb");
    let cmake = temp.path().join("cmake.eb");
    let app = temp.path().join("app.eb");
    std::fs::write(
        &zlib,
        "name = 'zlib'\nversion = '1.3.1'\n\
         toolchain = {'name': 'foss', 'version': '2026.1'}\n\
         sources = ['zlib-1.3.1.tar.gz']\n\
         checksums = ['aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa']\n",
    )
    .expect("zlib");
    std::fs::write(
        &cmake,
        "name = 'CMake'\nversion = '3.29.3'\n\
         toolchain = {'name': 'foss', 'version': '2026.1'}\n",
    )
    .expect("cmake");
    std::fs::write(
        &app,
        "name = 'App'\nversion = '1.0'\n\
         toolchain = {'name': 'foss', 'version': '2026.1'}\n\
         dependencies = [('zlib', '1.3.1')]\n\
         builddependencies = [('CMake', '3.29.3')]\n",
    )
    .expect("app");
    let lock_path = temp.path().join("stack.lock.json");
    let out = temp.path().join("stack.cdx.json");
    let package = |name: &str, version: &str, path: &std::path::Path| {
        json!({
            "name": name,
            "version": version,
            "toolchain": {"name": "foss", "version": "2026.1"},
            "versionsuffix": null,
            "easyconfig_path": path,
        })
    };
    std::fs::write(
        &lock_path,
        serde_json::to_string(&json!({
            "schema_version": 1,
            "toolchain": {"name": "foss", "version": "2026.1"},
            "packages": [
                package("zlib", "1.3.1", &zlib),
                package("CMake", "3.29.3", &cmake),
                package("App", "1.0", &app),
            ],
            "solver": {
                "engine": "resolvo",
                "engine_version": "0",
                "timestamp": "2026-01-01T00:00:00Z"
            }
        }))
        .expect("lock json"),
    )
    .expect("lock");
    let response = handle_message(&json!({
        "jsonrpc": "2.0",
        "id": 7,
        "method": "tools/call",
        "params": {
            "name": "eb_stack_sbom",
            "arguments": {"lock": lock_path, "out": out}
        }
    }))
    .expect("sbom response");
    assert_eq!(response["result"]["isError"], false, "{response}");
    let structured = &response["result"]["structuredContent"];
    assert_eq!(structured["components"], 3);
    assert_eq!(structured["recipes_read"], 3);
    assert_eq!(structured["recipes_unreadable"], 0);
    assert_eq!(structured["with_artifact_facts"], 1);
    let sbom: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&out).expect("read sbom")).expect("json");
    let app_ref = sbom["dependencies"]
        .as_array()
        .expect("dependencies")
        .iter()
        .find(|entry| entry["ref"].as_str().unwrap_or("").contains("App@1.0"))
        .expect("App dependency entry");
    let depends = app_ref["dependsOn"].as_array().expect("dependsOn");
    assert!(
        depends
            .iter()
            .any(|edge| edge.as_str().unwrap_or("").contains("zlib@1.3.1")),
        "{depends:?}"
    );
    assert!(
        depends
            .iter()
            .all(|edge| !edge.as_str().unwrap_or("").contains("CMake")),
        "a builddependency is not a runtime edge: {depends:?}"
    );
    let app_component = sbom["components"]
        .as_array()
        .expect("components")
        .iter()
        .find(|component| component["name"] == "App")
        .expect("App component");
    let build = app_component["properties"]
        .as_array()
        .expect("properties")
        .iter()
        .find(|property| property["name"] == "eb_stack:buildDependsOn")
        .expect("build edge");
    assert!(
        build["value"]
            .as_str()
            .unwrap_or("")
            .contains("CMake@3.29.3"),
        "{build}"
    );
    let zlib_component = sbom["components"]
        .as_array()
        .unwrap()
        .iter()
        .find(|component| component["name"] == "zlib")
        .expect("zlib component");
    assert_eq!(
        zlib_component["hashes"][0]["content"],
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    );
}

#[test]
fn mcp_schemas_match_the_handlers_for_bump_lint_and_resolve() {
    let response = handle_message(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
    }))
    .expect("tools/list");
    let tools = response["result"]["tools"].as_array().expect("tools");
    let tool = |name: &str| {
        tools
            .iter()
            .find(|tool| tool["name"] == name)
            .unwrap_or_else(|| panic!("missing {name}"))
    };
    let lint = tool("eb_recipe_lint");
    assert!(lint["inputSchema"]["properties"].get("recipe").is_some());
    assert!(lint["inputSchema"]["required"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(
        lint["inputSchema"]["anyOf"].as_array().map(Vec::len),
        Some(2)
    );
    let resolve = tool("eb_campaign_finding_resolve");
    assert!(resolve["inputSchema"]["properties"]
        .get("changes")
        .is_some());
    assert!(
        !resolve["inputSchema"]["required"]
            .as_array()
            .unwrap()
            .iter()
            .any(|name| name == "changes"),
        "changes is optional, as --change is"
    );

    let bump = handle_message(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "eb_package_bump",
            "arguments": {"source": "x.eb", "easyconfigs": ["."], "out_dir": "/tmp"}
        }
    }))
    .expect("bump");
    assert_eq!(bump["result"]["isError"], true, "{bump}");
    assert!(
        bump["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .contains("toolchain is required as name/version"),
        "{bump}"
    );
    assert_eq!(
        tool("eb_package_bump")["inputSchema"]["anyOf"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );

    let temp = tempfile::tempdir().expect("tempdir");
    let recipe = temp.path().join("short.eb");
    std::fs::write(&recipe, "name = 'tool'\n").expect("recipe");
    let linted = handle_message(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {"name": "eb_recipe_lint", "arguments": {"recipe": recipe}}
    }))
    .expect("lint");
    assert_eq!(linted["result"]["isError"], false, "{linted}");
}

#[test]
fn mcp_bump_strict_patches_rejects_an_undecided_patch() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("Gamma-1.0-GCCcore-14.3.0.eb");
    let robot = temp.path().join("robot");
    std::fs::create_dir_all(&robot).expect("robot");
    std::fs::write(
        &source,
        "easyblock = 'ConfigureMake'\nname = 'Gamma'\nversion = '1.0'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Synthetic package'\n\
         toolchain = {'name': 'GCCcore', 'version': '14.3.0'}\n\
         sources = ['gamma-1.0.tar.gz']\n\
         checksums = ['cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc']\n\
         patches = ['Gamma-1.0_fix.patch']\nmoduleclass = 'tools'\n",
    )
    .expect("source");
    std::fs::write(
        robot.join("Unrelated-9.9-GCCcore-15.2.0.eb"),
        "easyblock = 'ConfigureMake'\nname = 'Unrelated'\nversion = '9.9'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Filler'\n\
         toolchain = {'name': 'GCCcore', 'version': '15.2.0'}\n\
         sources = []\nchecksums = []\nmoduleclass = 'tools'\n",
    )
    .expect("filler");
    let response = call(
        71,
        "eb_package_bump",
        json!({
            "source": source,
            "toolchain": "GCCcore/15.2.0",
            "version": "2.0",
            "easyconfigs": [robot],
            "strict_patches": true,
            "out_dir": temp.path().join("out"),
        }),
    );
    assert_eq!(response["result"]["isError"], true, "{response}");
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("");
    assert!(
        text.contains("Gamma-1.0_fix.patch"),
        "strict_patches must fail the bump: {text}"
    );
}

#[test]
fn mcp_plan_reads_versions_from_a_package_index() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let temp = tempfile::tempdir().expect("tempdir");
    let index = temp.path().join("PACKAGES");
    std::fs::write(
        &index,
        "Package: processx\nVersion: 3.8.4\nMD5sum: 1111111111111111111111111111abcd\n\n\
         Package: rmarkdown\nVersion: 2.27\nMD5sum: 2222222222222222222222222222abcd\n",
    )
    .expect("index");
    let stack = temp.path().join("stack.toml");
    std::fs::write(
        &stack,
        "schema_version = 1\nname = \"test\"\n[toolchain]\nname = \"foss\"\nversion = \"2026.1\"\n",
    )
    .expect("stack");
    let output = temp.path().join("out");
    let response = call(
        72,
        "eb_package_plan",
        json!({
            "source": root.join("fixtures/foreign_ingest/cran_unpinned/cran.json"),
            "format": "cran",
            "toolchain": "foss/2026.1",
            "source_checksums": ["dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"],
            "easyconfigs": [root.join("fixtures/foreign_ingest/cran_bundle/robot")],
            "stack_policy": stack,
            "package_index": index,
            "out_dir": output,
        }),
    );
    assert_eq!(response["result"]["isError"], false, "{response}");
    let easyconfigs = response["result"]["structuredContent"]["easyconfigs"]
        .as_array()
        .expect("easyconfigs");
    assert!(!easyconfigs.is_empty(), "{response}");
    let recipe = std::fs::read_to_string(easyconfigs[0].as_str().expect("path")).expect("recipe");
    assert!(
        recipe.contains("('processx', '3.8.4'"),
        "the index version must reach the recipe:\n{recipe}"
    );
}

#[test]
fn mcp_toolchain_and_stack_policy_match_the_cli_names() {
    let listed = handle_message(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
        "params": {}
    }))
    .expect("tools/list");
    let tools = listed["result"]["tools"].as_array().expect("tools");
    let properties = |name: &str| {
        tools
            .iter()
            .find(|tool| tool["name"] == name)
            .unwrap_or_else(|| panic!("missing {name}"))["inputSchema"]["properties"]
            .clone()
    };
    assert!(properties("eb_package_inspect").get("toolchain").is_some());
    assert!(properties("eb_package_plan").get("toolchain").is_some());
    assert!(properties("eb_package_bump").get("toolchain").is_some());
    let solve = properties("eb_stack_solve");
    assert!(solve.get("stack_policy").is_some());
    assert!(solve.get("policy").is_none(), "{solve}");

    let bad = handle_message(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "eb_package_inspect",
            "arguments": {"source": "recipe.yaml", "out_dir": "bundle", "toolchain": "foss"}
        }
    }))
    .expect("inspect");
    assert_eq!(bad["result"]["isError"], true, "{bad}");
    let text = bad["result"]["content"][0]["text"].as_str().unwrap_or("");
    assert!(text.contains("toolchain foss:"), "{text}");

    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("recipe.yaml");
    std::fs::write(
        &source,
        "package:\n  name: eon\n  version: 1.0.0\nsource:\n  url: https://example.invalid/eon.tar.gz\n  sha256: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n",
    )
    .expect("recipe");
    for spec in ["system", "SYSTEM", "system/"] {
        let out_dir = temp.path().join(spec.replace('/', "slash"));
        let response = handle_message(&json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": "eb_package_inspect",
                "arguments": {
                    "source": source,
                    "format": "conda-forge",
                    "toolchain": spec,
                    "out_dir": out_dir,
                }
            }
        }))
        .expect("inspect system");
        assert_eq!(response["result"]["isError"], false, "{spec} {response}");
        let manifest = out_dir.join("package.plan.json");
        let plan = std::fs::read_to_string(&manifest).expect("plan");
        assert!(
            plan.contains("\"name\": \"system\"") && plan.contains("\"version\": \"\""),
            "{spec}: {plan}"
        );
    }

    let lone = temp.path().join("lone-version");
    let response = handle_message(&json!({
        "jsonrpc": "2.0",
        "id": 5,
        "method": "tools/call",
        "params": {
            "name": "eb_package_inspect",
            "arguments": {
                "source": source,
                "format": "conda-forge",
                "toolchain_version": "2026.1",
                "out_dir": lone,
            }
        }
    }))
    .expect("inspect lone version");
    assert_eq!(response["result"]["isError"], false, "{response}");
    let plan: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(lone.join("package.plan.json")).expect("plan"),
    )
    .expect("plan json");
    assert_eq!(
        plan["build"]["toolchain"],
        json!({"name": "foss", "version": "2026.1"}),
        "0.3.0 filled a missing toolchain_name with foss"
    );

    let missing = "/no/such/stack-policy.toml";
    let response = handle_message(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "eb_stack_solve",
            "arguments": {"easyconfigs": ["easyconfigs"], "stack_policy": missing}
        }
    }))
    .expect("solve");
    assert_eq!(response["result"]["isError"], true, "{response}");
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("");
    assert!(
        text.contains("stack_policy") && text.contains(missing),
        "{text}"
    );
    assert!(
        !text.contains("accepted for compatibility"),
        "stack_policy is the current spelling and is not warned: {text}"
    );
}

/// `policy` is the retired spelling of `stack_policy`. The CLI warns and
/// still reads the file; the tool does the same.
#[test]
fn mcp_policy_is_warned_like_the_cli_alias() {
    let missing = "/nope-policy.json";
    let response = handle_message(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "eb_stack_solve",
            "arguments": {"easyconfigs": ["easyconfigs"], "policy": missing}
        }
    }))
    .expect("solve");
    assert_eq!(response["result"]["isError"], true, "{response}");
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("");
    assert!(
        text.contains("warning: policy is accepted for compatibility. Use stack_policy."),
        "{text}"
    );
    assert!(
        text.contains("policy /nope-policy.json: not a file"),
        "{text}"
    );

    let both = handle_message(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "eb_stack_solve",
            "arguments": {
                "easyconfigs": ["easyconfigs"],
                "policy": missing,
                "stack_policy": missing
            }
        }
    }))
    .expect("both");
    assert_eq!(both["result"]["isError"], true, "{both}");
    let text = both["result"]["content"][0]["text"].as_str().unwrap_or("");
    assert!(
        text.contains("stack_policy and policy are the same argument; pass stack_policy"),
        "{text}"
    );
    assert!(
        !text.contains("accepted for compatibility"),
        "passing both names is a rejection, not the alias warning: {text}"
    );
}

#[test]
fn mcp_toolchain_fields_are_warned_like_the_cli_flags() {
    let response = handle_message(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "eb_package_inspect",
            "arguments": {
                "source": "/nope.yaml",
                "out_dir": "/tmp/out",
                "toolchain_name": "foss",
                "toolchain_version": "2024a"
            }
        }
    }))
    .expect("inspect");
    assert_eq!(response["result"]["isError"], true, "{response}");
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("");
    assert!(
        text.contains(
            "warning: toolchain_name is accepted for compatibility. Use toolchain as name/version."
        ),
        "{text}"
    );
    assert!(
        text.contains(
            "warning: toolchain_version is accepted for compatibility. Use toolchain as name/version."
        ),
        "{text}"
    );

    let current = handle_message(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "eb_package_inspect",
            "arguments": {
                "source": "/nope.yaml",
                "out_dir": "/tmp/out",
                "toolchain": "foss/2024a"
            }
        }
    }))
    .expect("current spelling");
    let text = current["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("");
    assert!(
        !text.contains("accepted for compatibility"),
        "toolchain is the current spelling and is not warned: {text}"
    );
}

#[test]
fn mcp_rejected_toolchain_fields_are_not_called_accepted() {
    let cases = [
        (
            "eb_package_inspect",
            json!({
                "source": "/nope.yaml",
                "out_dir": "/tmp/out",
                "toolchain": "foss/2024a",
                "toolchain_name": "foss"
            }),
            "toolchain cannot be used with toolchain_name or toolchain_version",
        ),
        (
            "eb_package_bump",
            json!({
                "source": "/nope.eb",
                "easyconfigs": ["/nope"],
                "out_dir": "/tmp/out",
                "toolchain_version": "2024a"
            }),
            "toolchain_name and toolchain_version are required",
        ),
    ];
    for (tool, arguments, needle) in cases {
        let response = handle_message(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": tool, "arguments": arguments }
        }))
        .expect(tool);
        assert_eq!(response["result"]["isError"], true, "{response}");
        let text = response["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or("");
        assert!(text.contains(needle), "{tool}: {text}");
        assert!(
            !text.contains("accepted for compatibility"),
            "{tool} rejects the fields, so they are not accepted: {text}"
        );
    }
}

#[test]
fn mcp_package_tools_declare_and_name_the_required_toolchain() {
    let listed = handle_message(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list"
    }))
    .expect("tools/list");
    let cases = [
        (
            "eb_package_inspect",
            json!(["toolchain_version"]),
            json!({"source": "/nope.yaml", "out_dir": "/tmp/out"}),
        ),
        (
            "eb_package_bump",
            json!(["toolchain_name", "toolchain_version"]),
            json!({"source": "/nope.eb", "easyconfigs": ["/nope"], "out_dir": "/tmp/out"}),
        ),
    ];
    for (tool, split, arguments) in cases {
        let schema = &listed["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["name"] == tool)
            .unwrap_or_else(|| panic!("missing {tool}"))["inputSchema"];
        assert_eq!(
            schema["anyOf"],
            json!([{"required": ["toolchain"]}, {"required": split}]),
            "{tool}"
        );
        let response = handle_message(&json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": { "name": tool, "arguments": arguments }
        }))
        .expect(tool);
        assert_eq!(response["result"]["isError"], true, "{response}");
        let text = response["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or("");
        assert!(
            text.contains("toolchain is required as name/version"),
            "{tool}: {text}"
        );
        assert!(
            !text.contains("toolchain_name") && !text.contains("toolchain_version"),
            "{tool}: a missing toolchain names the listed field only: {text}"
        );
    }
}

#[test]
fn mcp_module_lint_checks_an_easyblock_with_the_repository_line_length() {
    let temp = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        temp.path().join("setup.cfg"),
        "[flake8]\nmax-line-length = 120\n",
    )
    .expect("setup.cfg");
    let dir = temp.path().join("easybuild/easyblocks/x");
    std::fs::create_dir_all(&dir).expect("dirs");
    let module = dir.join("xpkg.py");
    let long = format!(
        "        self.cfg.update('configopts', '{}')\n",
        "a".repeat(100)
    );
    let text = format!(
        "from easybuild.easyblocks.generic.configuremake import ConfigureMake\n\n\nclass EB_xpkg(ConfigureMake):\n    def configure_step(self):\n{long}        super().configure_step()\n"
    );
    std::fs::write(&module, &text).expect("module");
    let response = call(
        40,
        "eb_module_lint",
        json!({"modules": [module], "flake8": false}),
    );
    assert_eq!(response["result"]["isError"], false, "{response}");
    let body = &response["result"]["structuredContent"];
    assert_eq!(body["results"][0]["max_line_length"], 120, "{body}");
    assert_eq!(
        body["clean"], false,
        "a 140-character line is E501 at 120: {body}"
    );
    assert_eq!(body["results"][0]["findings"][0]["code"], "E501", "{body}");

    std::fs::write(&module, text.replace(&long, "        pass\n")).expect("fixed");
    let fixed = call(
        41,
        "eb_module_lint",
        json!({"modules": [module], "flake8": false}),
    );
    assert_eq!(
        fixed["result"]["structuredContent"]["clean"], true,
        "{fixed}"
    );
}

#[test]
fn mcp_recipe_diff_names_the_python_scope_of_each_hunk() {
    let temp = tempfile::tempdir().expect("tempdir");
    let module = temp.path().join("xpkg.py");
    let before = "class EB_xpkg(Base):\n    def configure_step(self):\n        a = 1\n\n    def install_step(self):\n        b = 2\n";
    std::fs::write(&module, before).expect("module");
    let first = call(42, "eb_recipe_diff", json!({"recipes": [module]}));
    assert_eq!(first["result"]["isError"], false, "{first}");
    std::fs::write(&module, before.replace("a = 1", "a = 2")).expect("edit");
    let diff = call(43, "eb_recipe_diff", json!({"recipes": [module]}));
    let hunk = &diff["result"]["structuredContent"]["files"][0]["hunks"][0];
    assert_eq!(hunk["scope"], "EB_xpkg.configure_step", "{diff}");
}

#[test]
fn mcp_eb_tools_refuse_paths_outside_the_workspace() {
    for (id, name, arguments) in [
        (
            44,
            "eb_easyblock_dry_run",
            json!({"recipe": "../../outside.eb", "easyblocks": ["x.py"]}),
        ),
        (45, "eb_build_log", json!({"path": "../../outside.log"})),
    ] {
        let response = call(id, name, arguments);
        assert_eq!(response["result"]["isError"], true, "{name}: {response}");
    }
}

#[test]
fn mcp_build_test_is_refused_unless_builds_are_allowed() {
    if std::env::var_os("EB_STACK_ALLOW_BUILD").is_some() {
        return;
    }
    let response = call(
        46,
        "eb_build_test",
        json!({"recipe": "x.eb", "easyblocks": ["x.py"]}),
    );
    assert_eq!(response["result"]["isError"], true, "{response}");
    assert!(
        response
            .to_string()
            .contains("EB_STACK_ALLOW_BUILD=1 is not set"),
        "{response}"
    );
}

#[test]
fn mcp_dry_run_without_easybuild_is_an_environment_failure() {
    let _guard = WORKSPACE_ENV_LOCK.lock().expect("workspace env lock");
    let temp = tempfile::tempdir().expect("tempdir");
    std::fs::write(temp.path().join("demo.eb"), "name = 'demo'\n").expect("recipe");
    std::fs::write(
        temp.path().join("mod.py"),
        "class EB_Demo(object):\n    pass\n",
    )
    .expect("module");
    let previous_eb = std::env::var_os("EB_STACK_EB");
    let previous_root = std::env::var_os("EB_STACK_WORKSPACE");
    std::env::set_var("EB_STACK_EB", temp.path().join("missing-eb"));
    std::env::set_var("EB_STACK_WORKSPACE", temp.path());
    let response = call(
        91,
        "eb_easyblock_dry_run",
        json!({"recipe": "demo.eb", "easyblocks": ["mod.py"]}),
    );
    match previous_eb {
        Some(value) => std::env::set_var("EB_STACK_EB", value),
        None => std::env::remove_var("EB_STACK_EB"),
    }
    match previous_root {
        Some(value) => std::env::set_var("EB_STACK_WORKSPACE", value),
        None => std::env::remove_var("EB_STACK_WORKSPACE"),
    }
    assert_eq!(response["result"]["isError"], false, "{response}");
    let body = &response["result"]["structuredContent"];
    assert_eq!(body["failure_kind"], "environment", "{body}");
    assert!(
        body["summary"]
            .as_str()
            .unwrap_or("")
            .contains("not a fault in your easyblock"),
        "{body}"
    );
}

fn json_after(text: &str, label: &str) -> serde_json::Value {
    let start = text
        .find(label)
        .unwrap_or_else(|| panic!("missing {label}\n{text}"))
        + label.len();
    serde_json::Deserializer::from_str(&text[start..])
        .into_iter::<serde_json::Value>()
        .next()
        .unwrap_or_else(|| panic!("no json after {label}"))
        .unwrap_or_else(|error| panic!("{label}: {error}"))
}

fn source_classes(text: &str) -> Vec<serde_json::Value> {
    text.lines()
        .filter_map(|line| {
            let rest = line.strip_prefix("source_class=")?;
            let (class, url) = rest.split_once(" url=")?;
            Some(json!({"class": class, "url": url}))
        })
        .collect()
}

fn bracket_findings(text: &str) -> Vec<String> {
    text.lines()
        .filter(|line| line.starts_with("[err] ") || line.starts_with("[warn] "))
        .map(str::to_string)
        .collect()
}

fn mcp_tool(id: i64, name: &str, arguments: serde_json::Value) -> serde_json::Value {
    let response = handle_message(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": {"name": name, "arguments": arguments}
    }))
    .expect("mcp response");
    assert_eq!(response["result"]["isError"], false, "{response}");
    response["result"]["structuredContent"].clone()
}

#[test]
fn mcp_recipe_findings_match_the_cli_on_hardcases_and_eon() {
    let bin = env!("CARGO_BIN_EXE_eb-stack");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let recipes = [
        "fixtures/parser_hardcases/easyconfigs/builddeps_only.eb",
        "fixtures/parser_hardcases/easyconfigs/local_vars.eb",
        "fixtures/parser_hardcases/easyconfigs/multi_element_deps.eb",
        "fixtures/parser_hardcases/easyconfigs/system_toolchain.eb",
        "fixtures/parser_hardcases/easyconfigs/templates_version.eb",
        "fixtures/eon_core_rgpot/easyconfigs/e/eOn/eOn-2.17.10-foss-2026.1.eb",
    ];
    for (index, rel) in recipes.iter().enumerate() {
        let path = root.join(rel);
        let recipe = path.to_str().expect("utf8 recipe path");
        let easyconfigs = path
            .ancestors()
            .find(|ancestor| {
                ancestor
                    .file_name()
                    .is_some_and(|name| name == "easyconfigs")
            })
            .expect("easyconfigs directory")
            .to_str()
            .expect("utf8 easyconfigs path");
        let output = std::process::Command::new(bin)
            .args([
                "recipe",
                "check",
                "--recipe",
                recipe,
                "--easyconfigs",
                easyconfigs,
                "--metadata-only",
                "--strict",
                "--verify-sources",
            ])
            .output()
            .expect("recipe check");
        let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
        let body = mcp_tool(
            20 + index as i64,
            "eb_recipe_check",
            json!({
                "recipe": recipe,
                "easyconfigs": [easyconfigs],
                "metadata_only": true,
                "strict": true,
                "verify_sources": true,
            }),
        );
        assert!(
            stdout.contains("strict: every statement parsed"),
            "{rel}\n{stdout}"
        );
        assert_eq!(
            body["strict"]["skipped"].as_array().map(Vec::len),
            Some(0),
            "{rel}"
        );
        assert_eq!(
            body["maintainer_acceptability"],
            json_after(&stdout, "maintainer_acceptability="),
            "{rel}\n{stdout}"
        );
        assert_eq!(
            body["easyblock_resolution"],
            json_after(&stdout, "easyblock_resolution="),
            "{rel}\n{stdout}"
        );
        assert_eq!(
            body["sources"],
            json!(source_classes(&stdout)),
            "{rel}\n{stdout}"
        );
        assert_eq!(
            body["source_findings"],
            json!(bracket_findings(&stdout)),
            "{rel}\n{stdout}"
        );
        assert_eq!(
            rel.contains("eOn"),
            stdout.contains("TheochemUI/eOn/archive"),
            "{rel}\n{stdout}"
        );

        let lint = std::process::Command::new(bin)
            .args(["recipe", "lint", recipe])
            .output()
            .expect("recipe lint");
        let cli_lint: serde_json::Value = serde_json::from_slice(&lint.stdout).expect("lint json");
        let mcp_lint = mcp_tool(
            40 + index as i64,
            "eb_recipe_lint",
            json!({"recipes": [recipe]}),
        );
        assert_eq!(
            mcp_lint["results"][0]["findings"], cli_lint["style"],
            "{rel}"
        );
        assert_eq!(
            mcp_lint["results"][0]["maintainer_acceptability"],
            cli_lint["maintainer_acceptability"],
            "{rel}"
        );

        let before = std::fs::read(recipe).expect("recipe bytes");
        let formatted = std::process::Command::new(bin)
            .args(["recipe", "format", "--dry-run", recipe])
            .output()
            .expect("recipe format");
        let format_out = String::from_utf8(formatted.stdout).expect("utf8 format");
        let line = format_out.lines().next().expect("format summary");
        let (_, rest) = line.split_once("rewritten=").expect("rewritten");
        let (rewritten, remaining) = rest.split_once(" remaining=").expect("remaining");
        let mcp_format = mcp_tool(
            60 + index as i64,
            "eb_recipe_format",
            json!({"recipe": recipe, "dry_run": true}),
        );
        assert_eq!(
            mcp_format["rewritten"].as_u64(),
            rewritten.parse().ok(),
            "{rel} {line}"
        );
        assert_eq!(
            mcp_format["remaining"].as_array().map(Vec::len),
            remaining.trim().parse::<usize>().ok(),
            "{rel} {line}"
        );
        assert_eq!(std::fs::read(recipe).expect("reread"), before, "{rel}");
    }
}

#[test]
fn mcp_component_reach_accepts_a_purl() {
    let _lock = WORKSPACE_ENV_LOCK.lock().expect("lock");
    let previous = std::env::var("EASYBUILD_FILTER_DEPS").ok();
    std::env::remove_var("EASYBUILD_FILTER_DEPS");
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    std::fs::write(
        root.join("cURL.eb"),
        "name = 'cURL'\nversion = '8.17.0'\ntoolchain = {'name': 'GCCcore', 'version': '15.2.0'}\n",
    )
    .unwrap();
    let out = root.join("out");
    let response = handle_message(&json!({
        "jsonrpc": "2.0",
        "id": 9,
        "method": "tools/call",
        "params": {
            "name": "eb_component_reach",
            "arguments": {
                "purl": "pkg:generic/curl@8.17.0",
                "generation": "2026.1",
                "easyconfigs": [root],
                "generated_at": "2026-10-22T12:00:00Z",
                "out_dir": out
            }
        }
    }))
    .expect("component reach");
    if let Some(value) = previous {
        std::env::set_var("EASYBUILD_FILTER_DEPS", value);
    }
    assert_eq!(response["result"]["isError"], false, "{response}");
    let body = &response["result"]["structuredContent"];
    assert_eq!(body["component"], "curl");
    assert_eq!(body["purl"], "pkg:generic/curl@8.17.0");
    assert_eq!(body["carriers"][0]["role"], "module");
    assert!(out.join("component-reach.json").is_file());
    let refused = handle_message(&json!({
        "jsonrpc": "2.0",
        "id": 10,
        "method": "tools/call",
        "params": {
            "name": "eb_component_reach",
            "arguments": {
                "generation": "2026.1",
                "easyconfigs": [root],
                "easystack_url": ["http://example.invalid/stack.yml"]
            }
        }
    }))
    .expect("refused url");
    assert_eq!(refused["result"]["isError"], true, "{refused}");
}

fn cli_long_flags(help: &str) -> Vec<String> {
    let mut flags = Vec::new();
    let mut options = false;
    for line in help.lines() {
        if line == "Options:" {
            options = true;
            continue;
        }
        if !options || line.trim().is_empty() {
            continue;
        }
        let trimmed = line.trim();
        let Some(start) = trimmed.find("--") else {
            continue;
        };
        let before = &trimmed[..start];
        if !(before.is_empty() || before.ends_with(", ")) {
            continue;
        }
        let name = trimmed[start + 2..]
            .split(|c: char| c.is_whitespace() || c == '=')
            .next()
            .unwrap_or("");
        if !name.is_empty() && name != "help" {
            flags.push(name.to_string());
        }
    }
    flags
}

#[test]
fn mcp_schemas_cover_cli_flags() {
    let bin = env!("CARGO_BIN_EXE_eb-stack");
    let catalog = handle_message(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
        "params": {}
    }))
    .expect("tools/list");
    let tools = catalog["result"]["tools"].as_array().expect("tools");
    let commands = [
        ("eb_package_inspect", "package inspect"),
        ("eb_package_plan", "package plan"),
        ("eb_package_bump", "package bump"),
        ("eb_recipe_easyblock", "recipe easyblock"),
        ("eb_recipe_templates", "recipe templates"),
        ("eb_recipe_hierarchy", "recipe hierarchy"),
        ("eb_recipe_check", "recipe check"),
        ("eb_recipe_lint", "recipe lint"),
        ("eb_recipe_format", "recipe format"),
        ("eb_stack_solve", "stack solve"),
        ("eb_stack_sbom", "stack sbom"),
        ("eb_stack_order", "stack order"),
        ("eb_stack_easystack", "stack easystack"),
        ("eb_stack_sections", "stack sections"),
        ("eb_stack_from_commit", "stack from-commit"),
        ("eb_stack_gap", "stack gap"),
        ("eb_stack_batch", "stack batch"),
        ("eb_stack_arch", "stack arch"),
        ("eb_stack_files", "stack files"),
        ("eb_package_latest", "package latest"),
        ("eb_repro_summary", "repro summary"),
        ("eb_component_reach", "component-reach"),
        ("eb_target_list", "target list"),
        ("eb_target_doctor", "target doctor"),
        ("eb_campaign_run", "campaign run"),
        ("eb_campaign_status", "campaign status"),
        ("eb_campaign_finding_claim", "campaign finding claim"),
        ("eb_campaign_finding_resolve", "campaign finding resolve"),
        ("eb_pr_overlap", "pr-overlap"),
        ("eb_rebuild_set", "rebuild-set"),
        ("eb_component_reach", "component-reach"),
        ("eb_easyblock_reach", "easyblock-reach"),
    ];
    // These tools edit files or run EasyBuild. They are not a CLI subcommand,
    // so they have no `--help` flag list to mirror. A tool in neither list
    // is unmapped and fails the audit.
    let mcp_only = [
        "eb_recipe_read",
        "eb_recipe_list",
        "eb_recipe_edit",
        "eb_recipe_write",
        "eb_recipe_grep",
        "eb_recipe_apply_suggestion",
        "eb_recipe_diff",
        "eb_dep_versions",
        "eb_module_lint",
        "eb_easyblock_dry_run",
        "eb_build_log",
        "eb_build_test",
    ];
    let allow = [
        (
            "eb_package_inspect",
            "package-config",
            "repeatable flag, schema key package_configs",
        ),
        (
            "eb_package_plan",
            "package-catalog",
            "repeatable flag, schema key package_catalogs",
        ),
        (
            "eb_package_plan",
            "package-config",
            "repeatable flag, schema key package_configs",
        ),
        (
            "eb_package_plan",
            "source-checksum",
            "repeatable flag, schema key source_checksums",
        ),
        (
            "eb_package_plan",
            "cargo-source",
            "repeatable flag, schema key cargo_sources",
        ),
        (
            "eb_package_plan",
            "conda-source",
            "repeatable flag, schema key conda_sources",
        ),
        (
            "eb_package_plan",
            "easybuild-source",
            "repeatable flag, schema key easybuild_sources",
        ),
        (
            "eb_package_plan",
            "spack-source",
            "repeatable flag, schema key spack_sources",
        ),
        (
            "eb_package_bump",
            "dep",
            "repeatable NAME=VERSION flag, schema key dependencies",
        ),
        (
            "eb_package_bump",
            "package-config",
            "repeatable flag, schema key package_configs",
        ),
        (
            "eb_recipe_check",
            "require-configopt",
            "repeatable flag, schema key require_configopts",
        ),
        (
            "eb_target_list",
            "config",
            "repeatable flag, schema key configs",
        ),
        (
            "eb_target_doctor",
            "config",
            "repeatable flag, schema key configs",
        ),
        (
            "eb_campaign_run",
            "config",
            "repeatable flag, schema key configs",
        ),
        (
            "eb_campaign_finding_resolve",
            "change",
            "repeatable flag, schema key changes",
        ),
    ];
    assert!(allow.iter().all(|(_, _, reason)| !reason.is_empty()));
    let names = tools
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect::<Vec<_>>();
    for name in &names {
        let mirrors_cli = commands.iter().any(|(tool, _)| tool == name);
        let only = mcp_only.contains(name);
        assert!(
            mirrors_cli || only,
            "unmapped MCP tool {name}; add it to the CLI map or to mcp_only"
        );
    }
    for (tool, _) in commands {
        assert!(
            names.contains(&tool),
            "CLI mirror missing from the catalog: {tool}"
        );
    }
    for tool in mcp_only {
        assert!(names.contains(&tool), "MCP-only tool dropped: {tool}");
    }
    let mut used = vec![false; allow.len()];
    for (name, argv) in commands {
        assert!(names.contains(&name), "missing tool {name}");
        let schema = tools
            .iter()
            .find(|tool| tool["name"] == name)
            .unwrap_or_else(|| panic!("schema {name}"));
        let properties = schema["inputSchema"]["properties"]
            .as_object()
            .unwrap_or_else(|| panic!("properties {name}"));
        let help = std::process::Command::new(bin)
            .args(argv.split_whitespace())
            .arg("--help")
            .output()
            .unwrap_or_else(|error| panic!("{name} help: {error}"));
        let text = String::from_utf8(help.stdout).expect("utf8 help");
        for flag in cli_long_flags(&text) {
            let snake = flag.replace('-', "_");
            if properties.contains_key(&snake) {
                continue;
            }
            let Some(index) = allow
                .iter()
                .position(|(tool, named, _)| *tool == name && *named == flag)
            else {
                panic!("{name} has no schema property for --{flag}");
            };
            used[index] = true;
        }
    }
    let unused = allow
        .iter()
        .enumerate()
        .filter(|(index, _)| !used[*index])
        .map(|(_, (_, flag, _))| *flag)
        .collect::<Vec<_>>();
    assert!(
        unused.is_empty(),
        "allowlist entries no flag uses: {unused:?}"
    );
}

#[test]
fn mcp_pr_overlap_writes_the_same_files_as_the_command() {
    let _guard = WORKSPACE_ENV_LOCK.lock().expect("lock");
    let previous = std::env::var_os("EASYBUILD_FILTER_DEPS");
    std::env::remove_var("EASYBUILD_FILTER_DEPS");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let release = root.join("tests/pr_overlap_fixtures/release");
    let installed = root.join("tests/pr_overlap_fixtures/installed.txt");
    let stack = "easystacks/software.eessi.io/2026.06/eessi-2026.06-eb-5.4.0-2026.1.yml";
    let base = "easyconfigs:\n  - zlib-1.3.1-GCCcore-15.2.0.eb\n";
    let one = |n: u64, added: &str| {
        json!({
            "number": n,
            "head_sha": format!("{n:040}"),
            "title": format!("PR {n}"),
            "easystacks": [{
                "path": stack,
                "base": base,
                "head": format!("{base}{added}"),
            }],
        })
    };
    let prs = json!([
        one(11, "  - freetype-2.14.1-GCCcore-15.2.0.eb\n"),
        one(12, "  - Graphviz-14.0.2-GCCcore-15.2.0.eb\n"),
        one(13, "  - freetype-2.14.1-GCCcore-15.2.0.eb\n"),
    ]);
    let temp = tempfile::tempdir().expect("temp");
    let input = temp.path().join("prs.json");
    std::fs::write(&input, serde_json::to_string(&prs).unwrap()).unwrap();
    let cli_out = temp.path().join("cli");
    let mcp_out = temp.path().join("mcp");
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .env_remove("EASYBUILD_FILTER_DEPS")
        .args(["pr-overlap", "--pr"])
        .arg(&input)
        .arg("--easyconfigs")
        .arg(&release)
        .arg("--installed")
        .arg(&installed)
        .args(["--installed-label", "x86_64/amd/zen4"])
        .args(["--generated-at", "2026-10-22T00:00:00Z"])
        .args(["--repo-url", "https://github.com/EESSI/software-layer"])
        .args(["--merge-order", "--impact", "11", "--out-dir"])
        .arg(&cli_out)
        .status()
        .expect("cli");
    assert!(status.success(), "cli status {status}");
    let response = call(
        90,
        "eb_pr_overlap",
        json!({
            "pr": [input],
            "easyconfigs": [release],
            "installed": installed,
            "installed_label": "x86_64/amd/zen4",
            "generated_at": "2026-10-22T00:00:00Z",
            "repo_url": "https://github.com/EESSI/software-layer",
            "merge_order": true,
            "impact": [11],
            "out_dir": mcp_out,
        }),
    );
    assert_eq!(response["result"]["isError"], false, "{response}");
    assert_eq!(response["result"]["structuredContent"]["records"], 3);
    for name in [
        "overlap.toml",
        "overlap.md",
        "merge-order.toml",
        "merge-impact.toml",
        "prs/11.toml",
        "prs/12.toml",
    ] {
        let left = std::fs::read(cli_out.join(name)).unwrap_or_else(|_| panic!("cli {name}"));
        let right = std::fs::read(mcp_out.join(name)).unwrap_or_else(|_| panic!("mcp {name}"));
        assert_eq!(left, right, "{name}");
    }
    let easy = root.join("tests/pr_overlap_easyconfigs_fixtures");
    let lib = std::fs::read_to_string(easy.join("l/Lib/Lib-1.0.eb")).unwrap();
    let head = lib.replace("Fixture.", "Head.");
    let easy_prs = json!([
        {
            "number": 1,
            "head_sha": "1".repeat(40),
            "title": "edit Lib",
            "easyconfigs": [{"path": "l/Lib/Lib-1.0.eb", "base": lib, "head": head}],
        },
        {
            "number": 2,
            "head_sha": "2".repeat(40),
            "title": "edit Lib too",
            "easyconfigs": [{"path": "l/Lib/Lib-1.0.eb", "base": lib, "head": head}],
        },
    ]);
    let easy_input = temp.path().join("easy.json");
    std::fs::write(&easy_input, serde_json::to_string(&easy_prs).unwrap()).unwrap();
    let easy_cli = temp.path().join("easy-cli");
    let easy_mcp = temp.path().join("easy-mcp");
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .env_remove("EASYBUILD_FILTER_DEPS")
        .args(["pr-overlap", "--kind", "easyconfigs", "--pr"])
        .arg(&easy_input)
        .arg("--easyconfigs")
        .arg(&easy)
        .args(["--generated-at", "2026-10-22T00:00:00Z", "--out-dir"])
        .arg(&easy_cli)
        .status()
        .expect("easy cli");
    assert!(status.success(), "easy cli {status}");
    let response = call(
        91,
        "eb_pr_overlap",
        json!({
            "pr": [easy_input],
            "kind": "easyconfigs",
            "easyconfigs": [easy],
            "generated_at": "2026-10-22T00:00:00Z",
            "out_dir": easy_mcp,
        }),
    );
    assert_eq!(response["result"]["isError"], false, "{response}");
    let left = std::fs::read(easy_cli.join("overlap.toml")).unwrap();
    let right = std::fs::read(easy_mcp.join("overlap.toml")).unwrap();
    assert_eq!(left, right);
    assert!(std::str::from_utf8(&right).unwrap().contains("same-file"));
    let missing = call(
        92,
        "eb_pr_overlap",
        json!({
            "pr": [input],
            "easyconfigs": [release],
            "out_dir": temp.path().join("missing"),
        }),
    );
    assert_eq!(missing["result"]["isError"], true, "{missing}");
    if let Some(previous) = previous {
        std::env::set_var("EASYBUILD_FILTER_DEPS", previous);
    }
}

/// The CLI accepts `--osv-cache`. The tool schema has to list it too.
#[test]
fn mcp_component_reach_lists_osv_cache() {
    let listed = handle_message(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
        "params": {}
    }))
    .expect("tools/list");
    let tools = listed["result"]["tools"].as_array().expect("tools");
    let reach = tools
        .iter()
        .find(|tool| tool["name"] == "eb_component_reach")
        .expect("eb_component_reach");
    assert_eq!(
        reach["inputSchema"]["properties"]["osv_cache"]["type"], "string",
        "{reach}"
    );
    let required = reach["inputSchema"]["required"]
        .as_array()
        .expect("required");
    assert!(
        required.iter().all(|item| item != "osv_cache"),
        "osv_cache is optional: {required:?}"
    );

    let response = handle_message(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "eb_component_reach",
            "arguments": {
                "generation": "2026.1",
                "easyconfigs": ["/nope-tree"],
                "component": "curl",
                "version": "8.17.0",
                "osv_cache": "/nope-osv"
            }
        }
    }))
    .expect("call");
    assert_eq!(response["result"]["isError"], true, "{response}");
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("");
    assert!(
        text.contains("--osv-cache") && text.contains("/nope-osv"),
        "{text}"
    );
}

/// `merged` and `impact` are arrays on the CLI. A bare number is not one.
#[test]
fn mcp_pr_overlap_rejects_a_scalar_merged_or_impact() {
    let scalar = handle_message(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "eb_pr_overlap",
            "arguments": {
                "pr": ["/nope.json"],
                "easyconfigs": ["/nope-tree"],
                "merged": 9,
                "impact": 9
            }
        }
    }))
    .expect("scalar call");
    assert_eq!(scalar["result"]["isError"], true, "{scalar}");
    let text = scalar["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("");
    assert!(
        text.contains("argument merged must be an array of integers"),
        "{text}"
    );
    assert!(
        !text.contains("nope.json") && !text.contains("not readable"),
        "the scalar is rejected before any file is read: {text}"
    );

    let array = handle_message(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "eb_pr_overlap",
            "arguments": {
                "pr": ["/nope.json"],
                "easyconfigs": ["/nope-tree"],
                "kind": "easyconfigs",
                "merged": [9]
            }
        }
    }))
    .expect("array call");
    assert_eq!(array["result"]["isError"], true, "{array}");
    let text = array["result"]["content"][0]["text"].as_str().unwrap_or("");
    assert!(
        !text.contains("must be an array of integers"),
        "an array of integers is the accepted shape: {text}"
    );
    assert!(
        text.contains("nope.json"),
        "an integer array proceeds to the pull-request file: {text}"
    );

    let impact = handle_message(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "eb_pr_overlap",
            "arguments": {
                "pr": ["/nope.json"],
                "easyconfigs": ["/nope-tree"],
                "impact": 9
            }
        }
    }))
    .expect("impact call");
    assert_eq!(impact["result"]["isError"], true, "{impact}");
    let text = impact["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("");
    assert!(
        text.contains("argument impact must be an array of integers"),
        "{text}"
    );
}

fn eb_output(args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args(args)
        .output()
        .expect("eb-stack")
}

fn cli_stdout(args: &[&str]) -> String {
    let output = eb_output(args);
    assert!(
        output.status.success(),
        "status {}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("utf8 stdout")
}

/// The text after the CLI's `error: ` prefix. MCP returns that same text.
fn cli_error(args: &[&str]) -> String {
    let output = eb_output(args);
    assert!(!output.status.success(), "expected a failing command");
    let stderr = String::from_utf8(output.stderr).expect("utf8 stderr");
    stderr
        .strip_prefix("error: ")
        .unwrap_or(&stderr)
        .trim_end()
        .to_string()
}

fn mcp_error_text(id: u64, name: &str, arguments: serde_json::Value) -> String {
    let response = call(id, name, arguments);
    assert_eq!(response["result"]["isError"], true, "{response}");
    response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

fn tool_schema(name: &str) -> serde_json::Value {
    let listed = handle_message(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
        "params": {}
    }))
    .expect("tools/list");
    listed["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .find(|tool| tool["name"] == name)
        .unwrap_or_else(|| panic!("missing {name}"))
        .clone()
}

fn assert_schema(name: &str, required: &[&str], properties: &[(&str, &str)]) {
    let schema = tool_schema(name);
    let got = schema["inputSchema"]["required"]
        .as_array()
        .unwrap_or_else(|| panic!("required {name}"));
    let got_names = got
        .iter()
        .map(|item| item.as_str().unwrap_or(""))
        .collect::<Vec<_>>();
    assert_eq!(got_names, required, "{name} required");
    for (property, kind) in properties {
        assert_eq!(
            schema["inputSchema"]["properties"][property]["type"].as_str(),
            Some(*kind),
            "{name}.{property}"
        );
    }
}

fn swap_paths(text: &str, from: &std::path::Path, to: &str) -> String {
    text.replace(&from.display().to_string(), to)
}

fn help_subcommands(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut commands = false;
    for line in text.lines() {
        if line == "Commands:" {
            commands = true;
            continue;
        }
        if !commands {
            continue;
        }
        if line.trim().is_empty() {
            if !names.is_empty() {
                break;
            }
            continue;
        }
        if !line.starts_with("  ") || line.starts_with("   ") {
            if !line.starts_with(' ') {
                break;
            }
            continue;
        }
        let name = line.split_whitespace().next().unwrap_or("");
        if !name.is_empty() && name != "help" {
            names.push(name.to_string());
        }
    }
    names
}

fn cli_leaves() -> Vec<Vec<String>> {
    let bin = env!("CARGO_BIN_EXE_eb-stack");
    let mut leaves = Vec::new();
    let mut queue = vec![Vec::<String>::new()];
    while let Some(args) = queue.pop() {
        let output = std::process::Command::new(bin)
            .args(&args)
            .arg("--help")
            .output()
            .expect("help");
        assert!(
            output.status.success(),
            "help {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8(output.stdout).expect("utf8 help");
        let subs = help_subcommands(&text);
        if subs.is_empty() {
            leaves.push(args);
        } else {
            for name in subs {
                let mut next = args.clone();
                next.push(name);
                queue.push(next);
            }
        }
    }
    leaves.sort();
    leaves
}

#[test]
fn mcp_recipe_easyblock_matches_the_cli() {
    assert_schema(
        "eb_recipe_easyblock",
        &[],
        &[("name", "string"), ("charmap", "boolean")],
    );
    let stdout = cli_stdout(&["recipe", "easyblock", "zlib"]);
    let body = mcp_tool(101, "eb_recipe_easyblock", json!({"name": "zlib"}));
    assert_eq!(body["text"], stdout);
    assert_eq!(body["class_name"], "EB_zlib");
    assert!(body["charmap"].is_null());

    let table = cli_stdout(&["recipe", "easyblock", "--charmap"]);
    let named = cli_stdout(&["recipe", "easyblock", "zlib", "--charmap"]);
    assert_eq!(table, named);
    let body = mcp_tool(
        102,
        "eb_recipe_easyblock",
        json!({"name": "zlib", "charmap": true}),
    );
    assert_eq!(body["text"], table);
    assert_eq!(
        body["charmap"],
        serde_json::from_str::<serde_json::Value>(table.trim()).unwrap()
    );
    assert!(body["class_name"].is_null());
}

#[test]
fn mcp_recipe_templates_matches_the_cli() {
    assert_schema(
        "eb_recipe_templates",
        &[],
        &[
            ("name", "string"),
            ("version", "string"),
            ("export", "boolean"),
        ],
    );
    let exported = cli_stdout(&["recipe", "templates", "--export"]);
    let body = mcp_tool(111, "eb_recipe_templates", json!({"export": true}));
    assert_eq!(body["text"], exported);
    assert_eq!(
        body["document"],
        serde_json::from_str::<serde_json::Value>(exported.trim()).unwrap()
    );

    let resolved = cli_stdout(&[
        "recipe",
        "templates",
        "--name",
        "zlib",
        "--version",
        "1.3.1",
    ]);
    let body = mcp_tool(
        112,
        "eb_recipe_templates",
        json!({"name": "zlib", "version": "1.3.1"}),
    );
    assert_eq!(body["text"], resolved);
    assert_eq!(
        body["document"],
        serde_json::from_str::<serde_json::Value>(resolved.trim()).unwrap()
    );
    assert!(resolved.contains("\"arch\""));
    assert!(resolved.contains("\"version_major\""));

    let cli = cli_error(&["recipe", "templates"]);
    let mcp = mcp_error_text(113, "eb_recipe_templates", json!({}));
    assert_eq!(mcp, cli);
    assert!(mcp.contains("--name, or --export"), "{mcp}");
}

#[test]
fn mcp_recipe_hierarchy_matches_the_cli() {
    assert_schema("eb_recipe_hierarchy", &[], &[("export", "boolean")]);
    let plain = cli_stdout(&["recipe", "hierarchy"]);
    let exported = cli_stdout(&["recipe", "hierarchy", "--export"]);
    assert_ne!(plain, exported);
    assert!(plain.contains("\"generations\""), "{plain}");
    assert!(!plain.contains("\"synthetic\""), "{plain}");
    assert!(exported.contains("\"synthetic\""), "{exported}");
    let body = mcp_tool(121, "eb_recipe_hierarchy", json!({}));
    let flagged = mcp_tool(122, "eb_recipe_hierarchy", json!({"export": true}));
    assert_eq!(body["text"], plain);
    assert_eq!(flagged["text"], exported);
    assert_ne!(body["document"], flagged["document"]);
    assert_eq!(
        body["document"],
        serde_json::from_str::<serde_json::Value>(plain.trim()).unwrap()
    );
}

#[test]
fn mcp_stack_order_matches_the_cli() {
    assert_schema(
        "eb_stack_order",
        &["easyconfigs", "root"],
        &[
            ("easyconfigs", "array"),
            ("root", "array"),
            ("out", "string"),
            ("oldest", "boolean"),
            ("easystack_out", "string"),
            ("hashes_out", "string"),
            ("filter_deps", "array"),
        ],
    );
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let trees = root.join("fixtures/gromacs_2025_to_next/easyconfigs");
    let trees = trees.to_str().expect("utf8");
    let temp = tempfile::tempdir().expect("temp");
    let cli_order = temp.path().join("cli-order.txt");
    let cli_stack = temp.path().join("cli-stack.yml");
    let cli_hashes = temp.path().join("cli-hashes.txt");
    let mcp_order = temp.path().join("mcp-order.txt");
    let mcp_stack = temp.path().join("mcp-stack.yml");
    let mcp_hashes = temp.path().join("mcp-hashes.txt");
    let stdout = cli_stdout(&[
        "stack",
        "order",
        "--easyconfigs",
        trees,
        "--root",
        "GROMACS",
        "--root",
        "OpenBLAS==0.3.27",
        "--out",
        cli_order.to_str().unwrap(),
        "--easystack-out",
        cli_stack.to_str().unwrap(),
        "--hashes-out",
        cli_hashes.to_str().unwrap(),
    ]);
    let body = mcp_tool(
        131,
        "eb_stack_order",
        json!({
            "easyconfigs": [trees],
            "root": ["GROMACS", "OpenBLAS==0.3.27"],
            "out": mcp_order,
            "easystack_out": mcp_stack,
            "hashes_out": mcp_hashes,
        }),
    );
    let cli_norm = swap_paths(&stdout, &cli_hashes, "HASHES");
    let cli_norm = swap_paths(&cli_norm, &cli_stack, "EASY");
    let cli_norm = swap_paths(&cli_norm, &cli_order, "ORDER");
    let mcp_text = body["text"].as_str().unwrap_or("");
    let mcp_norm = swap_paths(mcp_text, &mcp_hashes, "HASHES");
    let mcp_norm = swap_paths(&mcp_norm, &mcp_stack, "EASY");
    let mcp_norm = swap_paths(&mcp_norm, &mcp_order, "ORDER");
    assert_eq!(mcp_norm, cli_norm);
    assert_eq!(body["builds"], 5);
    assert_eq!(
        std::fs::read(&cli_order).unwrap(),
        std::fs::read(&mcp_order).unwrap()
    );
    assert_eq!(
        std::fs::read(&cli_stack).unwrap(),
        std::fs::read(&mcp_stack).unwrap()
    );
    assert_eq!(
        std::fs::read(&cli_hashes).unwrap(),
        std::fs::read(&mcp_hashes).unwrap()
    );
    assert!(stdout.contains("builds=5"));
    assert!(stdout.contains("modules=5"));
    assert!(stdout.contains("incomplete=0"));
    assert!(!stdout.contains("multiple builds"));

    let cli_old = temp.path().join("cli-oldest.txt");
    let mcp_old = temp.path().join("mcp-oldest.txt");
    let old_stdout = cli_stdout(&[
        "stack",
        "order",
        "--easyconfigs",
        trees,
        "--root",
        "GROMACS",
        "--root",
        "OpenBLAS==0.3.27",
        "--oldest",
        "--out",
        cli_old.to_str().unwrap(),
    ]);
    let old = mcp_tool(
        132,
        "eb_stack_order",
        json!({
            "easyconfigs": [trees],
            "root": ["GROMACS", "OpenBLAS==0.3.27"],
            "oldest": true,
            "out": mcp_old,
        }),
    );
    assert_eq!(
        swap_paths(old["text"].as_str().unwrap_or(""), &mcp_old, "ORDER"),
        swap_paths(&old_stdout, &cli_old, "ORDER")
    );
    assert_eq!(
        std::fs::read(&cli_old).unwrap(),
        std::fs::read(&mcp_old).unwrap()
    );
    let oldest_builds = old_stdout
        .lines()
        .rev()
        .find_map(|line| line.rsplit(" builds=").next())
        .expect("builds")
        .trim()
        .parse::<u64>()
        .expect("builds count");
    assert_eq!(old["builds"], oldest_builds);
    assert!(oldest_builds >= 5, "{old_stdout}");

    let cli_filtered = temp.path().join("cli-filtered.txt");
    let mcp_filtered = temp.path().join("mcp-filtered.txt");
    let filtered_stdout = cli_stdout(&[
        "stack",
        "order",
        "--easyconfigs",
        trees,
        "--root",
        "GROMACS",
        "--filter-deps",
        "OpenBLAS",
        "--out",
        cli_filtered.to_str().unwrap(),
    ]);
    let filtered = mcp_tool(
        134,
        "eb_stack_order",
        json!({
            "easyconfigs": [trees],
            "root": ["GROMACS"],
            "filter_deps": ["OpenBLAS"],
            "out": mcp_filtered,
        }),
    );
    assert_eq!(
        swap_paths(
            filtered["text"].as_str().unwrap_or(""),
            &mcp_filtered,
            "ORDER"
        ),
        swap_paths(&filtered_stdout, &cli_filtered, "ORDER")
    );
    assert_eq!(
        std::fs::read(&cli_filtered).unwrap(),
        std::fs::read(&mcp_filtered).unwrap()
    );
    let filtered_text = std::fs::read_to_string(&mcp_filtered).unwrap();
    assert!(!filtered_text.contains("OpenBLAS-"), "{filtered_text}");
    assert!(filtered_text.contains("GROMACS-"), "{filtered_text}");

    let missing = mcp_error_text(
        133,
        "eb_stack_order",
        json!({"easyconfigs": [trees], "root": []}),
    );
    assert_eq!(missing, "argument root requires at least one value");
}

#[test]
fn mcp_stack_easystack_matches_the_cli() {
    assert_schema(
        "eb_stack_easystack",
        &["lock"],
        &[("lock", "string"), ("out", "string"), ("option", "array")],
    );
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let lock = root.join("fixtures/gromacs_2025_to_next/expected_prefer_newer.lock.json");
    let lock = lock.to_str().expect("utf8");
    let temp = tempfile::tempdir().expect("temp");
    let cli_out = temp.path().join("cli.yml");
    let mcp_out = temp.path().join("mcp.yml");
    let option = "FFTW-3.3.10-foss-2025b.eb:from-pr=24804";
    let stdout = cli_stdout(&[
        "stack",
        "easystack",
        "--lock",
        lock,
        "--out",
        cli_out.to_str().unwrap(),
        "--option",
        option,
    ]);
    let body = mcp_tool(
        141,
        "eb_stack_easystack",
        json!({"lock": lock, "out": mcp_out, "option": [option]}),
    );
    assert_eq!(
        swap_paths(body["text"].as_str().unwrap_or(""), &mcp_out, "OUT"),
        swap_paths(&stdout, &cli_out, "OUT")
    );
    assert_eq!(body["entries"], 5);
    assert_eq!(body["packages"], 5);
    let cli_yaml = std::fs::read(&cli_out).unwrap();
    let mcp_yaml = std::fs::read(&mcp_out).unwrap();
    assert_eq!(cli_yaml, mcp_yaml);
    let yaml = String::from_utf8(mcp_yaml).unwrap();
    assert!(yaml.contains("from-pr: 24804"), "{yaml}");

    let bad = temp.path().join("bad.yml");
    let cli = cli_error(&[
        "stack",
        "easystack",
        "--lock",
        lock,
        "--out",
        bad.to_str().unwrap(),
        "--option",
        "nope",
    ]);
    let mcp = mcp_error_text(
        142,
        "eb_stack_easystack",
        json!({"lock": lock, "out": bad, "option": ["nope"]}),
    );
    assert_eq!(mcp, cli);
    assert!(
        mcp.contains("--option wants <file.eb>:<option>=<value>, got nope"),
        "{mcp}"
    );
}

#[test]
fn mcp_stack_sections_matches_the_cli() {
    assert_schema(
        "eb_stack_sections",
        &["list", "from", "out"],
        &[
            ("list", "string"),
            ("from", "string"),
            ("out", "string"),
            ("unfiled", "string"),
        ],
    );
    let temp = tempfile::tempdir().expect("temp");
    let previous = temp.path().join("previous.list");
    std::fs::write(
        &previous,
        "#### MPI ####\n# OpenMPI-4.1.5-foss-2025a.eb\n#### Chemistry ####\nGROMACS-2024.1-foss-2025a.eb\n",
    )
    .unwrap();
    let list = temp.path().join("next.list");
    std::fs::write(
        &list,
        "# next generation\n# GROMACS-2025.0-foss-2025b.eb\n# OpenMPI-5.0.10-GCC-15.2.0.eb --hooks=/sw/h.py\n# CP2K-2026.1-foss-2025b.eb\n",
    )
    .unwrap();
    let cli_out = temp.path().join("cli.list");
    let mcp_out = temp.path().join("mcp.list");
    let stdout = cli_stdout(&[
        "stack",
        "sections",
        list.to_str().unwrap(),
        "--from",
        previous.to_str().unwrap(),
        "--out",
        cli_out.to_str().unwrap(),
    ]);
    let body = mcp_tool(
        151,
        "eb_stack_sections",
        json!({"list": list, "from": previous, "out": mcp_out}),
    );
    assert_eq!(
        swap_paths(body["text"].as_str().unwrap_or(""), &mcp_out, "OUT"),
        swap_paths(&stdout, &cli_out, "OUT")
    );
    assert_eq!(body["sections"], 3);
    assert_eq!(body["of"], 3);
    let grouped = std::fs::read(&cli_out).unwrap();
    assert_eq!(grouped, std::fs::read(&mcp_out).unwrap());
    assert_eq!(
        String::from_utf8(grouped).unwrap(),
        "# next generation\n\n#### MPI ####\n# OpenMPI-5.0.10-GCC-15.2.0.eb --hooks=/sw/h.py\n\n#### Chemistry ####\n# GROMACS-2025.0-foss-2025b.eb\n\n#### New in this generation ####\n# CP2K-2026.1-foss-2025b.eb\n"
    );

    let cli_later = temp.path().join("cli-later.list");
    let mcp_later = temp.path().join("mcp-later.list");
    let later = cli_stdout(&[
        "stack",
        "sections",
        list.to_str().unwrap(),
        "--from",
        previous.to_str().unwrap(),
        "--unfiled",
        "Later",
        "--out",
        cli_later.to_str().unwrap(),
    ]);
    let body = mcp_tool(
        152,
        "eb_stack_sections",
        json!({
            "list": list,
            "from": previous,
            "out": mcp_later,
            "unfiled": "Later",
        }),
    );
    assert_eq!(
        swap_paths(body["text"].as_str().unwrap_or(""), &mcp_later, "OUT"),
        swap_paths(&later, &cli_later, "OUT")
    );
    assert_eq!(
        std::fs::read(&cli_later).unwrap(),
        std::fs::read(&mcp_later).unwrap()
    );
    assert!(std::fs::read_to_string(&mcp_later)
        .unwrap()
        .contains("#### Later ####"));
}

#[test]
fn mcp_repro_summary_matches_the_cli() {
    use eb_stack::{write_case_score, ReproCaseScore, ReproRatchet, ReproScore};
    assert_schema(
        "eb_repro_summary",
        &["scores"],
        &[
            ("scores", "string"),
            ("ratchet", "string"),
            ("json", "boolean"),
        ],
    );
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let ratchet_path = root.join("tests/repro_ratchet.json");
    let ratchet = ReproRatchet::read(&ratchet_path).expect("ratchet");
    let scores = tempfile::tempdir().expect("scores");
    for (case, count) in &ratchet.cases {
        let allowance = (0..*count).map(|index| format!("line {index}")).collect();
        write_case_score(
            scores.path(),
            &ReproCaseScore {
                case: case.clone(),
                pull_request: None,
                source: "source.eb".into(),
                target: "target.eb".into(),
                score: ReproScore::Exact,
                allowance,
                stale_allowance: Vec::new(),
                residual_lines: 0,
            },
        )
        .expect("seed score");
    }
    let scores_arg = scores.path().to_str().unwrap();
    let ratchet_arg = ratchet_path.to_str().unwrap();
    let table = cli_stdout(&["repro", "summary", "--scores", scores_arg]);
    let body = mcp_tool(161, "eb_repro_summary", json!({"scores": scores_arg}));
    assert_eq!(body["text"], table);

    let held = cli_stdout(&[
        "repro",
        "summary",
        "--scores",
        scores_arg,
        "--ratchet",
        ratchet_arg,
    ]);
    let body = mcp_tool(
        162,
        "eb_repro_summary",
        json!({"scores": scores_arg, "ratchet": ratchet_arg}),
    );
    assert_eq!(body["text"], held);
    assert!(held.contains("ratchet:"));
    assert!(held.contains("cases hold"));

    let as_json = cli_stdout(&["repro", "summary", "--scores", scores_arg, "--json"]);
    let body = mcp_tool(
        163,
        "eb_repro_summary",
        json!({"scores": scores_arg, "json": true}),
    );
    assert_eq!(body["text"], as_json);

    let pair = tempfile::tempdir().expect("pair");
    let seed = |case: &str, allowance: Vec<String>, score: ReproScore| {
        write_case_score(
            pair.path(),
            &ReproCaseScore {
                case: case.into(),
                pull_request: None,
                source: "source.eb".into(),
                target: "target.eb".into(),
                score,
                allowance,
                stale_allowance: Vec::new(),
                residual_lines: 0,
            },
        )
        .expect("seed");
    };
    seed(
        "reproduces_gromacs",
        vec!["    ('pybind11', '2.12.0'),".into()],
        ReproScore::Exact,
    );
    seed("reproduces_scafacos", Vec::new(), ReproScore::Semantic);
    let ratchet_dir = tempfile::tempdir().expect("ratchet");
    let low = ratchet_dir.path().join("low.json");
    std::fs::write(
        &low,
        r#"{"note":"test","cases":{"reproduces_gromacs":0,"reproduces_scafacos":0},"total":0}"#,
    )
    .unwrap();
    let failed = eb_output(&[
        "repro",
        "summary",
        "--scores",
        pair.path().to_str().unwrap(),
        "--ratchet",
        low.to_str().unwrap(),
    ]);
    assert_eq!(failed.status.code(), Some(1));
    let cli_out = String::from_utf8(failed.stdout).unwrap();
    let cli_err = String::from_utf8(failed.stderr).unwrap();
    let mcp = mcp_error_text(
        164,
        "eb_repro_summary",
        json!({"scores": pair.path(), "ratchet": low}),
    );
    assert!(
        cli_out.contains("| reproduces_gromacs | - | EXACT | 1 | 0 |"),
        "{cli_out}"
    );
    for text in [&cli_err, &mcp] {
        assert!(
            text.contains("allowance grew to 1, ratchet records 0"),
            "{text}"
        );
        assert!(text.contains("1 ratchet violation"), "{text}");
        assert!(text.contains("ratchet:"), "{text}");
    }
}

#[test]
fn every_cli_subcommand_has_an_mcp_tool() {
    let mapped = [
        ("eb_package_inspect", &["package", "inspect"][..]),
        ("eb_package_plan", &["package", "plan"]),
        ("eb_package_bump", &["package", "bump"]),
        ("eb_recipe_easyblock", &["recipe", "easyblock"]),
        ("eb_recipe_templates", &["recipe", "templates"]),
        ("eb_recipe_hierarchy", &["recipe", "hierarchy"]),
        ("eb_recipe_check", &["recipe", "check"]),
        ("eb_recipe_lint", &["recipe", "lint"]),
        ("eb_recipe_format", &["recipe", "format"]),
        ("eb_stack_solve", &["stack", "solve"]),
        ("eb_stack_sbom", &["stack", "sbom"]),
        ("eb_stack_order", &["stack", "order"]),
        ("eb_stack_easystack", &["stack", "easystack"]),
        ("eb_stack_sections", &["stack", "sections"]),
        ("eb_stack_from_commit", &["stack", "from-commit"]),
        ("eb_stack_gap", &["stack", "gap"]),
        ("eb_stack_batch", &["stack", "batch"]),
        ("eb_stack_arch", &["stack", "arch"]),
        ("eb_stack_files", &["stack", "files"]),
        ("eb_package_latest", &["package", "latest"]),
        ("eb_target_list", &["target", "list"]),
        ("eb_target_doctor", &["target", "doctor"]),
        ("eb_campaign_run", &["campaign", "run"]),
        ("eb_campaign_status", &["campaign", "status"]),
        (
            "eb_campaign_finding_claim",
            &["campaign", "finding", "claim"],
        ),
        (
            "eb_campaign_finding_resolve",
            &["campaign", "finding", "resolve"],
        ),
        ("eb_repro_summary", &["repro", "summary"]),
        ("eb_pr_overlap", &["pr-overlap"]),
        ("eb_rebuild_set", &["rebuild-set"]),
        ("eb_component_reach", &["component-reach"]),
        ("eb_easyblock_reach", &["easyblock-reach"]),
    ];
    let listed = handle_message(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
        "params": {}
    }))
    .expect("tools/list");
    let names = listed["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect::<Vec<_>>();
    let leaves = cli_leaves();
    // `mcp` is the server that publishes this catalog.
    assert!(
        leaves.iter().any(|path| path.as_slice() == ["mcp"]),
        "help did not list mcp: {leaves:?}"
    );
    for (tool, path) in mapped {
        let rendered = path.join(" ");
        assert!(
            leaves.iter().any(|leaf| {
                leaf.len() == path.len() && leaf.iter().zip(path.iter()).all(|(a, b)| a == b)
            }),
            "mapped path {rendered} is not a CLI subcommand"
        );
        let derived = format!("eb_{}", path.join("_").replace('-', "_"));
        assert_eq!(tool, derived, "{rendered}");
        assert!(
            names.contains(&tool),
            "CLI subcommand {rendered} has no MCP tool"
        );
    }
    for leaf in &leaves {
        if leaf.as_slice() == ["mcp"] {
            continue;
        }
        let rendered = leaf.join(" ");
        let listed = mapped.iter().any(|(_, path)| {
            leaf.len() == path.len() && leaf.iter().zip(path.iter()).all(|(a, b)| a == b)
        });
        assert!(listed, "CLI subcommand {rendered} has no MCP tool");
    }
}
