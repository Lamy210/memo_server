#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import sys
import urllib.error
import urllib.request
from typing import Any
from urllib.parse import urlsplit

MAX_JSON_BYTES = 256 * 1024
MAX_JWKS_KEYS = 64
MAX_KID_BYTES = 128
DEFAULT_TIMEOUT_SECONDS = 10.0

_ALLOWED_SIGNATURE_MODES = {
    "rs256": {"RS256"},
    "rs256-es384": {"RS256", "ES384"},
    "es384": {"ES384"},
}
_PRIVATE_JWK_MEMBERS = {"d", "p", "q", "dp", "dq", "qi", "oth", "k"}


class PreflightError(RuntimeError):
    pass


class NoRedirectHandler(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def validate_https_url(value: str, field_name: str) -> str:
    if not isinstance(value, str) or not value or value != value.strip():
        raise PreflightError(f"{field_name} must be a non-empty canonical HTTPS URL")
    if any(ord(char) < 0x20 or ord(char) == 0x7F for char in value) or "\\" in value:
        raise PreflightError(f"{field_name} contains an ambiguous character")

    try:
        parsed = urlsplit(value)
    except ValueError as error:
        raise PreflightError(f"{field_name} is not a valid URL") from error

    if (
        parsed.scheme != "https"
        or not parsed.hostname
        or parsed.username is not None
        or parsed.password is not None
        or parsed.query
        or parsed.fragment
    ):
        raise PreflightError(
            f"{field_name} must be an absolute HTTPS URL without credentials, query, or fragment"
        )

    return value


def _require_object(document: object, name: str) -> dict[str, Any]:
    if not isinstance(document, dict):
        raise PreflightError(f"{name} must be a JSON object")
    return document


def validate_discovery_document(
    document: object,
    *,
    expected_issuer: str,
    expected_jwks_uri: str,
) -> dict[str, str]:
    payload = _require_object(document, "discovery document")

    issuer = payload.get("issuer")
    if issuer != expected_issuer:
        raise PreflightError("discovery issuer does not exactly match the configured issuer")

    jwks_uri = payload.get("jwks_uri")
    if not isinstance(jwks_uri, str):
        raise PreflightError("discovery document is missing jwks_uri")
    validate_https_url(jwks_uri, "discovery jwks_uri")
    if jwks_uri != expected_jwks_uri:
        raise PreflightError("discovery jwks_uri does not exactly match the configured JWKS URI")

    summary = {"issuer": issuer, "jwks_uri": jwks_uri}
    for field in ("authorization_endpoint", "token_endpoint"):
        value = payload.get(field)
        if value is None:
            continue
        if not isinstance(value, str):
            raise PreflightError(f"discovery {field} must be a string")
        validate_https_url(value, f"discovery {field}")
        summary[field] = value

    return summary


def _key_candidate_algorithms(key: dict[str, Any]) -> set[str]:
    declared_alg = key.get("alg")
    if declared_alg is not None and not isinstance(declared_alg, str):
        raise PreflightError("JWKS key alg must be a string when present")

    public_use = key.get("use")
    if public_use is not None and public_use != "sig":
        return set()

    key_ops = key.get("key_ops")
    if key_ops is not None:
        if not isinstance(key_ops, list) or not all(isinstance(value, str) for value in key_ops):
            raise PreflightError("JWKS key_ops must be a string array when present")
        if "verify" not in key_ops:
            return set()

    kty = key.get("kty")
    if kty == "RSA":
        if not isinstance(key.get("n"), str) or not key["n"]:
            return set()
        if not isinstance(key.get("e"), str) or not key["e"]:
            return set()
        inferred = {"RS256"}
    elif kty == "EC":
        if key.get("crv") != "P-384":
            return set()
        if not isinstance(key.get("x"), str) or not key["x"]:
            return set()
        if not isinstance(key.get("y"), str) or not key["y"]:
            return set()
        inferred = {"ES384"}
    else:
        return set()

    if declared_alg is None:
        return inferred
    return inferred if declared_alg in inferred else set()


def validate_jwks_document(document: object, *, signature_mode: str) -> dict[str, object]:
    allowed_algorithms = _ALLOWED_SIGNATURE_MODES.get(signature_mode)
    if allowed_algorithms is None:
        raise PreflightError(f"unsupported signature mode: {signature_mode}")

    payload = _require_object(document, "JWKS document")
    keys = payload.get("keys")
    if not isinstance(keys, list) or not (1 <= len(keys) <= MAX_JWKS_KEYS):
        raise PreflightError(f"JWKS must contain 1..={MAX_JWKS_KEYS} keys")

    seen_kids: set[str] = set()
    compatible_algorithms: set[str] = set()
    compatible_key_count = 0

    for raw_key in keys:
        if not isinstance(raw_key, dict):
            raise PreflightError("each JWKS key must be a JSON object")
        key = raw_key

        if _PRIVATE_JWK_MEMBERS.intersection(key):
            raise PreflightError("JWKS must not publish private or symmetric key material")

        kid = key.get("kid")
        if kid is not None:
            if not isinstance(kid, str) or not kid or len(kid.encode("utf-8")) > MAX_KID_BYTES:
                raise PreflightError("JWKS contains an empty or oversized kid")
            if kid in seen_kids:
                raise PreflightError("JWKS contains duplicate kid values")
            seen_kids.add(kid)

        candidate_algorithms = _key_candidate_algorithms(key).intersection(allowed_algorithms)
        if kid is not None and candidate_algorithms:
            compatible_key_count += 1
            compatible_algorithms.update(candidate_algorithms)

    if compatible_key_count == 0:
        raise PreflightError("JWKS does not contain a usable verification key for the signature mode")

    return {
        "key_count": len(keys),
        "compatible_key_count": compatible_key_count,
        "algorithms": sorted(compatible_algorithms),
    }


def _default_opener():
    return urllib.request.build_opener(NoRedirectHandler())


def fetch_json(
    url: str,
    *,
    opener=None,
    timeout_seconds: float = DEFAULT_TIMEOUT_SECONDS,
) -> object:
    validate_https_url(url, "request URL")
    if timeout_seconds <= 0:
        raise PreflightError("timeout must be positive")

    request = urllib.request.Request(
        url,
        method="GET",
        headers={"Accept": "application/json", "User-Agent": "memo-server-identity-preflight/1"},
    )
    opener = opener or _default_opener()

    try:
        with opener.open(request, timeout=timeout_seconds) as response:
            status = getattr(response, "status", 200)
            if status < 200 or status >= 300:
                raise PreflightError(f"identity endpoint returned HTTP {status}")
            body = response.read(MAX_JSON_BYTES + 1)
    except PreflightError:
        raise
    except urllib.error.HTTPError as error:
        raise PreflightError(f"identity endpoint returned HTTP {error.code}") from error
    except (urllib.error.URLError, TimeoutError, OSError) as error:
        raise PreflightError("identity endpoint could not be reached") from error

    if len(body) > MAX_JSON_BYTES:
        raise PreflightError(f"identity JSON response exceeds {MAX_JSON_BYTES} bytes")

    try:
        return json.loads(body)
    except (json.JSONDecodeError, UnicodeDecodeError) as error:
        raise PreflightError("identity endpoint returned invalid JSON") from error


def discovery_url(issuer: str) -> str:
    validate_https_url(issuer, "issuer")
    return issuer.rstrip("/") + "/.well-known/openid-configuration"


def run_preflight(
    *,
    issuer: str,
    jwks_uri: str,
    signature_mode: str,
    timeout_seconds: float = DEFAULT_TIMEOUT_SECONDS,
    opener=None,
) -> dict[str, object]:
    issuer = validate_https_url(issuer, "issuer")
    jwks_uri = validate_https_url(jwks_uri, "jwks_uri")

    discovery = fetch_json(
        discovery_url(issuer),
        opener=opener,
        timeout_seconds=timeout_seconds,
    )
    discovery_summary = validate_discovery_document(
        discovery,
        expected_issuer=issuer,
        expected_jwks_uri=jwks_uri,
    )

    jwks = fetch_json(jwks_uri, opener=opener, timeout_seconds=timeout_seconds)
    jwks_summary = validate_jwks_document(jwks, signature_mode=signature_mode)

    return {
        "status": "ok",
        "issuer": issuer,
        "jwks_uri": jwks_uri,
        "signature_mode": signature_mode,
        "discovery": discovery_summary,
        "jwks": jwks_summary,
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Validate the provider-neutral production identity issuer/JWKS contract"
    )
    parser.add_argument("--issuer", required=True)
    parser.add_argument("--jwks-uri", required=True)
    parser.add_argument(
        "--signature-mode",
        choices=sorted(_ALLOWED_SIGNATURE_MODES),
        default="rs256",
    )
    parser.add_argument("--timeout-seconds", type=float, default=DEFAULT_TIMEOUT_SECONDS)
    args = parser.parse_args(argv)

    try:
        result = run_preflight(
            issuer=args.issuer,
            jwks_uri=args.jwks_uri,
            signature_mode=args.signature_mode,
            timeout_seconds=args.timeout_seconds,
        )
    except PreflightError as error:
        print(json.dumps({"status": "error", "message": str(error)}, ensure_ascii=False))
        return 1

    print(json.dumps(result, ensure_ascii=False, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main())
