#!/usr/bin/env python3
from __future__ import annotations

import io
import unittest
from contextlib import redirect_stderr, redirect_stdout
from unittest.mock import Mock

from scripts.run_identity_acceptance import (
    AcceptanceConfig,
    AcceptanceEvidence,
    AcceptancePhase,
    build_verifier_env,
    main,
    run_acceptance,
    run_verifier,
)
from scripts.validate_identity_rollout import PreflightError


class Completed:
    def __init__(self, returncode: int) -> None:
        self.returncode = returncode


class IdentityAcceptanceRunnerTests(unittest.TestCase):
    def config(self) -> AcceptanceConfig:
        return AcceptanceConfig(
            issuer="https://id.example.test/tenant",
            jwks_uri="https://id.example.test/tenant/jwks.json",
            audience="memo-api",
            signature_mode="es384",
            token_type_mode="at-jwt",
            max_access_token_lifetime_seconds=300,
            timeout_seconds=7.5,
        )

    def evidence(self) -> AcceptanceEvidence:
        return AcceptanceEvidence(
            browser_flow_validated=True,
            rotation_rollback_validated=True,
            monitoring_validated=True,
        )

    def test_incomplete_operator_evidence_stops_before_network_and_token_verifier(self) -> None:
        preflight = Mock(return_value={"status": "ok"})
        verifier = Mock(return_value=0)
        incomplete = AcceptanceEvidence(
            browser_flow_validated=True,
            rotation_rollback_validated=False,
            monitoring_validated=True,
        )

        result = run_acceptance(
            self.config(),
            incomplete,
            preflight_fn=preflight,
            verifier_fn=verifier,
        )

        self.assertEqual(result.phase, AcceptancePhase.OPERATOR_EVIDENCE)
        self.assertFalse(result.ready)
        self.assertFalse(result.rotation_rollback_validated)
        preflight.assert_not_called()
        verifier.assert_not_called()

    def test_preflight_failure_stops_before_token_verifier(self) -> None:
        verifier = Mock(return_value=0)

        def failing_preflight(**_kwargs):
            raise PreflightError("issuer unavailable")

        result = run_acceptance(
            self.config(),
            self.evidence(),
            preflight_fn=failing_preflight,
            verifier_fn=verifier,
        )

        self.assertEqual(result.phase, AcceptancePhase.ISSUER_PREFLIGHT)
        self.assertFalse(result.ready)
        verifier.assert_not_called()

    def test_unexpected_failure_is_normalized_without_dynamic_details(self) -> None:
        verifier = Mock(return_value=0)
        sensitive_detail = "https://id.example.test/jwks?credential=must-not-leak"

        def exploding_preflight(**_kwargs):
            raise RuntimeError(sensitive_detail)

        result = run_acceptance(
            self.config(),
            self.evidence(),
            preflight_fn=exploding_preflight,
            verifier_fn=verifier,
        )

        self.assertEqual(result.phase, AcceptancePhase.INTERNAL)
        self.assertFalse(result.ready)
        self.assertNotIn(sensitive_detail, result.render())
        verifier.assert_not_called()

    def test_success_runs_preflight_before_verifier_and_returns_non_identifying_evidence(self) -> None:
        calls: list[str] = []

        def preflight(**kwargs):
            calls.append("preflight")
            self.assertEqual(kwargs["issuer"], self.config().issuer)
            self.assertEqual(kwargs["jwks_uri"], self.config().jwks_uri)
            self.assertEqual(kwargs["signature_mode"], self.config().signature_mode)
            self.assertEqual(kwargs["timeout_seconds"], self.config().timeout_seconds)
            return {"status": "ok"}

        def verifier(config: AcceptanceConfig) -> int:
            calls.append("verifier")
            self.assertEqual(config, self.config())
            return 0

        result = run_acceptance(
            self.config(),
            self.evidence(),
            preflight_fn=preflight,
            verifier_fn=verifier,
        )

        self.assertEqual(calls, ["preflight", "verifier"])
        self.assertTrue(result.ready)
        self.assertEqual(result.phase, AcceptancePhase.COMPLETE)
        rendered = result.render()
        self.assertEqual(
            rendered,
            "identity_acceptance.browser_flow=true\n"
            "identity_acceptance.rotation_rollback=true\n"
            "identity_acceptance.monitoring=true\n"
            "identity_acceptance.issuer_preflight=true\n"
            "identity_acceptance.access_token=true\n"
            "identity_acceptance.ready=true",
        )
        self.assertNotIn(self.config().issuer, rendered)
        self.assertNotIn(self.config().jwks_uri, rendered)
        self.assertNotIn(self.config().audience, rendered)

    def test_verifier_environment_is_explicit_and_never_contains_token_material(self) -> None:
        inherited = {
            "PATH": "/usr/bin",
            "AUTH_MODE": "development",
            "AUTH_ISSUER": "https://wrong.example.test",
            "ACCESS_TOKEN": "must-not-survive",
        }

        env = build_verifier_env(self.config(), inherited_env=inherited)

        self.assertEqual(env["PATH"], "/usr/bin")
        self.assertEqual(env["AUTH_MODE"], "jwt")
        self.assertEqual(env["AUTH_ISSUER"], self.config().issuer)
        self.assertEqual(env["AUTH_AUDIENCE"], self.config().audience)
        self.assertEqual(env["AUTH_JWKS_URI"], self.config().jwks_uri)
        self.assertEqual(env["AUTH_JWT_SIGNATURE_MODE"], "es384")
        self.assertEqual(env["AUTH_JWT_TYPE_MODE"], "at-jwt")
        self.assertEqual(env["AUTH_ACCESS_TOKEN_MAX_LIFETIME_SECONDS"], "300")
        self.assertNotIn("ACCESS_TOKEN", env)

    def test_verifier_inherits_stdin_and_suppresses_child_output(self) -> None:
        captured: dict[str, object] = {}

        def fake_runner(command, **kwargs):
            captured["command"] = command
            captured.update(kwargs)
            return Completed(0)

        status = run_verifier(
            self.config(),
            runner=fake_runner,
            inherited_env={"PATH": "/usr/bin"},
        )

        self.assertEqual(status, 0)
        self.assertEqual(
            captured["command"],
            [
                "cargo",
                "run",
                "--locked",
                "--quiet",
                "--manifest-path",
                "backend/Cargo.toml",
                "--bin",
                "verify_identity_access_token",
            ],
        )
        self.assertNotIn("input", captured)
        self.assertNotIn("stdin", captured)
        self.assertIn("stdout", captured)
        self.assertIn("stderr", captured)

    def test_cli_rejects_token_arguments_and_prints_only_stable_failure_evidence(self) -> None:
        stdout = io.StringIO()
        stderr = io.StringIO()

        with redirect_stdout(stdout), redirect_stderr(stderr):
            status = main(
                [
                    "--issuer",
                    self.config().issuer,
                    "--jwks-uri",
                    self.config().jwks_uri,
                    "--audience",
                    self.config().audience,
                    "--token",
                    "secret-token",
                ]
            )

        self.assertNotEqual(status, 0)
        combined = stdout.getvalue() + stderr.getvalue()
        self.assertNotIn("secret-token", combined)
        self.assertNotIn(self.config().issuer, combined)
        self.assertNotIn(self.config().jwks_uri, combined)


if __name__ == "__main__":
    unittest.main()
