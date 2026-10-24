//! The MCP server over a real stdio JSON-RPC client.
//!
//! `eb_stack_sbom` has to write the same CycloneDX document as `stack sbom`,
//! including checksums and source URLs. The overlap and reach tools have to
//! write the same report files as the CLI, on the repository fixtures.

use eb_stack::pr_overlap::{EasystackChange, FileChange, PrInput};
use serde_json::json;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};

struct McpClient {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<std::process::ChildStdout>,
    next_id: i64,
}

impl McpClient {
    fn spawn(workspace: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
            .args(["mcp", "--workspace"])
            .arg(workspace)
            .env_remove("EASYBUILD_FILTER_DEPS")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn eb-stack mcp");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        Self {
            child,
            stdin: Some(stdin),
            stdout,
            next_id: 1,
        }
    }

    fn send(&mut self, message: &serde_json::Value) {
        let stdin = self.stdin.as_mut().expect("stdin open");
        writeln!(stdin, "{message}").expect("write json-rpc");
        stdin.flush().expect("flush json-rpc");
    }

    fn request(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }));
        let mut line = String::new();
        self.stdout
            .read_line(&mut line)
            .expect("read json-rpc response");
        serde_json::from_str(&line).unwrap_or_else(|error| {
            panic!("response is not JSON ({error}): {line}");
        })
    }

    fn handshake(&mut self) {
        let response = self.request(
            "initialize",
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "eb-stack-stdio", "version": "0"}
            }),
        );
        assert_eq!(response["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(response["result"]["serverInfo"]["name"], "eb-stack");
        self.send(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }));
    }

    fn call(&mut self, name: &str, arguments: serde_json::Value) -> serde_json::Value {
        self.request("tools/call", json!({"name": name, "arguments": arguments}))
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        drop(self.stdin.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn files_under(dir: &Path) -> BTreeMap<String, String> {
    let mut files = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in std::fs::read_dir(&current).expect("read out") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let rel = path.strip_prefix(dir).unwrap().display().to_string();
                files.insert(rel, std::fs::read_to_string(&path).expect("read file"));
            }
        }
    }
    files
}

fn cli(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args(args)
        .env_remove("EASYBUILD_FILTER_DEPS")
        .output()
        .expect("cli")
}

fn counts(line: &str) -> &str {
    line.rsplit_once(" out=")
        .map(|(head, _)| head)
        .unwrap_or(line)
}

fn assert_same_summary(output: &std::process::Output, response: &serde_json::Value) {
    assert!(
        output.status.success(),
        "cli stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(response["result"]["isError"], false, "{response}");
    let summary = response["result"]["structuredContent"]["summary"]
        .as_str()
        .expect("summary");
    let cli_line = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        counts(summary),
        counts(cli_line.trim()),
        "summary {summary} vs {}",
        cli_line.trim()
    );
}

fn recipe(name: &str, version: &str, checksum: &str, deps: &str) -> String {
    format!(
        "name = '{name}'\n\
         version = '{version}'\n\
         homepage = 'https://example.invalid/{name}'\n\
         description = '{name} fixture'\n\
         toolchain = {{'name': 'GCCcore', 'version': '13.3.0'}}\n\
         source_urls = ['https://example.invalid/{name}/']\n\
         sources = ['{name}-{version}.tar.gz']\n\
         checksums = ['{checksum}']\n\
         {deps}\n"
    )
}

fn without_serial(document: &mut serde_json::Value) {
    if let Some(object) = document.as_object_mut() {
        object.remove("serialNumber");
    }
}

