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

# Error injection

`POST /__inject` queues canned failures for a path, so the fixture can drive
the paths a happy-path mock structurally cannot reach: a throttle, a refusal, a
gateway page where JSON was expected. Those were the last uncovered half of
this channel — the note in `feishu_inbound/crypto.rs` says so in as many words
("what this still does not cover, and cannot without an app credential: how the
client behaves against a live 429 or a live permission error") — and they are
exactly the paths where the client used to be wrong: a throttle that Lark
reports on its legacy 400 shape never became `FeishuSendError::RateLimited`, so
`ChannelRegistry` never retried it and the reply was dropped in silence.

    POST /__inject {"path": "/open-apis/im/v1/messages", "times": 2,
                    "status": 400, "headers": {"x-ogw-ratelimit-reset": "1"},
                    "body": {"code": 99991400, "msg": "request trigger frequency limit"}}

Directives are consumed one per matching request, oldest first; when the queue
for a path empties the mock goes back to answering normally. That is what lets
one assertion span "rejected twice, then accepted" — a fixed failure could only
ever prove the client gives up.

Usage:  mock_lark.py <port> <observations-path>
"""
import json
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT = int(sys.argv[1])
OBS_PATH = sys.argv[2]

_lock = threading.Lock()

# path -> [directive, ...], consumed oldest-first.
_injected = {}


def record(method, path, body, injected=None):
    """Append one observation.

    `injected` carries the status this request was *answered* with when a
    directive fired. The assertions need it because the request itself looks
    identical either way — without it "the client retried after a throttle" and
    "the client sent three unrelated messages" are the same observation.
    """
    entry = {"method": method, "path": path, "body": body}
    if injected is not None:
        entry["injected_status"] = injected
    with _lock, open(OBS_PATH, "a") as fh:
        fh.write(json.dumps(entry) + "\n")


def take_injection(path):
    """Pop the next queued failure for `path`, if any."""
    with _lock:
        queue = _injected.get(path)
        if not queue:
            return None
        directive = queue.pop(0)
    return directive


def queue_injection(directive):
    times = int(directive.get("times", 1))
    path = directive["path"]
    with _lock:
        _injected.setdefault(path, []).extend([directive] * times)


def pending_injections():
    with _lock:
        return {k: len(v) for k, v in _injected.items() if v}


def reset_injections():
    """Drop every queued directive and report what was dropped.

    Cases must not inherit each other's leftovers. A case that ends with
    directives still queued has already failed; carrying them forward makes the
    *next* case answer a throttle it never asked for, so one regression prints
    as several unrelated ones and the first real cause is buried.
    """
    with _lock:
        left = {k: len(v) for k, v in _injected.items() if v}
        _injected.clear()
    return left


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_a):  # keep the fixture's stdout readable
        pass

    def _reply(self, payload, status=200, headers=None, raw_body=None):
        raw = raw_body.encode() if raw_body is not None else json.dumps(payload).encode()
        self.send_response(status)
        ctype = "application/json; charset=utf-8" if raw_body is None else "text/html"
        self.send_header("Content-Type", ctype)
        for k, v in (headers or {}).items():
            self.send_header(k, str(v))
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def _serve_injection(self, path, method, body):
        """Answer with a queued failure if one is due. Returns True if it fired."""
        directive = take_injection(path)
        if directive is None:
            return False
        status = int(directive.get("status", 500))
        record(method, path, body, injected=status)
        self._reply(
            directive.get("body"),
            status=status,
            headers=directive.get("headers"),
            raw_body=directive.get("raw_body"),
        )
        return True

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
        if path == "/__pending":
            raw = json.dumps(pending_injections()).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(raw)))
            self.end_headers()
            self.wfile.write(raw)
            return
        if self._serve_injection(path, "GET", None):
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
        if path == "/__reset":
            raw = json.dumps(reset_injections()).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(raw)))
            self.end_headers()
            self.wfile.write(raw)
            return
        if path == "/__inject":
            # Directives are not recorded as observations: they are the
            # fixture's own traffic, and counting them would make every
            # "how many calls did the channel make" assertion off by one.
            for directive in body if isinstance(body, list) else [body]:
                queue_injection(directive)
            self._reply({"queued": pending_injections()})
            return
        if self._serve_injection(path, "POST", body):
            return
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
