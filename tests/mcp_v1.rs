use eb_stack::mcp::handle_message;
use serde_json::json;

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
        "eb_recipe_write",
        "eb_recipe_grep",
        "eb_recipe_check",
        "eb_recipe_lint",
        "eb_recipe_format",
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
    assert_eq!(names.len(), 19, "unexpected MCP tools: {names:?}");

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
    use std::sync::Mutex;
    static ENV_LOCK: Mutex<()> = Mutex::new(());
    let _guard = ENV_LOCK.lock().expect("workspace env lock");
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