#[test]
fn mcp_stdio_stack_sbom_matches_the_cli() {
    let temp = tempfile::tempdir().expect("tempdir");
    let zlib_eb = temp.path().join("zlib-1.3.1-GCCcore-13.3.0.eb");
    let libpng_eb = temp.path().join("libpng-1.6.43-GCCcore-13.3.0.eb");
    let sha_zlib = "a".repeat(64);
    let sha_libpng = "b".repeat(64);
    std::fs::write(
        &zlib_eb,
        recipe(
            "zlib",
            "1.3.1",
            &sha_zlib,
            "dependencies = [('libpng', '1.6.43')]",
        ),
    )
    .expect("zlib recipe");
    std::fs::write(
        &libpng_eb,
        recipe(
            "libpng",
            "1.6.43",
            &sha_libpng,
            "builddependencies = [('zlib', '1.3.1')]",
        ),
    )
    .expect("libpng recipe");
    let lock = temp.path().join("stack.lock.json");
    std::fs::write(
        &lock,
        format!(
            r#"{{
  "schema_version": 1,
  "toolchain": {{"name": "GCCcore", "version": "13.3.0"}},
  "packages": [
    {{
      "name": "libpng",
      "version": "1.6.43",
      "toolchain": {{"name": "GCCcore", "version": "13.3.0"}},
      "easyconfig_path": "{libpng}"
    }},
    {{
      "name": "zlib",
      "version": "1.3.1",
      "toolchain": {{"name": "GCCcore", "version": "13.3.0"}},
      "easyconfig_path": "{zlib}"
    }}
  ],
  "solver": {{
    "engine": "resolvo",
    "engine_version": "0",
    "timestamp": "2026-01-01T00:00:00Z"
  }}
}}"#,
            libpng = libpng_eb.display(),
            zlib = zlib_eb.display(),
        ),
    )
    .expect("lock");

    let cli_out = temp.path().join("cli.cdx.json");
    let output = cli(&[
        "stack",
        "sbom",
        "--lock",
        lock.to_str().unwrap(),
        "--out",
        cli_out.to_str().unwrap(),
    ]);
    assert!(
        output.status.success(),
        "cli: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let cli_text = String::from_utf8(output.stdout).expect("cli stdout");
    assert!(cli_text.contains("components=2"), "{cli_text}");
    assert!(
        cli_text.contains("with_artifact_facts=2"),
        "both recipes contribute hashes and source URLs: {cli_text}"
    );

    let mut client = McpClient::spawn(temp.path());
    client.handshake();
    let mcp_out = temp.path().join("mcp.cdx.json");
    let response = client.call("eb_stack_sbom", json!({"lock": lock, "out": mcp_out}));
    assert_eq!(response["result"]["isError"], false, "{response}");
    assert_eq!(
        response["result"]["structuredContent"]["components"], 2,
        "{response}"
    );

    let mut cli_doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cli_out).expect("cli sbom"))
            .expect("cli json");
    let mut mcp_doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&mcp_out).expect("mcp sbom"))
            .expect("mcp json");
    without_serial(&mut cli_doc);
    without_serial(&mut mcp_doc);
    assert_eq!(
        mcp_doc, cli_doc,
        "MCP stack sbom diverges from the CLI document"
    );

    let zlib = mcp_doc["components"]
        .as_array()
        .expect("components")
        .iter()
        .find(|component| component["name"] == "zlib")
        .expect("zlib component");
    assert_eq!(zlib["hashes"][0]["alg"], "SHA-256");
    assert_eq!(zlib["hashes"][0]["content"], sha_zlib);
    assert!(
        zlib["externalReferences"][0]["url"]
            .as_str()
            .unwrap_or("")
            .contains("example.invalid/zlib"),
        "source URL missing: {zlib}"
    );
    let edges: Vec<&str> = mcp_doc["dependencies"]
        .as_array()
        .expect("dependencies")
        .iter()
        .find(|edge| {
            edge["ref"]
                .as_str()
                .unwrap_or("")
                .starts_with("pkg:generic/zlib@")
        })
        .and_then(|edge| edge["dependsOn"].as_array())
        .map(|depends| depends.iter().filter_map(|item| item.as_str()).collect())
        .unwrap_or_default();
    assert!(
        edges
            .iter()
            .any(|edge| edge.starts_with("pkg:generic/libpng@")),
        "zlib dependsOn libpng: {edges:?}"
    );
}

#[test]
fn mcp_stdio_catalog_lists_the_overlap_and_reach_tools() {
    let mut client = McpClient::spawn(&root());
    client.handshake();
    let listed = client.request("tools/list", json!({}));
    let names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    for expected in [
        "eb_pr_overlap",
        "eb_rebuild_set",
        "eb_component_reach",
        "eb_easyblock_reach",
        "eb_stack_sbom",
        "eb_stack_order",
        "eb_stack_easystack",
        "eb_stack_sections",
        "eb_recipe_easyblock",
        "eb_recipe_templates",
        "eb_recipe_hierarchy",
        "eb_repro_summary",
    ] {
        assert!(names.contains(&expected), "missing {expected}: {names:?}");
    }
    assert_eq!(names.len(), 37, "{names:?}");
}

