#!/usr/bin/env python3
"""Deterministic model endpoint; Gateway, tools, storage and UI remain production."""
import argparse
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import time


def response(body):
    names = [tool.get("name") for tool in body.get("tools", [])]
    last = body.get("messages", [{}])[-1].get("content", [])
    has_result = isinstance(last, list) and any(block.get("type") == "tool_result" for block in last if isinstance(block, dict))
    publish = "artifact_publish" in names and not has_result
    content = [{"type": "text", "text": "PHASE9_RUNTIME_PROOF"}]
    if publish:
        content.append({"type": "tool_use", "id": "phase9-artifact", "name": "artifact_publish",
                        "input": {"title": "Phase9 runtime proof", "content": "Controlled runtime fixture. Actual Gateway artifact storage and notification.", "figures": []}})
    return {"id": "phase9-model-message", "type": "message", "role": "assistant", "model": body.get("model", "phase9-qa"),
            "content": content, "stop_reason": "tool_use" if publish else "end_turn", "usage": {"input_tokens": 8, "output_tokens": 8}}


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("content-length", "0"))))
        answer = response(body)
        with self.server.record.open("a") as record:
            record.write(json.dumps({"tools": [tool.get("name") for tool in body.get("tools", [])], "stop_reason": answer["stop_reason"]}) + "\n")
        if not body.get("stream"):
            data = json.dumps(answer).encode()
            self.send_response(200)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
            return
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("transfer-encoding", "chunked")
        self.end_headers()
        def event(value):
            data = f"event: {value['type']}\ndata: {json.dumps(value)}\n\n".encode()
            self.wfile.write(f"{len(data):X}\r\n".encode() + data + b"\r\n")
            self.wfile.flush()
        try:
            event({"type": "message_start", "message": answer | {"content": [], "stop_reason": None}})
            for index, block in enumerate(answer["content"]):
                start = {"type": block["type"], "text": ""} if block["type"] == "text" else block | {"input": {}}
                event({"type": "content_block_start", "index": index, "content_block": start})
                delta = {"type": "text_delta", "text": block["text"]} if block["type"] == "text" else {"type": "input_json_delta", "partial_json": json.dumps(block["input"])}
                event({"type": "content_block_delta", "index": index, "delta": delta})
                time.sleep(0.02)
                event({"type": "content_block_stop", "index": index})
            event({"type": "message_delta", "delta": {"stop_reason": answer["stop_reason"], "stop_sequence": None}, "usage": {"output_tokens": 8}})
            event({"type": "message_stop"})
            self.wfile.write(b"0\r\n\r\n")
            self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError):
            self.close_connection = True


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--record", type=Path, required=True)
    args = parser.parse_args()
    server = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    server.record = args.record
    server.serve_forever()
