#!/usr/bin/env python3
"""Local MCP fixture for stdio and Streamable HTTP."""

import argparse
import json
import os
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path


def answer(request, label):
    if "id" not in request:
        return None
    response = {"jsonrpc": "2.0", "id": request["id"]}
    method = request.get("method", "")
    if method == "initialize":
        response["result"] = {
            "protocolVersion": "2025-03-26",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "qa-mock", "version": "0"},
        }
    elif method == "tools/list":
        response["result"] = {"tools": [{
            "name": "qa_echo",
            "description": "echoes its input",
            "inputSchema": {"type": "object", "properties": {}},
        }]}
    elif method == "tools/call" and request.get("params", {}).get("name") == "qa_echo":
        response["result"] = {"content": [{"type": "text", "text": json.dumps({
            "label": label,
            "arguments": request["params"].get("arguments", {}),
        }, sort_keys=True)}]}
    elif method == "ping":
        response["result"] = {}
    else:
        response["error"] = {"code": -32601, "message": f"method not found: {method}"}
    return response


def record(path, request, header=None):
    if path:
        with path.open("a") as stream:
            stream.write(json.dumps({"method": request.get("method"), "header": header}) + "\n")


def stdio(label, traffic):
    for line in sys.stdin:
        try:
            request = json.loads(line)
        except json.JSONDecodeError:
            continue
        record(traffic, request)
        response = answer(request, label)
        if response is not None:
            print(json.dumps(response), flush=True)


def http(port, label, traffic, ready):
    class Handler(BaseHTTPRequestHandler):
        def do_POST(self):
            if self.path != "/mcp":
                self.send_error(404)
                return
            request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            record(traffic, request, self.headers.get("X-Qa-Bundle"))
            response = answer(request, label)
            if response is None:
                self.send_response(202)
                self.end_headers()
                return
            body = json.dumps(response).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):
            self.send_error(405)

        def do_DELETE(self):
            self.send_response(204)
            self.end_headers()

    with ThreadingHTTPServer(("127.0.0.1", port), Handler) as server:
        if ready:
            ready.write_text(str(server.server_port))
        server.serve_forever()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--http", type=int)
    parser.add_argument("--label", default=os.environ.get("QA_MCP_LABEL", "qa-mock"))
    parser.add_argument("--traffic", type=Path)
    parser.add_argument("--ready", type=Path)
    args = parser.parse_args()
    if args.http is None:
        stdio(args.label, args.traffic)
    else:
        http(args.http, args.label, args.traffic, args.ready)


if __name__ == "__main__":
    main()