fn software_layer_prs() -> Vec<PrInput> {
    const STACK: &str = "easystacks/software.eessi.io/2026.06/eessi-2026.06-eb-5.4.0-2026.1.yml";
    const BASE: &str = "easyconfigs:\n  - zlib-1.3.1-GCCcore-15.2.0.eb\n";
    let change = |path: &str, added: &str| EasystackChange {
        path: path.into(),
        base: Some(BASE.into()),
        head: Some(format!("{BASE}{added}")),
        base_blob: None,
        head_blob: None,
    };
    vec![
        PrInput {
            number: 101,
            head_sha: format!("{:0>40}", 101),
            title: Some("PR 101".into()),
            easystacks: vec![change(STACK, "  - Graphviz-14.0.2-GCCcore-15.2.0.eb\n")],
            files: Vec::new(),
            easyconfigs: Vec::new(),
        },
        PrInput {
            number: 102,
            head_sha: format!("{:0>40}", 102),
            title: Some("PR 102".into()),
            easystacks: vec![change(STACK, "  - gnuplot-6.0.3-GCCcore-15.2.0.eb\n")],
            files: Vec::new(),
            easyconfigs: Vec::new(),
        },
    ]
}

#[test]
fn mcp_stdio_pr_overlap_software_layer_matches_the_cli() {
    let fixtures = root().join("tests/pr_overlap_fixtures");
    let temp = tempfile::tempdir().expect("tempdir");
    let input = temp.path().join("prs.json");
    std::fs::write(
        &input,
        serde_json::to_string(&software_layer_prs()).expect("json"),
    )
    .expect("write prs");
    let cli_out = temp.path().join("cli");
    let mcp_out = temp.path().join("mcp");
    let output = cli(&[
        "pr-overlap",
        "--pr",
        input.to_str().unwrap(),
        "--easyconfigs",
        fixtures.join("release").to_str().unwrap(),
        "--installed",
        fixtures.join("installed.txt").to_str().unwrap(),
        "--installed-label",
        "x86_64/amd/zen4",
        "--exclude-path",
        "/accel/",
        "--generated-at",
        "2026-10-19T12:00:00Z",
        "--out-dir",
        cli_out.to_str().unwrap(),
    ]);
    let mut client = McpClient::spawn(temp.path());
    client.handshake();
    let response = client.call(
        "eb_pr_overlap",
        json!({
            "pr": [input],
            "kind": "software-layer",
            "easyconfigs": [fixtures.join("release")],
            "installed": fixtures.join("installed.txt"),
            "installed_label": "x86_64/amd/zen4",
            "exclude_path": ["/accel/"],
            "generated_at": "2026-10-19T12:00:00Z",
            "out_dir": mcp_out,
        }),
    );
    assert_same_summary(&output, &response);
    assert_eq!(files_under(&cli_out), files_under(&mcp_out));
    assert!(mcp_out.join("prs/101.toml").is_file());
    assert!(mcp_out.join("overlap.md").is_file());
}

#[test]
fn mcp_stdio_pr_overlap_easyconfigs_matches_the_cli() {
    let fixtures = root().join("tests/pr_overlap_easyconfigs_fixtures");
    let lib = std::fs::read_to_string(fixtures.join("l/Lib/Lib-1.0.eb")).unwrap();
    let prs = vec![PrInput {
        number: 1,
        head_sha: "1".repeat(40),
        title: Some("edit Lib".into()),
        easystacks: Vec::new(),
        files: Vec::new(),
        easyconfigs: vec![FileChange {
            path: "l/Lib/Lib-1.0.eb".into(),
            base: Some(lib.clone()),
            head: Some(lib.replace("Fixture.", "Head.")),
            base_blob: None,
            head_blob: None,
        }],
    }];
    let temp = tempfile::tempdir().expect("tempdir");
    let input = temp.path().join("prs.json");
    std::fs::write(&input, serde_json::to_string(&prs).unwrap()).unwrap();
    let cli_out = temp.path().join("cli");
    let mcp_out = temp.path().join("mcp");
    let output = cli(&[
        "pr-overlap",
        "--kind",
        "easyconfigs",
        "--pr",
        input.to_str().unwrap(),
        "--easyconfigs",
        fixtures.to_str().unwrap(),
        "--generated-at",
        "2026-10-22T00:00:00Z",
        "--repo-url",
        "https://github.com/easybuilders/easybuild-easyconfigs",
        "--out-dir",
        cli_out.to_str().unwrap(),
    ]);
    let mut client = McpClient::spawn(temp.path());
    client.handshake();
    let response = client.call(
        "eb_pr_overlap",
        json!({
            "pr": [input],
            "kind": "easyconfigs",
            "easyconfigs": [fixtures],
            "generated_at": "2026-10-22T00:00:00Z",
            "repo_url": "https://github.com/easybuilders/easybuild-easyconfigs",
            "out_dir": mcp_out,
        }),
    );
    assert_same_summary(&output, &response);
    assert_eq!(files_under(&cli_out), files_under(&mcp_out));
}

