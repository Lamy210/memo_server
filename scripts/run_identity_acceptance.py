#!/usr/bin/env python3
from __future__ import annotations

import argparse
import math
import os
import subprocess
import sys
from dataclasses import dataclass
from enum import Enum
from pathlib import Path
from typing import Callable, Mapping, Sequence

from scripts.validate_identity_rollout import PreflightError, run_preflight

_VERIFIER_COMMAND = [
    "cargo",
    "run",
    "--locked",
    "--quiet",
    "--manifest-path",
    "backend/Cargo.toml",
    "--bin",
    "verify_identity_access_token",
]
_SIGNATURE_MODES = ("rs256", "rs256-es384", "es384")
_TOKEN_TYPE_MODES = ("legacy-any", "at-jwt")
_SENSITIVE_INHERITED_ENV_KEYS = {
    "ACCESS_TOKEN",
    "BEARER_TOKEN",
    "IDENTITY_ACCESS_TOKEN",
}
_REPOSITORY_ROOT = Path(__file__).resolve().parents[1]


class AcceptanceUsageError(RuntimeError):
    pass


class SafeArgumentParser(argparse.ArgumentParser):
    def error(self, _message: str) -> None:
        raise AcceptanceUsageError("invalid arguments")


class AcceptancePhase(Enum):
    USAGE = "usage"
    OPERATOR_EVIDENCE = "operator_evidence"
    ISSUER_PREFLIGHT = "issuer_preflight"
    ACCESS_TOKEN = "access_token"
    INTERNAL = "internal"
    COMPLETE = "complete"


@dataclass(frozen=True)
class AcceptanceConfig:
    issuer: str
    jwks_uri: str
    audience: str
    signature_mode: str
    token_type_mode: str
    max_access_token_lifetime_seconds: int
    timeout_seconds: float


@dataclass(frozen=True)
class AcceptanceEvidence:
    browser_flow_validated: bool
    rotation_rollback_validated: bool
    monitoring_validated: bool

    @property
    def complete(self) -> bool:
        return (
            self.browser_flow_validated
            and self.rotation_rollback_validated
            and self.monitoring_validated
        )


@dataclass(frozen=True)
class AcceptanceResult:
    phase: AcceptancePhase
    browser_flow_validated: bool | None
    rotation_rollback_validated: bool | None
    monitoring_validated: bool | None
    issuer_preflight: bool | None
    access_token: bool | None
    ready: bool

    @staticmethod
    def _status(value: bool | None) -> str:
        if value is None:
            return "not_run"
        return "true" if value else "false"

    def render(self) -> str:
        lines = [
            f"identity_acceptance.browser_flow={self._status(self.browser_flow_validated)}",
            "identity_acceptance.rotation_rollback="
            f"{self._status(self.rotation_rollback_validated)}",
            f"identity_acceptance.monitoring={self._status(self.monitoring_validated)}",
            f"identity_acceptance.issuer_preflight={self._status(self.issuer_preflight)}",
            f"identity_acceptance.access_token={self._status(self.access_token)}",
            f"identity_acceptance.ready={'true' if self.ready else 'false'}",
        ]
        if not self.ready:
            lines.append(f"identity_acceptance.failure_phase={self.phase.value}")
        return "\n".join(lines)


def build_verifier_env(
    config: AcceptanceConfig,
    *,
    inherited_env: Mapping[str, str] | None = None,
) -> dict[str, str]:
    env = dict(os.environ if inherited_env is None else inherited_env)
    for key in _SENSITIVE_INHERITED_ENV_KEYS:
        env.pop(key, None)

    env.update(
        {
            "AUTH_MODE": "jwt",
            "AUTH_ISSUER": config.issuer,
            "AUTH_AUDIENCE": config.audience,
            "AUTH_JWKS_URI": config.jwks_uri,
            "AUTH_JWT_SIGNATURE_MODE": config.signature_mode,
            "AUTH_JWT_TYPE_MODE": config.token_type_mode,
            "AUTH_ACCESS_TOKEN_MAX_LIFETIME_SECONDS": str(
                config.max_access_token_lifetime_seconds
            ),
        }
    )
    return env


def run_verifier(
    config: AcceptanceConfig,
    *,
    runner: Callable[..., object] = subprocess.run,
    inherited_env: Mapping[str, str] | None = None,
) -> int:
    try:
        completed = runner(
            list(_VERIFIER_COMMAND),
            cwd=_REPOSITORY_ROOT,
            env=build_verifier_env(config, inherited_env=inherited_env),
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        )
    except OSError:
        return 127

    return int(getattr(completed, "returncode", 1))


