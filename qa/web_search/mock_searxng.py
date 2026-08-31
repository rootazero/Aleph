#!/usr/bin/env python3
"""A SearXNG instance that records what it was asked.

The claim these phases make is about the WIRE: that a parameter the model
named on the tool face reaches the backend's query string. An in-process test
can assert that `SearchOptions::searxng_time_range` returns "week"; it cannot
assert that a turn the model drives ends with `time_range=week` in a request.
Those are two objects on two paths, and this file is the oracle for the second.

SearXNG is the only backend this can be done against. Seven of the nine
hardcode their endpoint, and the ninth (firecrawl) needs a credential; only
searxng takes a `base_url` and no API key. So the fixture proves the wiring,
not the nine backends — see README.md.

Usage:  mock_searxng.py PORT LOG_PATH [--empty] [--shared]

  --empty  answer every query with zero results and no unresponsive engines.
           That is an *answer*, not an error: the provider promotes
           "0 results + unresponsive engines" to a typed error, and this
           fixture needs the other case — the one where a backend legitimately
           found nothing and the chain must keep going.

  --shared make the FIRST result a page every `--shared` instance also
           returns, under a url each instance spells differently (one adds a
           tracking parameter, the other a `www.` and a trailing slash). The
           second result stays instance-specific. That is the shape a fan-out
           merge has to get right: the same page found twice must collapse to
           one row while the two distinct pages both survive, and it has to
           happen through url normalisation rather than string equality —
           which is exactly what two real backends do to one link.
"""
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse

PORT = int(sys.argv[1])
LOG = sys.argv[2]
EMPTY = "--empty" in sys.argv[3:]
SHARED = "--shared" in sys.argv[3:]
# Distinguishable per instance so a driver can tell which backend answered
# from the result text alone.
TAG = f"port{PORT}"

# Two spellings of one url, handed out by whichever instance answers first.
# Neither is a string match for the other, so a merge that collapses them has
# gone through normalisation; a merge that keeps both has not.
SHARED_SPELLINGS = [
    "https://example.invalid/shared/page?utm_source=qa&utm_campaign=fanout",
    "https://www.example.invalid/shared/page/",
]
SHARED_URL = SHARED_SPELLINGS[PORT % len(SHARED_SPELLINGS)]


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):  # noqa: N802 - http.server's interface
        parsed = urlparse(self.path)
        if parsed.path != "/search":
            self.send_response(404)
            self.end_headers()
            return

        # The full query string, verbatim. Not parsed into a dict: a driver
        # asserting on `time_range=week` should be reading the bytes that went
        # over the wire, not this file's opinion of them.
        with open(LOG, "a", encoding="utf-8") as fh:
            fh.write(parsed.query + "\n")
            fh.flush()

        first_url = SHARED_URL if SHARED else f"https://example.invalid/{TAG}/1"
        results = (
            []
            if EMPTY
            else [
                {
                    "title": f"QA result 1 from {TAG}",
                    "url": first_url,
                    "content": f"QA snippet one from {TAG}.",
                },
                {
                    "title": f"QA result 2 from {TAG}",
                    "url": f"https://example.invalid/{TAG}/2",
                    "content": f"QA snippet two from {TAG}.",
                },
            ]
        )
        body = json.dumps(
            {"results": results, "unresponsive_engines": []}
        ).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_args):
        pass


if __name__ == "__main__":
    open(LOG, "w", encoding="utf-8").close()
    print(f"mock searxng on {PORT} -> {LOG} (empty={EMPTY})", flush=True)
    ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