#[test]
fn mcp_stdio_rebuild_set_matches_the_cli() {
    let fixtures = root().join("tests/rebuild_set_fixtures");
    let temp = tempfile::tempdir().expect("tempdir");
    let cli_out = temp.path().join("cli");
    let mcp_out = temp.path().join("mcp");
    let changed = fixtures.join("changed.txt");
    let installed = fixtures.join("installed.txt");
    let release = fixtures.join("release");
    let stack = fixtures.join("eessi-2026.06-eb-5.4.0-2026.1.yml");
    let output = cli(&[
        "rebuild-set",
        "--easyconfigs",
        release.to_str().unwrap(),
        "--installed",
        installed.to_str().unwrap(),
        "--installed-label",
        "x86_64/amd/zen4",
        "--generated-at",
        "2026-10-22T12:00:00Z",
        "--changed",
        changed.to_str().unwrap(),
        "--easystack",
        stack.to_str().unwrap(),
        "--out-dir",
        cli_out.to_str().unwrap(),
    ]);
    let mut client = McpClient::spawn(temp.path());
    client.handshake();
    let response = client.call(
        "eb_rebuild_set",
        json!({
            "easyconfigs": [release],
            "installed": installed,
            "installed_label": "x86_64/amd/zen4",
            "generated_at": "2026-10-22T12:00:00Z",
            "changed": [changed],
            "easystack": [stack],
            "out_dir": mcp_out,
        }),
    );
    assert_same_summary(&output, &response);
    assert_eq!(files_under(&cli_out), files_under(&mcp_out));
    assert!(mcp_out.join("rebuild-set.json").is_file());
}

#[test]
fn mcp_stdio_component_reach_matches_the_cli() {
    let fixtures = root().join("tests/rebuild_set_fixtures");
    let temp = tempfile::tempdir().expect("tempdir");
    let cli_out = temp.path().join("cli");
    let mcp_out = temp.path().join("mcp");
    let release = fixtures.join("release");
    let stack = fixtures.join("eessi-2026.06-eb-5.4.0-2026.1.yml");
    let output = cli(&[
        "component-reach",
        "--component",
        "zlib",
        "--version",
        "1.3.1",
        "--generation",
        "2026.1",
        "--easyconfigs",
        release.to_str().unwrap(),
        "--easystack",
        stack.to_str().unwrap(),
        "--generated-at",
        "2026-10-22T12:00:00Z",
        "--out-dir",
        cli_out.to_str().unwrap(),
    ]);
    let mut client = McpClient::spawn(temp.path());
    client.handshake();
    let response = client.call(
        "eb_component_reach",
        json!({
            "component": "zlib",
            "version": "1.3.1",
            "generation": "2026.1",
            "easyconfigs": [release],
            "easystack": [stack],
            "generated_at": "2026-10-22T12:00:00Z",
            "out_dir": mcp_out,
        }),
    );
    assert_same_summary(&output, &response);
    assert_eq!(files_under(&cli_out), files_under(&mcp_out));
}

