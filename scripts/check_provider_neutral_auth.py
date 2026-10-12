#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import re
from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class Finding:
    path: str
    reason: str


_TEXT_FILES = (
    Path("README.md"),
    Path("CONTRIBUTING.md"),
    Path("docs/authentication.md"),
    Path("docker-compose.yml"),
)
_TEXT_DIRS = (
    Path("frontend/src"),
    Path("backend/src"),
    Path(".github/workflows"),
)
_MANIFESTS = (
    Path("frontend/package.json"),
    Path("backend/Cargo.toml"),
)
_ROOT_CURRENT_DOC_SUFFIXES = {".md", ".txt"}

# This intentionally spans several common implementations. The check is an
# abstraction guard, not a declaration of which implementation is deployed.
_PROVIDER_FINGERPRINT_WORDS = (
    "auth0",
    "authentik",
    "clerk",
    "cognito",
    "fusionauth",
    "hydra",
    "keycloak",
    "keto",
    "kratos",
    "logto",
    "ory",
    "workos",
    "zitadel",
)
_PROVIDER_WORD_RE = re.compile(
    r"(?i)(?<![A-Za-z0-9])(?:" + "|".join(map(re.escape, _PROVIDER_FINGERPRINT_WORDS)) + r")(?![A-Za-z0-9])"
)
_CONCRETE_UPSTREAM_AUTH_RE = re.compile(
    r"(?i)(?<![A-Za-z0-9])(?:github|google|microsoft|apple)\s+(?:oauth|oidc|login|identity)(?![A-Za-z0-9])"
)
_INTERNAL_PLATFORM_RE = re.compile(r"\b[A-Z][A-Za-z0-9]*(?:Auth|Identity)?Platform[A-Z][A-Za-z0-9]*\b")


def _iter_existing_text_paths(root: Path):
    yielded: set[Path] = set()

    for relative in _TEXT_FILES:
        path = root / relative
        if path.is_file():
            yielded.add(relative)
            yield relative, path

    for path in sorted(root.iterdir()):
        if not path.is_file() or path.suffix.lower() not in _ROOT_CURRENT_DOC_SUFFIXES:
            continue
        relative = path.relative_to(root)
        if relative not in yielded:
            yielded.add(relative)
            yield relative, path

    for relative_dir in _TEXT_DIRS:
        directory = root / relative_dir
        if not directory.is_dir():
            continue
        for path in sorted(directory.rglob("*")):
            if not path.is_file():
                continue
            if path.suffix.lower() not in {".md", ".rs", ".ts", ".svelte", ".js", ".cjs", ".mjs", ".json", ".yml", ".yaml"}:
                continue
            relative = path.relative_to(root)
            if relative not in yielded:
                yielded.add(relative)
                yield relative, path


def _text_findings(relative: Path, text: str) -> list[Finding]:
    findings: list[Finding] = []
    if _PROVIDER_WORD_RE.search(text) or _CONCRETE_UPSTREAM_AUTH_RE.search(text):
        findings.append(Finding(relative.as_posix(), "provider implementation fingerprint in current boundary"))
    if _INTERNAL_PLATFORM_RE.search(text):
        findings.append(Finding(relative.as_posix(), "internal platform identifier in current boundary"))
    return findings


def _dependency_names(path: Path) -> list[str]:
    if path.name == "package.json":
        payload = json.loads(path.read_text(encoding="utf-8"))
        names: list[str] = []
        for section in ("dependencies", "devDependencies", "peerDependencies", "optionalDependencies"):
            values = payload.get(section, {})
            if isinstance(values, dict):
                names.extend(str(name) for name in values)
        return names

    # Cargo dependency declarations are simple enough for this guard: inspect
    # dependency keys and package aliases, not lockfile content or source URLs.
    text = path.read_text(encoding="utf-8")
    names = []
    in_dependencies = False
    for raw_line in text.splitlines():
        line = raw_line.strip()
        if line.startswith("[") and line.endswith("]"):
            in_dependencies = "dependencies" in line.lower()
            continue
        if not in_dependencies or not line or line.startswith("#") or "=" not in line:
            continue
        names.append(line.split("=", 1)[0].strip().strip('"'))
    return names


def check_paths(root: Path) -> list[Finding]:
    root = root.resolve()
    findings: list[Finding] = []

    for relative, path in _iter_existing_text_paths(root):
        text = path.read_text(encoding="utf-8", errors="replace")
        findings.extend(_text_findings(relative, text))

    for relative in _MANIFESTS:
        path = root / relative
        if not path.is_file():
            continue
        for dependency in _dependency_names(path):
            if _PROVIDER_WORD_RE.search(dependency) or _INTERNAL_PLATFORM_RE.search(dependency):
                findings.append(
                    Finding(relative.as_posix(), f"provider-specific dependency: {dependency}")
                )

    return sorted(set(findings), key=lambda finding: (finding.path, finding.reason))


def main() -> int:
    parser = argparse.ArgumentParser(description="Check provider-neutral authentication boundaries")
    parser.add_argument("root", nargs="?", default=".")
    args = parser.parse_args()

    findings = check_paths(Path(args.root))
    if not findings:
        print("provider-neutral auth boundary: ok")
        return 0

    for finding in findings:
        print(f"{finding.path}: {finding.reason}")
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
