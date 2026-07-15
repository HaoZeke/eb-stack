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
        "eb_recipe_format",
        "eb_stack_solve",
        "eb_target_list",
        "eb_target_doctor",
        "eb_campaign_run",
        "eb_campaign_status",
        "eb_campaign_finding_claim",
        "eb_campaign_finding_resolve",
    ] {
        assert!(names.contains(&expected), "missing {expected}: {names:?}");
    }
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