#[test]
fn mcp_stdio_easyblock_reach_matches_the_cli() {
    let temp = tempfile::tempdir().expect("tempdir");
    let py = temp.path().join("py");
    let ecs = temp.path().join("eb");
    let write = |root: &Path, rel: &str, text: &str| {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    write(
        &py,
        "easybuild/easyblocks/generic/configuremake.py",
        "class ConfigureMake:\n    pass\n",
    );
    write(
        &py,
        "easybuild/easyblocks/generic/cmakemake.py",
        "class CMakeMake(ConfigureMake):\n    pass\n",
    );
    write(
        &ecs,
        "zlib.eb",
        "name = 'zlib'\nversion = '1.3'\ntoolchain = SYSTEM\neasyblock = 'CMakeMake'\n",
    );
    let changed = py.join("easybuild/easyblocks/generic/cmakemake.py");
    let cli_out = temp.path().join("cli");
    let mcp_out = temp.path().join("mcp");
    let output = cli(&[
        "easyblock-reach",
        "--easyblocks",
        py.to_str().unwrap(),
        "--easyconfigs",
        ecs.to_str().unwrap(),
        "--file",
        changed.to_str().unwrap(),
        "--generated-at",
        "2026-10-22T00:00:00Z",
        "--out-dir",
        cli_out.to_str().unwrap(),
    ]);
    let mut client = McpClient::spawn(temp.path());
    client.handshake();
    let response = client.call(
        "eb_easyblock_reach",
        json!({
            "easyblocks": [py],
            "easyconfigs": [ecs],
            "file": [changed],
            "generated_at": "2026-10-22T00:00:00Z",
            "out_dir": mcp_out,
        }),
    );
    assert_same_summary(&output, &response);
    assert_eq!(files_under(&cli_out), files_under(&mcp_out));
    assert!(mcp_out.join("reach.json").is_file());
}
#[test]
fn mcp_stdio_campaign_run_failure_is_a_tool_error() {
    let temp = tempfile::tempdir().expect("tempdir");
    let bundle = temp.path().join("bundle");
    let recipes = bundle.join("easyconfigs/e/eOn");
    std::fs::create_dir_all(&recipes).unwrap();
    std::fs::create_dir_all(bundle.join("locks")).unwrap();
    std::fs::write(
        bundle.join("package.plan.json"),
        r#"{"package":{"name":"eOn","version":"2.16.0"}}"#,
    )
    .unwrap();
    std::fs::write(
        bundle.join("locks/default.lock.json"),
        r#"{"profile":"default","solver":"resolvo"}"#,
    )
    .unwrap();
    std::fs::write(
        recipes.join("eOn-2.16.0-foss-2026.1.eb"),
        "name = 'eOn'\nversion = '2.16.0'\ntoolchain = SYSTEM\nsources = ['source.tar.gz']\nchecksums = ['deadbeef']\nmoduleclass = 'tools'\n",
    )
    .unwrap();
    let config = temp.path().join("targets.toml");
    std::fs::write(
        &config,
        r#"
schema_version = 1
[[targets]]
name = "test-builder"
[targets.transport]
kind = "local"
[targets.executor]
kind = "direct"
[targets.runtime]
kind = "host"
[targets.easybuild]
command = "false"
robot_paths = ["/tmp"]
work_root = "/tmp"
tmp_root = "/tmp"
"#,
    )
    .unwrap();
    let cli_state = temp.path().join("cli-campaign.json");
    let mcp_state = temp.path().join("mcp-campaign.json");
    let output = cli(&[
        "campaign",
        "run",
        "--bundle",
        bundle.to_str().unwrap(),
        "--config",
        config.to_str().unwrap(),
        "--target",
        "test-builder",
        "--state",
        cli_state.to_str().unwrap(),
    ]);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("campaign build failed with typed findings in its state file"),
        "{stderr}"
    );
    let cli_body: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("cli state json");

    let mut client = McpClient::spawn(temp.path());
    client.handshake();
    let response = client.call(
        "eb_campaign_run",
        json!({
            "bundle": bundle,
            "configs": [config],
            "target": "test-builder",
            "state": mcp_state,
        }),
    );
    assert_eq!(response["result"]["isError"], true, "{response}");
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .expect("error text");
    let (message, body) = text.split_once('\n').expect("message and state");
    assert_eq!(
        message,
        "campaign build failed with typed findings in its state file"
    );
    let mcp_body: serde_json::Value = serde_json::from_str(body).expect("mcp state");
    assert_eq!(mcp_body["status"], cli_body["status"]);
    assert_eq!(mcp_body["claims"], cli_body["claims"]);
    assert_eq!(
        mcp_body["findings"][0]["class"],
        cli_body["findings"][0]["class"]
    );
    assert_eq!(mcp_body["status"], "failed");
    let on_disk: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&mcp_state).unwrap()).unwrap();
    assert_eq!(on_disk["status"], "failed");
}
