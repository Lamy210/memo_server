#!/usr/bin/env python3
import io
import json
import unittest

from scripts.validate_identity_rollout import (
    MAX_JWKS_KEYS,
    MAX_JSON_BYTES,
    PreflightError,
    fetch_json,
    validate_discovery_document,
    validate_https_url,
    validate_jwks_document,
)


class FakeResponse:
    def __init__(self, payload: object, *, status: int = 200, extra_bytes: bytes = b"") -> None:
        self.status = status
        self._body = json.dumps(payload).encode("utf-8") + extra_bytes
        self.headers = {"Content-Type": "application/json"}

    def read(self, size: int = -1) -> bytes:
        return io.BytesIO(self._body).read(size)

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc, tb):
        return False


class FakeOpener:
    def __init__(self, response: FakeResponse) -> None:
        self.response = response
        self.calls = []

    def open(self, request, *, timeout: float):
        self.calls.append((request, timeout))
        return self.response


class IdentityRolloutPreflightTests(unittest.TestCase):
    def test_requires_https_absolute_urls_without_ambiguous_components(self) -> None:
        self.assertEqual(
            validate_https_url("https://id.example.test/tenant", "issuer"),
            "https://id.example.test/tenant",
        )

        for value in (
            "http://id.example.test",
            "https://user:secret@id.example.test",
            "https://id.example.test:bad",
            "https://id.example.test?x=1",
            "https://id.example.test#fragment",
            "/relative",
        ):
            with self.subTest(value=value):
                with self.assertRaises(PreflightError):
                    validate_https_url(value, "issuer")

    def test_discovery_requires_exact_issuer_and_expected_jwks_uri(self) -> None:
        document = {
            "issuer": "https://id.example.test/tenant",
            "jwks_uri": "https://id.example.test/tenant/jwks.json?set=prod",
            "authorization_endpoint": "https://id.example.test/tenant/authorize?tenant=prod",
            "token_endpoint": "https://id.example.test/tenant/token?tenant=prod",
        }

        validate_discovery_document(
            document,
            expected_issuer="https://id.example.test/tenant",
            expected_jwks_uri="https://id.example.test/tenant/jwks.json?set=prod",
        )

        with self.assertRaises(PreflightError):
            validate_discovery_document(
                document,
                expected_issuer="https://id.example.test/tenant/",
                expected_jwks_uri="https://id.example.test/tenant/jwks.json?set=prod",
            )

        with self.assertRaises(PreflightError):
            validate_discovery_document(
                document,
                expected_issuer="https://id.example.test/tenant",
                expected_jwks_uri="https://keys.example.test/jwks.json?set=prod",
            )

    def test_jwks_requires_unique_public_keys_compatible_with_signature_mode(self) -> None:
        valid = {
            "keys": [
                {
                    "kid": "rsa-2026-10",
                    "kty": "RSA",
                    "alg": "RS256",
                    "use": "sig",
                    "n": "AQAB",
                    "e": "AQAB",
                },
                {
                    "kid": "ec-2026-10",
                    "kty": "EC",
                    "alg": "ES384",
                    "use": "sig",
                    "crv": "P-384",
                    "x": "AA",
                    "y": "BB",
                },
            ]
        }

        summary = validate_jwks_document(valid, signature_mode="rs256-es384")
        self.assertEqual(summary["key_count"], 2)
        self.assertEqual(summary["algorithms"], ["ES384", "RS256"])

        duplicate = {"keys": [valid["keys"][0], dict(valid["keys"][0])]}
        with self.assertRaises(PreflightError):
            validate_jwks_document(duplicate, signature_mode="rs256")

        private_key = {"keys": [{**valid["keys"][0], "d": "private"}]}
        with self.assertRaises(PreflightError):
            validate_jwks_document(private_key, signature_mode="rs256")

        with self.assertRaises(PreflightError):
            validate_jwks_document(valid, signature_mode="es384-only-invalid-mode")

    def test_jwks_rejects_malformed_verification_metadata_even_with_another_usable_key(self) -> None:
        valid_key = {
            "kid": "rsa-good",
            "kty": "RSA",
            "alg": "RS256",
            "use": "sig",
            "n": "AQAB",
            "e": "AQAB",
        }
        malformed_metadata = {
            "kid": "rsa-bad-metadata",
            "kty": "RSA",
            "alg": "RS256",
            "use": ["sig"],
            "n": "AQAB",
            "e": "AQAB",
        }

        with self.assertRaises(PreflightError):
            validate_jwks_document(
                {"keys": [valid_key, malformed_metadata]},
                signature_mode="rs256",
            )

    def test_jwks_rejects_unbounded_sets(self) -> None:
        keys = [
            {
                "kid": f"k-{index}",
                "kty": "RSA",
                "alg": "RS256",
                "use": "sig",
                "n": "AQAB",
                "e": "AQAB",
            }
            for index in range(MAX_JWKS_KEYS + 1)
        ]
        with self.assertRaises(PreflightError):
            validate_jwks_document({"keys": keys}, signature_mode="rs256")

    def test_fetch_json_uses_bounded_timeout_and_response_size(self) -> None:
        opener = FakeOpener(FakeResponse({"issuer": "https://id.example.test"}))

        payload = fetch_json(
            "https://id.example.test/.well-known/openid-configuration",
            opener=opener,
            timeout_seconds=7.5,
        )

        self.assertEqual(payload["issuer"], "https://id.example.test")
        self.assertEqual(len(opener.calls), 1)
        request, timeout = opener.calls[0]
        self.assertEqual(timeout, 7.5)
        self.assertEqual(request.full_url, "https://id.example.test/.well-known/openid-configuration")
        self.assertEqual(request.get_header("Accept"), "application/json")

        too_large = FakeOpener(FakeResponse({}, extra_bytes=b"x" * (MAX_JSON_BYTES + 1)))
        with self.assertRaises(PreflightError):
            fetch_json("https://id.example.test/jwks.json", opener=too_large)


if __name__ == "__main__":
    unittest.main()
