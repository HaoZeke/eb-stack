---
name: eb-stack-mcp
description: Serve the eb-stack commands over stdio as a Model Context Protocol server. Use when a client should call inspect, plan, bump, stack SBOM, or pr-overlap without parsing the CLI. The server reads one JSON-RPC object per line.
---

# Serve the command over stdio

Start the server from the repository root. `--workspace` is the root for the recipe file tools. The process speaks newline-delimited JSON-RPC on stdin and stdout.

```sh
eb-stack mcp --workspace .
```

A client sends `initialize`, then `notifications/initialized`, then `tools/list` or `tools/call`. This session, driven from the repository root, inspects the conda-forge eOn recipe. It is the same work as `eb-stack package inspect`.

```sh
python3 - <<'PY'
import json
import subprocess

proc = subprocess.Popen(
    ["eb-stack", "mcp", "--workspace", "."],
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    text=True,
)

def call(message):
    proc.stdin.write(json.dumps(message) + "\n")
    proc.stdin.flush()
    return json.loads(proc.stdout.readline())

init = call({
    "jsonrpc": "2.0",
    "id": 1,
    "method": "initialize",
    "params": {
        "protocolVersion": "2024-11-05",
        "capabilities": {},
        "clientInfo": {"name": "probe", "version": "0"},
    },
})
assert init["result"]["serverInfo"]["name"] == "eb-stack"
proc.stdin.write(json.dumps({
    "jsonrpc": "2.0",
    "method": "notifications/initialized",
}) + "\n")
proc.stdin.flush()
result = call({
    "jsonrpc": "2.0",
    "id": 2,
    "method": "tools/call",
    "params": {
        "name": "eb_package_inspect",
        "arguments": {
            "source": "fixtures/foreign_ingest/conda_eon/recipe.yaml",
            "format": "conda-forge",
            "toolchain": "foss/2026.1",
            "package_configs": [
                "examples/packages/common.toml",
                "examples/packages/eon.toml",
            ],
            "out_dir": "/tmp/eon-inspect-mcp",
        },
    },
})
assert result["result"]["isError"] is False
proc.stdin.close()
proc.wait(timeout=5)
PY
```

`tools/list` returns 37 tools. The names that mirror the commands above are `eb_package_inspect`, `eb_package_plan`, `eb_package_bump`, `eb_stack_sbom`, and `eb_pr_overlap`. The input schema requires `toolchain` as `name/version` for inspect, plan, and bump. Inspect and plan also accept `toolchain_version` alone and default the name to `foss`. Bump does not: it requires `toolchain`, or both `toolchain_name` and `toolchain_version`. `eb_stack_sbom` takes `lock` and optional `out`. `eb_pr_overlap` takes `pr` as an array of paths and `easyconfigs` as an array of directories.

`eb_build_test` runs only when `EB_STACK_ALLOW_BUILD=1`.

# Registry manifest and client configuration

`server.json` names this server `io.github.HaoZeke/eb-stack` for the cargo package `eb_stack`. Client files for Claude Desktop, Visual Studio Code, and Copilot are on the manual page `docs/orgmode/reference/mcp.org`. `eb-stack mcp --help` prints `--workspace` and exits 0.
