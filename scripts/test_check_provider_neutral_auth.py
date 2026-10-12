#!/usr/bin/env python3
import json
import tempfile
import unittest
from pathlib import Path

from scripts.check_provider_neutral_auth import check_paths


class ProviderNeutralAuthScannerTests(unittest.TestCase):
    def write(self, root: Path, relative: str, content: str) -> None:
        path = root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8")

    def test_detects_vendor_fingerprint_in_public_auth_docs(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.write(root, "docs/authentication.md", "Authentication uses Keycloak directly.\n")

            findings = check_paths(root)

            self.assertTrue(any(f.path == "docs/authentication.md" for f in findings))

    def test_detects_vendor_fingerprint_in_contributing_guidance(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.write(root, "CONTRIBUTING.md", "Do not depend directly on Ory.\n")

            findings = check_paths(root)

            self.assertTrue(any(f.path == "CONTRIBUTING.md" for f in findings))

    def test_detects_concrete_upstream_login_provider_in_root_current_doc(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.write(root, "仕様書.txt", "認証はGitHub OAuthログインに対応します。\n")

            findings = check_paths(root)

            self.assertTrue(any(f.path == "仕様書.txt" for f in findings))

    def test_detects_internal_camel_case_platform_identifier_without_naming_it(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.write(
                root,
                "README.md",
                "The application calls ExampleIdentityPlatformInternal for login.\n",
            )

            findings = check_paths(root)

            self.assertTrue(any(f.path == "README.md" for f in findings))

    def test_allows_standards_terms(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.write(
                root,
                "docs/authentication.md",
                "OIDC OAuth JWKS JWT resource server authorization server identity provider\n",
            )

            self.assertEqual(check_paths(root), [])

    def test_ignores_architecture_review_artifacts(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.write(
                root,
                "docs/superpowers/specs/archival-example.md",
                "Keycloak ExampleIdentityPlatformInternal\n",
            )

            self.assertEqual(check_paths(root), [])

    def test_detects_provider_sdk_dependency(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            package_json = {
                "dependencies": {"@auth0/nextjs-auth0": "1.0.0", "marked": "15.0.4"},
                "devDependencies": {},
            }
            self.write(root, "frontend/package.json", json.dumps(package_json))

            findings = check_paths(root)

            self.assertTrue(
                any(
                    f.path == "frontend/package.json" and "provider-specific dependency" in f.reason
                    for f in findings
                )
            )


if __name__ == "__main__":
    unittest.main()
