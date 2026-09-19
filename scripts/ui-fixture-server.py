#!/usr/bin/env python3
import argparse
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse

MEMO_ID = "018f0c7a-8b7d-7f25-b239-36e6d9f9b001"
USER_ID = "12345678-1234-1234-1234-123456789012"
TIMESTAMP = "2026-09-19T08:30:00.000Z"

MEMOS = [
    {
        "id": MEMO_ID,
        "title": "UI regression baseline",
        "content": "Visual regression testing keeps layout changes reviewable before merge.",
        "tags": ["ui", "ci", "playwright"],
        "user_id": USER_ID,
        "created_at": TIMESTAMP,
        "updated_at": TIMESTAMP,
        "version": 3,
    },
    {
        "id": "018f0c7a-8b7d-7f25-b239-36e6d9f9b002",
        "title": "Release checklist",
        "content": "Run checks, inspect visual diffs, review the smoke test, then merge.",
        "tags": ["release", "quality"],
        "user_id": USER_ID,
        "created_at": TIMESTAMP,
        "updated_at": TIMESTAMP,
        "version": 2,
    },
    {
        "id": "018f0c7a-8b7d-7f25-b239-36e6d9f9b003",
        "title": "Architecture notes",
        "content": "ScyllaDB is authoritative. Redis and Elasticsearch are rebuildable projections.",
        "tags": ["architecture", "backend"],
        "user_id": USER_ID,
        "created_at": TIMESTAMP,
        "updated_at": TIMESTAMP,
        "version": 7,
    },
]


class FixtureHandler(BaseHTTPRequestHandler):
    def send_json(self, status, payload):
        body = json.dumps(payload, separators=(",", ":")).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        path = urlparse(self.path).path

        if path == "/healthz":
            self.send_json(200, {"status": "ok"})
            return

        if path == "/api/v1/memos/search":
            self.send_json(
                200,
                {
                    "items": [MEMOS[0], MEMOS[2]],
                    "total": 2,
                    "page": 1,
                    "total_pages": 1,
                },
            )
            return

        if path == f"/api/v1/memos/{MEMO_ID}":
            self.send_json(200, MEMOS[0])
            return

        if path == "/api/v1/memos":
            self.send_json(200, MEMOS)
            return

        self.send_json(404, {"message": "Visual fixture route not found"})

    def do_POST(self):
        if urlparse(self.path).path == "/api/v1/memos":
            self.send_json(201, {**MEMOS[0], "version": 1})
            return
        self.send_json(404, {"message": "Visual fixture route not found"})

    def do_PATCH(self):
        if urlparse(self.path).path.startswith("/api/v1/memos/"):
            self.send_json(200, {**MEMOS[0], "version": 4})
            return
        self.send_json(404, {"message": "Visual fixture route not found"})

    def do_DELETE(self):
        if urlparse(self.path).path.startswith("/api/v1/memos/"):
            self.send_response(204)
            self.end_headers()
            return
        self.send_json(404, {"message": "Visual fixture route not found"})


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, default=18080)
    args = parser.parse_args()
    ThreadingHTTPServer(("127.0.0.1", args.port), FixtureHandler).serve_forever()


if __name__ == "__main__":
    main()
