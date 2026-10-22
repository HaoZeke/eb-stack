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
        "eb_target_list",
        "eb_target_doctor",
        "eb_campaign_run",
        "eb_campaign_status",
        "eb_campaign_finding_claim",
        "eb_campaign_finding_resolve",
    ] {
        assert!(names.contains(&expected), "missing {expected}: {names:?}");
    }
    assert_eq!(names.len(), 26, "unexpected MCP tools: {names:?}");

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
                "toolchain_name": "foss",
                "toolchain_version": "2026.1",
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
                "toolchain_name": "foss",
                "toolchain_version": "2026.1",
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
            "arguments": {"toolchain_version": "2026.1", "source": "x.eb", "easyconfigs": ["."], "out_dir": "/tmp"}
        }
    }))
    .expect("bump");
    assert_eq!(bump["result"]["isError"], true, "{bump}");
    assert!(
        bump["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .contains("toolchain_name"),
        "{bump}"
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
