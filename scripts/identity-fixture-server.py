#!/usr/bin/env python3
import argparse
import base64
import hashlib
import json
import secrets
import ssl
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlencode, urlparse, urlunparse

ISSUER = ""
CLIENT_ID = ""
CLIENT_SECRET = ""
REDIRECT_URI = ""
AUTHORIZATION_CODES = {}
REFRESH_TOKENS = {"refresh-fixture-1": 2, "refresh-fixture-2": 3}


def base64url_sha256(value: str) -> str:
    digest = hashlib.sha256(value.encode("ascii")).digest()
    return base64.urlsafe_b64encode(digest).decode("ascii").rstrip("=")


def append_query(url: str, values: dict[str, str]) -> str:
    parsed = urlparse(url)
    query = parse_qs(parsed.query, keep_blank_values=True)
    for key, value in values.items():
        query[key] = [value]
    flattened = []
    for key, items in query.items():
        for item in items:
            flattened.append((key, item))
    return urlunparse(parsed._replace(query=urlencode(flattened)))


class IdentityFixtureHandler(BaseHTTPRequestHandler):
    server_version = "IdentityFixture"
    sys_version = ""

    def log_message(self, format, *args):
        return

    def send_json(self, status, payload, *, internal_metadata=False):
        body = json.dumps(payload, separators=(",", ":")).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Cache-Control", "no-store")
        if internal_metadata:
            # Deliberate fixture-only metadata: the SvelteKit boundary must not
            # reflect these upstream details into browser-facing responses.
            self.send_header("X-Fixture-Internal", "private")
            self.send_header(
                "Set-Cookie", "upstream_internal=secret; Path=/; HttpOnly; SameSite=Lax"
            )
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        parsed = urlparse(self.path)
        path = parsed.path

        if path == "/healthz":
            self.send_json(200, {"status": "ok"})
            return

        if path == "/.well-known/openid-configuration":
            self.send_json(
                200,
                {
                    "issuer": ISSUER,
                    "authorization_endpoint": f"{ISSUER}/authorize",
                    "token_endpoint": f"{ISSUER}/token",
                    "end_session_endpoint": f"{ISSUER}/logout",
                    "code_challenge_methods_supported": ["S256"],
                    "token_endpoint_auth_methods_supported": ["client_secret_basic"],
                },
                internal_metadata=True,
            )
            return

        if path == "/authorize":
            query = parse_qs(parsed.query, keep_blank_values=True)
            client_id = query.get("client_id", [None])[0]
            redirect_uri = query.get("redirect_uri", [None])[0]
            response_type = query.get("response_type", [None])[0]
            prompt = query.get("prompt", [None])[0]
            state = query.get("state", [None])[0]
            challenge = query.get("code_challenge", [None])[0]
            challenge_method = query.get("code_challenge_method", [None])[0]
            scopes = set((query.get("scope", [""])[0] or "").split())

            if (
                client_id != CLIENT_ID
                or redirect_uri != REDIRECT_URI
                or response_type != "code"
                or prompt != "consent"
                or not state
                or not challenge
                or challenge_method != "S256"
                or not {"openid", "offline_access"}.issubset(scopes)
            ):
                self.send_json(400, {"error": "invalid_request"})
                return

            code = f"fixture-code-{secrets.token_urlsafe(12)}"
            AUTHORIZATION_CODES[code] = {
                "challenge": challenge,
                "redirect_uri": redirect_uri,
            }
            location = append_query(redirect_uri, {"code": code, "state": state})
            self.send_response(303)
            self.send_header("Location", location)
            self.send_header("Cache-Control", "no-store")
            self.send_header("Content-Length", "0")
            self.end_headers()
            return

        if path == "/logout":
            self.send_json(200, {"status": "ok"})
            return

        self.send_json(404, {"error": "not_found"})

    def do_POST(self):
        if urlparse(self.path).path != "/token":
            self.send_json(404, {"error": "not_found"})
            return

        expected_basic = base64.b64encode(
            f"{CLIENT_ID}:{CLIENT_SECRET}".encode("utf-8")
        ).decode("ascii")
        if self.headers.get("Authorization") != f"Basic {expected_basic}":
            self.send_json(401, {"error": "invalid_client"}, internal_metadata=True)
            return

        try:
            content_length = int(self.headers.get("Content-Length", "0"))
        except ValueError:
            self.send_json(400, {"error": "invalid_request"}, internal_metadata=True)
            return
        if content_length <= 0 or content_length > 16 * 1024:
            self.send_json(400, {"error": "invalid_request"}, internal_metadata=True)
            return

        body = self.rfile.read(content_length).decode("utf-8", errors="strict")
        form = parse_qs(body, keep_blank_values=True)
        grant_type = form.get("grant_type", [None])[0]

        if grant_type == "authorization_code":
            code = form.get("code", [None])[0]
            verifier = form.get("code_verifier", [None])[0]
            redirect_uri = form.get("redirect_uri", [None])[0]
            transaction = AUTHORIZATION_CODES.pop(code, None) if code else None

            if (
                transaction is None
                or not verifier
                or redirect_uri != transaction["redirect_uri"]
                or redirect_uri != REDIRECT_URI
                or base64url_sha256(verifier) != transaction["challenge"]
            ):
                self.send_json(400, {"error": "invalid_grant"}, internal_metadata=True)
                return

            self.send_json(
                200,
                {
                    "access_token": "access-fixture-1",
                    "token_type": "Bearer",
                    "expires_in": 35,
                    "refresh_token": "refresh-fixture-1",
                },
                internal_metadata=True,
            )
            return

        if grant_type == "refresh_token":
            refresh_token = form.get("refresh_token", [None])[0]
            sequence = REFRESH_TOKENS.pop(refresh_token, None) if refresh_token else None
            if sequence is None:
                self.send_json(400, {"error": "invalid_grant"}, internal_metadata=True)
                return

            next_refresh = f"refresh-fixture-{sequence}"
            REFRESH_TOKENS[next_refresh] = sequence + 1
            self.send_json(
                200,
                {
                    "access_token": f"access-fixture-{sequence}",
                    "token_type": "Bearer",
                    "expires_in": 300,
                    "refresh_token": next_refresh,
                },
                internal_metadata=True,
            )
            return

        self.send_json(400, {"error": "unsupported_grant_type"}, internal_metadata=True)


def main():
    global ISSUER, CLIENT_ID, CLIENT_SECRET, REDIRECT_URI

    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, default=19090)
    parser.add_argument("--client-id", required=True)
    parser.add_argument("--client-secret", required=True)
    parser.add_argument("--redirect-uri", required=True)
    parser.add_argument("--tls-cert", required=True)
    parser.add_argument("--tls-key", required=True)
    args = parser.parse_args()

    ISSUER = f"https://127.0.0.1:{args.port}"
    CLIENT_ID = args.client_id
    CLIENT_SECRET = args.client_secret
    REDIRECT_URI = args.redirect_uri

    server = ThreadingHTTPServer(("127.0.0.1", args.port), IdentityFixtureHandler)
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(certfile=args.tls_cert, keyfile=args.tls_key)
    server.socket = context.wrap_socket(server.socket, server_side=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
