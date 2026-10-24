//! The MCP server over a real stdio JSON-RPC client.
//!
//! `eb_stack_sbom` has to write the same CycloneDX document as `stack sbom`,
//! including checksums and source URLs.

use serde_json::json;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
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

fn cli(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args(args)
        .env_remove("EASYBUILD_FILTER_DEPS")
        .output()
        .expect("cli")
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
