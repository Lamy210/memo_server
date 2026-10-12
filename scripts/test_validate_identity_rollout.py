#!/usr/bin/env python3
import io
import json
import unittest

from scripts.validate_identity_rollout import (
    MAX_JWKS_KEYS,
    MAX_JSON_BYTES,
    PreflightError,
    fetch_json,
    run_preflight,
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


class SequenceOpener:
    def __init__(self, responses: list[FakeResponse]) -> None:
        self.responses = iter(responses)
        self.calls = []

    def open(self, request, *, timeout: float):
        self.calls.append((request, timeout))
        return next(self.responses)


def discovery_document(*, jwks_uri: str = "https://id.example.test/tenant/jwks.json?set=prod"):
    return {
        "issuer": "https://id.example.test/tenant",
        "jwks_uri": jwks_uri,
        "authorization_endpoint": "https://id.example.test/tenant/authorize?tenant=prod",
        "token_endpoint": "https://id.example.test/tenant/token?tenant=prod",
        "end_session_endpoint": "https://id.example.test/tenant/logout?tenant=prod",
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["client_secret_basic"],
    }


def rsa_key(*, kid: str = "rsa-2026-10"):
    return {
        "kid": kid,
        "kty": "RSA",
        "alg": "RS256",
        "use": "sig",
        "n": "AQAB",
        "e": "AQAB",
    }


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
            "https://id.example.test/tenant path",
            "https://id.example.test?x=1",
            "https://id.example.test#fragment",
            "/relative",
        ):
            with self.subTest(value=value):
                with self.assertRaises(PreflightError):
                    validate_https_url(value, "issuer")

    def test_discovery_requires_exact_issuer_and_expected_jwks_uri(self) -> None:
        document = discovery_document()

        summary = validate_discovery_document(
            document,
            expected_issuer="https://id.example.test/tenant",
            expected_jwks_uri="https://id.example.test/tenant/jwks.json?set=prod",
        )
        self.assertEqual(summary["issuer"], "https://id.example.test/tenant")

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

    def test_discovery_rejects_browser_incompatible_capabilities(self) -> None:
        missing_s256 = {
            **discovery_document(),
            "code_challenge_methods_supported": ["plain"],
        }
        with self.assertRaises(PreflightError):
            validate_discovery_document(
                missing_s256,
                expected_issuer="https://id.example.test/tenant",
                expected_jwks_uri="https://id.example.test/tenant/jwks.json?set=prod",
            )

        unsupported_client_auth = {
            **discovery_document(),
            "token_endpoint_auth_methods_supported": ["client_secret_post"],
        }
        with self.assertRaises(PreflightError):
            validate_discovery_document(
                unsupported_client_auth,
                expected_issuer="https://id.example.test/tenant",
                expected_jwks_uri="https://id.example.test/tenant/jwks.json?set=prod",
            )

        missing_authorization_endpoint = discovery_document()
        missing_authorization_endpoint.pop("authorization_endpoint")
        with self.assertRaises(PreflightError):
            validate_discovery_document(
                missing_authorization_endpoint,
                expected_issuer="https://id.example.test/tenant",
                expected_jwks_uri="https://id.example.test/tenant/jwks.json?set=prod",
            )

    def test_jwks_requires_unique_public_keys_compatible_with_signature_mode(self) -> None:
        valid = {
            "keys": [
                rsa_key(),
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
        malformed_metadata = {
            **rsa_key(kid="rsa-bad-metadata"),
            "use": ["sig"],
        }

        with self.assertRaises(PreflightError):
            validate_jwks_document(
                {"keys": [rsa_key(kid="rsa-good"), malformed_metadata]},
                signature_mode="rs256",
            )

    def test_jwks_rejects_invalid_base64url_key_material(self) -> None:
        invalid_key = {**rsa_key(), "n": "not+base64url"}
        with self.assertRaises(PreflightError):
            validate_jwks_document({"keys": [invalid_key]}, signature_mode="rs256")

    def test_jwks_rejects_unbounded_sets(self) -> None:
        keys = [rsa_key(kid=f"k-{index}") for index in range(MAX_JWKS_KEYS + 1)]
        with self.assertRaises(PreflightError):
            validate_jwks_document({"keys": keys}, signature_mode="rs256")

    def test_fetch_json_uses_finite_bounded_timeout_and_response_size(self) -> None:
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

        for invalid_timeout in (float("nan"), float("inf"), float("-inf"), 0.0, -1.0):
            with self.subTest(timeout=invalid_timeout):
                with self.assertRaises(PreflightError):
                    fetch_json(
                        "https://id.example.test/jwks.json",
                        opener=opener,
                        timeout_seconds=invalid_timeout,
                    )

    def test_preflight_summary_does_not_echo_endpoint_query_secrets(self) -> None:
        jwks_uri = "https://id.example.test/tenant/jwks.json?token=top-secret"
        opener = SequenceOpener(
            [
                FakeResponse(discovery_document(jwks_uri=jwks_uri)),
                FakeResponse({"keys": [rsa_key()]}),
            ]
        )

        result = run_preflight(
            issuer="https://id.example.test/tenant",
            jwks_uri=jwks_uri,
            signature_mode="rs256",
            opener=opener,
        )

        serialized = json.dumps(result, sort_keys=True)
        self.assertNotIn("top-secret", serialized)
        self.assertEqual(result["jwks_uri"], "https://id.example.test/tenant/jwks.json?<redacted>")
        self.assertEqual(
            result["discovery"]["authorization_endpoint"],
            "https://id.example.test/tenant/authorize?<redacted>",
        )


if __name__ == "__main__":
    unittest.main()
