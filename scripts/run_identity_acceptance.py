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
    ISSUER_PREFLIGHT = "issuer_preflight"
    ACCESS_TOKEN = "access_token"
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
class AcceptanceResult:
    phase: AcceptancePhase
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


def run_acceptance(
    config: AcceptanceConfig,
    *,
    preflight_fn: Callable[..., object] = run_preflight,
    verifier_fn: Callable[[AcceptanceConfig], int] = run_verifier,
) -> AcceptanceResult:
    try:
        preflight_fn(
            issuer=config.issuer,
            jwks_uri=config.jwks_uri,
            signature_mode=config.signature_mode,
            timeout_seconds=config.timeout_seconds,
        )
    except PreflightError:
        return AcceptanceResult(
            phase=AcceptancePhase.ISSUER_PREFLIGHT,
            issuer_preflight=False,
            access_token=None,
            ready=False,
        )

    if verifier_fn(config) != 0:
        return AcceptanceResult(
            phase=AcceptancePhase.ACCESS_TOKEN,
            issuer_preflight=True,
            access_token=False,
            ready=False,
        )

    return AcceptanceResult(
        phase=AcceptancePhase.COMPLETE,
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
        description=(
            "Run provider-neutral production identity issuer/JWKS and access-token acceptance checks"
        )
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
    return parser


def _usage_failure() -> AcceptanceResult:
    return AcceptanceResult(
        phase=AcceptancePhase.USAGE,
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
    result = run_acceptance(config)
    print(result.render())
    return 0 if result.ready else 1


if __name__ == "__main__":
    sys.exit(main())
