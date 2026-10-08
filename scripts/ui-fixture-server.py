#!/usr/bin/env python3
import argparse
import gzip
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

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
        "id": "018f0c7a-8b7d-4f25-b239-36e6d9f9b002",
        "title": "Release checklist",
        "content": "Run checks, inspect visual diffs, review the smoke test, then merge.",
        "tags": ["release", "quality"],
        "user_id": USER_ID,
        "created_at": TIMESTAMP,
        "updated_at": TIMESTAMP,
        "version": 2,
    },
    {
        "id": "018f0c7a-8b7d-4f25-b239-36e6d9f9b003",
        "title": "Architecture notes",
        "content": "ScyllaDB is authoritative. Redis and Elasticsearch are rebuildable projections.",
        "tags": ["architecture", "backend"],
        "user_id": USER_ID,
        "created_at": TIMESTAMP,
        "updated_at": TIMESTAMP,
        "version": 7,
    },
]


UNAUTHORIZED = False
REQUIRE_IDENTITY_ENCODING = False
ENCODED_RESPONSE = False


class FixtureHandler(BaseHTTPRequestHandler):
    def send_json(self, status, payload):
        body = json.dumps(payload, separators=(",", ":")).encode("utf-8")
        encoded = ENCODED_RESPONSE and urlparse(self.path).path.startswith("/api/v1/")
        wire_body = gzip.compress(body) if encoded else body

        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        if encoded:
            self.send_header("Content-Encoding", "gzip")
        self.send_header("Content-Length", str(len(wire_body)))
        self.end_headers()
        self.wfile.write(wire_body)

    def require_identity_encoding(self, path):
        if (
            REQUIRE_IDENTITY_ENCODING
            and path.startswith("/api/v1/")
            and self.headers.get("Accept-Encoding") != "identity"
        ):
            self.send_json(
                400,
                {
                    "message": "Fixture expected Accept-Encoding: identity on memo API backend requests"
                },
            )
            return False
        return True

    def do_GET(self):
        parsed = urlparse(self.path)
        path = parsed.path
        query = parse_qs(parsed.query, keep_blank_values=True)

        if not self.require_identity_encoding(path):
            return

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
            if UNAUTHORIZED:
                self.send_json(401, {"message": "Unauthorized visual fixture"})
            elif query.get("pagination") == ["cursor-v1"]:
                try:
                    limit = int(query.get("limit", ["20"])[0])
                except ValueError:
                    self.send_json(400, {"message": "Invalid memo list limit"})
                    return

                cursor = query.get("cursor", [None])[0]
                ordered = sorted(MEMOS, key=lambda memo: memo["id"], reverse=True)
                if cursor:
                    prefix = "v1."
                    if not cursor.startswith(prefix):
                        self.send_json(400, {"message": "Invalid memo list cursor"})
                        return
                    after_id = cursor[len(prefix) :]
                    ordered = [memo for memo in ordered if memo["id"] < after_id]

                page_items = ordered[:limit]
                has_more = len(ordered) > limit
                next_cursor = (
                    f"v1.{page_items[-1]['id']}" if has_more and page_items else None
                )
                self.send_json(
                    200,
                    {
                        "pagination": "cursor-v1",
                        "items": page_items,
                        "limit": limit,
                        "next_cursor": next_cursor,
                    },
                )
            else:
                self.send_json(200, MEMOS)
            return

        self.send_json(404, {"message": "Visual fixture route not found"})

    def do_POST(self):
        global UNAUTHORIZED, ENCODED_RESPONSE
        path = urlparse(self.path).path

        if not self.require_identity_encoding(path):
            return

        if path == "/__visual__/scenario/unauthorized":
            UNAUTHORIZED = True
            self.send_json(200, {"scenario": "unauthorized"})
            return

        if path == "/__visual__/scenario/encoded":
            ENCODED_RESPONSE = True
            self.send_json(200, {"scenario": "encoded"})
            return

        if path == "/__visual__/scenario/success":
            UNAUTHORIZED = False
            ENCODED_RESPONSE = False
            self.send_json(200, {"scenario": "success"})
            return

        if path == "/api/v1/memos":
            self.send_json(201, {**MEMOS[0], "version": 1})
            return
        self.send_json(404, {"message": "Visual fixture route not found"})

    def do_PATCH(self):
        path = urlparse(self.path).path
        if not self.require_identity_encoding(path):
            return
        if path.startswith("/api/v1/memos/"):
            self.send_json(200, {**MEMOS[0], "version": 4})
            return
        self.send_json(404, {"message": "Visual fixture route not found"})

    def do_DELETE(self):
        path = urlparse(self.path).path
        if not self.require_identity_encoding(path):
            return
        if path.startswith("/api/v1/memos/"):
            self.send_response(204)
            self.end_headers()
            return
        self.send_json(404, {"message": "Visual fixture route not found"})


def main():
    global REQUIRE_IDENTITY_ENCODING

    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, default=18080)
    parser.add_argument("--require-identity-encoding", action="store_true")
    args = parser.parse_args()
    REQUIRE_IDENTITY_ENCODING = args.require_identity_encoding
    ThreadingHTTPServer(("127.0.0.1", args.port), FixtureHandler).serve_forever()


if __name__ == "__main__":
    main()