def _result(
    *,
    phase: AcceptancePhase,
    evidence: AcceptanceEvidence,
    issuer_preflight: bool | None,
    access_token: bool | None,
    ready: bool,
) -> AcceptanceResult:
    return AcceptanceResult(
        phase=phase,
        browser_flow_validated=evidence.browser_flow_validated,
        rotation_rollback_validated=evidence.rotation_rollback_validated,
        monitoring_validated=evidence.monitoring_validated,
        issuer_preflight=issuer_preflight,
        access_token=access_token,
        ready=ready,
    )


def run_acceptance(
    config: AcceptanceConfig,
    evidence: AcceptanceEvidence,
    *,
    preflight_fn: Callable[..., object] = run_preflight,
    verifier_fn: Callable[[AcceptanceConfig], int] = run_verifier,
) -> AcceptanceResult:
    if not evidence.complete:
        return _result(
            phase=AcceptancePhase.OPERATOR_EVIDENCE,
            evidence=evidence,
            issuer_preflight=None,
            access_token=None,
            ready=False,
        )

    try:
        preflight_fn(
            issuer=config.issuer,
            jwks_uri=config.jwks_uri,
            signature_mode=config.signature_mode,
            timeout_seconds=config.timeout_seconds,
        )
    except PreflightError:
        return _result(
            phase=AcceptancePhase.ISSUER_PREFLIGHT,
            evidence=evidence,
            issuer_preflight=False,
            access_token=None,
            ready=False,
        )
    except Exception:
        return _result(
            phase=AcceptancePhase.INTERNAL,
            evidence=evidence,
            issuer_preflight=False,
            access_token=None,
            ready=False,
        )

    if verifier_fn(config) != 0:
        return _result(
            phase=AcceptancePhase.ACCESS_TOKEN,
            evidence=evidence,
            issuer_preflight=True,
            access_token=False,
            ready=False,
        )

    return _result(
        phase=AcceptancePhase.COMPLETE,
        evidence=evidence,
        issuer_preflight=True,
        access_token=True,
        ready=True,
    )


def _positive_int(value: str) -> int:
    try:
        parsed = int(value)
    except ValueError as error:
        raise argparse.ArgumentTypeError("must be an integer") from error
    if parsed <= 0:
        raise argparse.ArgumentTypeError("must be positive")
    return parsed


def _positive_finite_float(value: str) -> float:
    try:
        parsed = float(value)
    except ValueError as error:
        raise argparse.ArgumentTypeError("must be a number") from error
    if not math.isfinite(parsed) or parsed <= 0:
        raise argparse.ArgumentTypeError("must be a positive finite number")
    return parsed


def _parser() -> SafeArgumentParser:
    parser = SafeArgumentParser(
        description="Run the provider-neutral production identity acceptance gate"
    )
    parser.add_argument("--issuer", required=True)
    parser.add_argument("--jwks-uri", required=True)
    parser.add_argument("--audience", required=True)
    parser.add_argument("--signature-mode", required=True, choices=_SIGNATURE_MODES)
    parser.add_argument("--token-type-mode", required=True, choices=_TOKEN_TYPE_MODES)
    parser.add_argument(
        "--max-access-token-lifetime-seconds",
        required=True,
        type=_positive_int,
    )
    parser.add_argument(
        "--timeout-seconds",
        type=_positive_finite_float,
        default=10.0,
    )
    parser.add_argument("--browser-flow-validated", action="store_true")
    parser.add_argument("--rotation-rollback-validated", action="store_true")
    parser.add_argument("--monitoring-validated", action="store_true")
    return parser


def _usage_failure() -> AcceptanceResult:
    return AcceptanceResult(
        phase=AcceptancePhase.USAGE,
        browser_flow_validated=None,
        rotation_rollback_validated=None,
        monitoring_validated=None,
        issuer_preflight=None,
        access_token=None,
        ready=False,
    )


def main(argv: Sequence[str] | None = None) -> int:
    try:
        args = _parser().parse_args(argv)
    except AcceptanceUsageError:
        print(_usage_failure().render())
        return 2

    config = AcceptanceConfig(
        issuer=args.issuer,
        jwks_uri=args.jwks_uri,
        audience=args.audience,
        signature_mode=args.signature_mode,
        token_type_mode=args.token_type_mode,
        max_access_token_lifetime_seconds=args.max_access_token_lifetime_seconds,
        timeout_seconds=args.timeout_seconds,
    )
    evidence = AcceptanceEvidence(
        browser_flow_validated=args.browser_flow_validated,
        rotation_rollback_validated=args.rotation_rollback_validated,
        monitoring_validated=args.monitoring_validated,
    )
    result = run_acceptance(config, evidence)
    print(result.render())
    return 0 if result.ready else 1


if __name__ == "__main__":
    sys.exit(main())
