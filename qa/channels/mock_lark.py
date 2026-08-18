#!/usr/bin/env python3
"""Stand-in for the Feishu/Lark Open Platform, for real-machine channel QA.

`FeishuConfig.domain` accepts an arbitrary URL (`base_url()`'s `custom` arm),
so the channel can be pointed here and `start()` runs its *real* code path:
fetch an app access token, fetch bot info, latch the bot's open_id, spawn the
refresher, bring up the webhook server. Nothing about the channel is stubbed —
only the far end of the socket is.

That matters because the alternative evidence for "feishu is wired" is a boot
line saying a channel was constructed, and construction is exactly the half
that was never broken. What had never run was `start()`.

Every request is appended to an observations file, one JSON object per line,
so the fixture can assert on what the channel actually *sent* rather than on
what its logs say it did.

Usage:  mock_lark.py <port> <observations-path>
"""
import json
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT = int(sys.argv[1])
OBS_PATH = sys.argv[2]

_lock = threading.Lock()


def record(method, path, body):
    with _lock, open(OBS_PATH, "a") as fh:
        fh.write(json.dumps({"method": method, "path": path, "body": body}) + "\n")


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_a):  # keep the fixture's stdout readable
        pass

    def _reply(self, payload, status=200):
        raw = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def _read_body(self):
        n = int(self.headers.get("Content-Length") or 0)
        if not n:
            return None
        raw = self.rfile.read(n)
        try:
            return json.loads(raw)
        except Exception:
            return {"_raw": raw.decode("utf-8", "replace")}

    def do_GET(self):
        path = self.path.split("?", 1)[0]
        if path == "/__observations":
            try:
                with open(OBS_PATH) as fh:
                    body = fh.read()
            except FileNotFoundError:
                body = ""
            raw = body.encode()
            self.send_response(200)
            self.send_header("Content-Type", "text/plain")
            self.send_header("Content-Length", str(len(raw)))
            self.end_headers()
            self.wfile.write(raw)
            return
        record("GET", path, None)
        if path == "/open-apis/bot/v3/info":
            # `BotInfo` only reads app_name and open_id; open_id is latched into
            # the channel and used to decide whether a group mention is at us.
            self._reply({
                "code": 0,
                "msg": "ok",
                "bot": {"app_name": "Aleph QA Bot", "open_id": "ou_qa_bot"},
            })
            return
        self._reply({"code": 0, "msg": "ok", "data": {}})

    def do_POST(self):
        path = self.path.split("?", 1)[0]
        body = self._read_body()
        record("POST", path, body)
        if path == "/open-apis/auth/v3/app_access_token/internal":
            # `TokenManager::refresh_token` reads code / app_access_token /
            # expire. A short expiry would make the refresher fire mid-run and
            # add noise; 7200 is what the real API returns.
            self._reply({
                "code": 0,
                "msg": "ok",
                "app_access_token": "mock-app-access-token",
                "expire": 7200,
            })
            return
        if path == "/open-apis/cardkit/v1/cards":
            # `create_streaming_card` reads data.card_id. Answering it is what
            # lets the streaming emitter get past its first call — and a
            # request landing here at all is the only observable that
            # `try_create_feishu_emitter` returned `Some`.
            self._reply({
                "code": 0,
                "msg": "ok",
                "data": {"card_id": "card_qa_1"},
            })
            return
        if path == "/open-apis/im/v1/messages":
            self._reply({
                "code": 0,
                "msg": "ok",
                "data": {"message_id": "om_qa_reply_1"},
            })
            return
        self._reply({"code": 0, "msg": "ok", "data": {}})


if __name__ == "__main__":
    open(OBS_PATH, "w").close()
    srv = ThreadingHTTPServer(("127.0.0.1", PORT), Handler)
    print(f"mock lark on 127.0.0.1:{PORT}, observations -> {OBS_PATH}", flush=True)
    srv.serve_forever()
