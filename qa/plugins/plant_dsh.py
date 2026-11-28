#!/usr/bin/env python3
"""Write a local DSH bundle and the call sequence for the mock provider."""
import json
import sys
from pathlib import Path


def plant(source, root, port, mock):
    source.mkdir(parents=True, exist_ok=True)
    package = {
        "name": "qa-dsh",
        "version": "1.0.0",
        "dsh": {"bundle": {"patch": "cordis.patch.yml"}},
        "scripts": {"postinstall": f"touch {root / 'dsh-postinstall-marker'}"},
    }
    (source / "package.json").write_text(json.dumps(package))
    node = "@deepseek-ai/dsh-mcp-client"
    def mcp(server, **fields):
        return {"id": server, "name": node, "config": {"serverName": server, **fields}}

    http_package = {**package, "name": "qa-dsh-http", "scripts": {}}
    http_source = source.with_name("qa-dsh-http")
    http_source.mkdir(parents=True, exist_ok=True)
    (http_source / "package.json").write_text(json.dumps(http_package))
    (http_source / "cordis.patch.yml").write_text(json.dumps([{"insert": [
        mcp("http", transport="streamable-http", url=f"http://127.0.0.1:{port}/mcp",
            headers={"X-Qa-Bundle": "dsh-http"}),
    ]}]))
    patch = [{"insert": [
        mcp("stdio", transport="stdio", command="python3", args=[
            str(mock), "--label", "dsh-stdio", "--traffic", str(root / "dsh-stdio.jsonl"),
        ]),
        mcp("startup", transport="stdio", command="python3", failOnStartupError=True),
    ]}]
    # JSON is a YAML subset. Add an authored tag using YAML syntax so the
    # importer sees the exact distinction between a tag and a quoted scalar.
    encoded = json.dumps(patch[0]["insert"], indent=2)
    (source / "cordis.patch.yml").write_text(
        "- insert:\n" + "".join(f"    {line}\n" for line in encoded.splitlines())
        # The separate insert keeps its location independent of literal nodes.
        + "- insert:\n"
        + f"  - {{ id: tagged, name: '{node}', config: {{ serverName: tagged, transport: stdio, "
          f"command: !!js 'touch {root / 'dsh-tag-marker'}' }} }}\n"
    )
    specs = [{"name": "plugin_qa-dsh_stdio__qa_echo", "input": {"probe": "mcp-stdio-roundtrip"}}]
    (root / "dsh-tool-specs.json").write_text(json.dumps([specs, None]))


if __name__ == "__main__":
    plant(Path(sys.argv[1]), Path(sys.argv[2]), int(sys.argv[3]), Path(sys.argv[4]))
