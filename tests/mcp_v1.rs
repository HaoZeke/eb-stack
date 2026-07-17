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
    assert_eq!(names.len(), 14, "unexpected MCP tools: {names:?}");

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
